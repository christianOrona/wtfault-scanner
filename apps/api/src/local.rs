//! Only this machine's own interface may call the API.
//!
//! The server binds to loopback, and for a long time that was the whole of the
//! argument: nothing off the machine can connect, so nothing needs to prove
//! who it is. It is not the whole of it. A browser on this machine can connect,
//! and a browser runs pages from everywhere:
//!
//! * A page on another site whose host name is made to resolve to `127.0.0.1`
//!   is, to the browser, talking to its own origin. It can read every answer
//!   and send any request, including the ones that change a vehicle.
//! * Any page at all can open a WebSocket to a loopback port, and the browser
//!   does not stop it. That is the live-data stream.
//!
//! Neither needs anything installed. So two things a browser cannot lie about
//! are checked on every request:
//!
//! * **`Host`** names this machine. A page reached through somebody else's host
//!   name sends that name, however it resolves.
//! * **`Origin`**, when the browser sends one, is this machine too. It is sent
//!   on every WebSocket handshake and every request that is not a plain read.
//!
//! A program that is not a browser can send whatever it likes in both. That is
//! a program running as the person, and it can open the serial port itself.

use crate::error::ApiError;
use aim_types::{AimError, ErrorCode};
use axum::extract::Request;
use axum::http::{header, HeaderMap};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Refuse a request that did not come from this machine's own interface.
pub async fn local_only(request: Request, next: Next) -> Response {
    let authority = request.uri().authority().map(|a| a.as_str().to_string());
    if let Err(why) = check(request.headers(), authority.as_deref()) {
        tracing::warn!(reason = why, path = %request.uri().path(), "refused a request that is not local");
        return ApiError::new(
            AimError::new(
                ErrorCode::OperationNotAllowed,
                format!("this API answers the application on this machine and nothing else: {why}"),
            )
            .with_details(serde_json::json!({ "reason": why })),
        )
        .into_response();
    }
    next.run(request).await
}

/// Whether a request's headers say it came from here. `authority` is the
/// request target's own, for a request that carries one instead of `Host`.
pub fn check(headers: &HeaderMap, authority: Option<&str>) -> Result<(), &'static str> {
    let host = match headers.get(header::HOST) {
        Some(value) => Some(value.to_str().map_err(|_| "the Host header is not readable")?),
        None => authority,
    };
    match host {
        Some(host) if is_this_machine(host_of(host)) => {}
        Some(_) => return Err("it was addressed to a host name that is not this machine"),
        None => return Err("it does not say which host it was addressed to"),
    }

    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin.to_str().map_err(|_| "the Origin header is not readable")?;
        if !is_local_origin(origin) {
            return Err("it came from a page on another origin");
        }
    }

    // What a current browser says about where a request came from. Absent from
    // anything that is not a browser, and from old ones, so it can only refuse.
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        if site.eq_ignore_ascii_case("cross-site") {
            return Err("the browser reports it as coming from another site");
        }
    }
    Ok(())
}

/// The host part of `host[:port]`, with an IPv6 literal kept in its brackets.
fn host_of(authority: &str) -> &str {
    let authority = authority.trim();
    if let Some(rest) = authority.strip_prefix('[') {
        return match rest.find(']') {
            Some(end) => &authority[..end + 2],
            None => authority,
        };
    }
    authority.rsplit_once(':').map_or(authority, |(host, _port)| host)
}

/// Loopback, by name or by address. `*.localhost` is loopback by definition
/// and is what a desktop shell's own pages are served from.
fn is_this_machine(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "[::1]"
}

fn is_local_origin(origin: &str) -> bool {
    let origin = origin.trim();
    let Some((scheme, rest)) = origin.split_once("://") else {
        // `null`, which is what a sandboxed frame or a file sends.
        return false;
    };
    if !["http", "https", "tauri"].contains(&scheme.to_ascii_lowercase().as_str()) {
        return false;
    }
    // An origin has no path, and nothing before an `@` is a host.
    if rest.contains('/') || rest.contains('@') {
        return false;
    }
    is_this_machine(host_of(rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn the_applications_own_requests_pass() {
        for request in [
            // curl, the tests, the desktop window reading.
            vec![("host", "127.0.0.1:8787")],
            vec![("host", "localhost:8787")],
            vec![("host", "[::1]:8787")],
            vec![("host", "127.0.0.1")],
            // The desktop window changing something.
            vec![("host", "127.0.0.1:8787"), ("origin", "http://127.0.0.1:8787")],
            vec![
                ("host", "127.0.0.1:51234"),
                ("origin", "http://127.0.0.1:51234"),
                ("sec-fetch-site", "same-origin"),
            ],
            // The dev server, which forwards the page's origin.
            vec![("host", "127.0.0.1:8787"), ("origin", "http://localhost:5173")],
            // A shell serving its pages from its own scheme.
            vec![("host", "127.0.0.1:8787"), ("origin", "tauri://localhost")],
            vec![("host", "127.0.0.1:8787"), ("origin", "http://tauri.localhost")],
        ] {
            assert_eq!(check(&headers(&request), None), Ok(()), "{request:?}");
        }
        // A request with its authority in the target rather than a header.
        assert_eq!(check(&HeaderMap::new(), Some("127.0.0.1:8787")), Ok(()));
    }

    /// A page loaded from a name that has been made to resolve here.
    #[test]
    fn a_request_addressed_to_another_name_is_refused() {
        for host in [
            "attacker.example",
            "attacker.example:8787",
            "127.0.0.1.attacker.example",
            "localhost.attacker.example:8787",
            "127.0.0.2:8787",
            "192.168.1.20:8787",
            "[::2]:8787",
            "",
        ] {
            assert!(check(&headers(&[("host", host)]), None).is_err(), "{host:?}");
        }
        assert!(check(&HeaderMap::new(), None).is_err(), "no host at all");
        assert!(check(&HeaderMap::new(), Some("attacker.example")).is_err());
    }

    /// A page on another site, reaching the right address.
    #[test]
    fn a_request_from_another_origin_is_refused() {
        for origin in [
            "http://attacker.example",
            "https://attacker.example:8787",
            "http://127.0.0.1.attacker.example",
            "http://localhost@attacker.example",
            "http://attacker.example/127.0.0.1",
            "null",
            "file://",
            "chrome-extension://abcdef",
            "",
        ] {
            let request = headers(&[("host", "127.0.0.1:8787"), ("origin", origin)]);
            assert!(check(&request, None).is_err(), "{origin:?}");
        }
        let cross = headers(&[("host", "127.0.0.1:8787"), ("sec-fetch-site", "cross-site")]);
        assert!(check(&cross, None).is_err());
    }
}
