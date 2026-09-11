//! What leaves this machine, and where it goes.
//!
//! # The distinction that matters is not local versus hosted
//!
//! It is *whose machine*. Three situations, and they are genuinely different:
//!
//! 1. **This computer.** Ollama on `127.0.0.1`. The VIN never leaves the
//!    machine it was read on, and withholding it buys nothing.
//! 2. **Another machine you control.** Ollama on a GPU box in the next room.
//!    The VIN crosses a network. Whether that matters is the owner's judgement
//!    and nobody else's, and calling it "local" would make that judgement for
//!    them by hiding the fact there is one.
//! 3. **Somebody else's machine.** Anthropic, xAI, an OpenAI-compatible
//!    endpoint on the internet. The VIN goes to a company, is subject to their
//!    retention, and may be used in ways the owner does not control.
//!
//! Most software collapses the first two into "local" because the config file
//! says `ollama`. That is wrong, and it is wrong in the direction that
//! understates the exposure.
//!
//! # Why the VIN specifically
//!
//! A VIN identifies one vehicle and, in practice, one person. Paired with a
//! fault history and a location it is more identifying than most things people
//! are careful about. Nothing else this app reads comes close: a coolant
//! temperature is not about anybody.
//!
//! # What withholding costs
//!
//! Some usefulness, honestly. The model cannot use the VIN to reason about
//! which vehicle this is, and this project does not decode a model from a VIN
//! anyway — so the loss is smaller than it looks, and it is stated rather than
//! glossed.

use crate::settings::{ProviderConfig, ProviderKind};
use serde::{Deserialize, Serialize};

/// Where a configured provider actually runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locality {
    /// This computer. Nothing crosses a network.
    ThisMachine,
    /// Another machine on the owner's own network.
    YourNetwork,
    /// Somebody else's infrastructure.
    SomebodyElse,
}

impl Locality {
    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Locality::ThisMachine => "this_machine",
            Locality::YourNetwork => "your_network",
            Locality::SomebodyElse => "somebody_else",
        }
    }

    /// Where the data goes, in one sentence meant for a person.
    pub fn explain(&self) -> &'static str {
        match self {
            Locality::ThisMachine => {
                "This model runs on this computer. Nothing you ask it leaves the machine."
            }
            Locality::YourNetwork => {
                "This model runs on another machine on your own network. What you ask it crosses \
                 your network but does not leave it — assuming that machine is yours and you \
                 know what it does with what it is sent."
            }
            Locality::SomebodyElse => {
                "This model runs on somebody else's computers. Everything you ask it — including \
                 anything identifying about the vehicle — is sent to them, and what happens to \
                 it afterwards is governed by their terms and not by this app."
            }
        }
    }

    /// Whether identifying data is exposed beyond the owner's control.
    pub fn leaves_your_control(&self) -> bool {
        matches!(self, Locality::SomebodyElse)
    }
}

/// Hosts that mean "this very machine".
const LOOPBACK: [&str; 4] = ["127.0.0.1", "localhost", "::1", "[::1]"];

/// Where a provider runs, from its kind and its endpoint.
///
/// Anthropic and xAI are somebody else's by definition. Everything else is
/// decided by the host in the URL, because "ollama" says what dialect it
/// speaks and nothing at all about where it is: an Ollama endpoint on a GPU
/// box in the next room is not this machine, and an OpenAI-compatible endpoint
/// on `127.0.0.1` is.
pub fn locality_of(config: &ProviderConfig) -> Locality {
    match config.kind {
        ProviderKind::Anthropic | ProviderKind::Xai => Locality::SomebodyElse,
        ProviderKind::Ollama | ProviderKind::OpenAiCompatible => {
            let url = config
                .base_url
                .clone()
                .or_else(|| config.kind.default_base_url().map(String::from))
                .unwrap_or_default();
            match host_of(&url) {
                // No host to judge. Treated as somebody else's, because an
                // unparseable endpoint is not evidence of safety and this is
                // the one place a wrong guess costs somebody their privacy.
                None => Locality::SomebodyElse,
                Some(h) if LOOPBACK.contains(&h.as_str()) => Locality::ThisMachine,
                Some(h) if is_private_network(&h) => Locality::YourNetwork,
                Some(_) => Locality::SomebodyElse,
            }
        }
    }
}

/// The host portion of a URL, lower-cased and without its port.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split(['/', '?']).next()?;
    // Strip credentials, then the port. IPv6 literals keep their brackets,
    // which is why the port is taken from after the closing one.
    let after_credentials = authority.rsplit('@').next()?;
    let host = match after_credentials.strip_prefix('[') {
        Some(v6) => format!("[{}]", v6.split(']').next()?),
        None => after_credentials.split(':').next()?.to_string(),
    };
    let host = host.trim().to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether a host is on a private network rather than the public internet.
///
/// The RFC 1918 ranges plus link-local and `.local`. Deliberately conservative:
/// anything not recognisably private is treated as somebody else's, because
/// being wrong in that direction over-warns and being wrong in the other
/// direction sends a VIN somewhere the owner did not expect.
fn is_private_network(host: &str) -> bool {
    if host.ends_with(".local") || host.ends_with(".lan") || host.ends_with(".home") {
        return true;
    }
    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    let Ok(a) = octets[0].parse::<u8>() else { return false };
    let Ok(b) = octets[1].parse::<u8>() else { return false };
    if octets[2].parse::<u8>().is_err() || octets[3].parse::<u8>().is_err() {
        return false;
    }
    match a {
        10 => true,
        172 => (16..=31).contains(&b),
        192 => b == 168,
        // Link-local, the address a machine gives itself with no DHCP.
        169 => b == 254,
        _ => false,
    }
}

/// Whether identifying data is withheld from a provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareIdentifiers {
    /// Withhold from anything that is not this machine or the owner's own
    /// network. The default, because it is the choice somebody would make if
    /// they had thought about it, and the cost is small.
    #[default]
    NotBeyondYourNetwork,
    /// Withhold from everything, including a model on this machine. For
    /// somebody who would rather the VIN never appear in a prompt at all.
    Never,
    /// Send it wherever the model runs. A deliberate choice, and it is theirs
    /// to make.
    Always,
}

impl ShareIdentifiers {
    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            ShareIdentifiers::NotBeyondYourNetwork => "not_beyond_your_network",
            ShareIdentifiers::Never => "never",
            ShareIdentifiers::Always => "always",
        }
    }

    /// Whether the VIN may be sent to a provider in this situation.
    pub fn may_send_to(&self, where_it_runs: Locality) -> bool {
        match self {
            ShareIdentifiers::Always => true,
            ShareIdentifiers::Never => false,
            ShareIdentifiers::NotBeyondYourNetwork => !where_it_runs.leaves_your_control(),
        }
    }
}

/// Replace a VIN with something that says one was withheld.
///
/// Not blanked. A prompt that simply omits the VIN reads as a vehicle that was
/// never identified, and a model told nothing will say the VIN could not be
/// read — which is false and would send somebody looking for a fault that is
/// not there. Saying it was withheld is both true and useful: the model knows
/// the vehicle is identified, knows it cannot see how, and can say so.
pub const WITHHELD: &str = "withheld by this app's privacy setting (the vehicle IS identified; \
                            the VIN is simply not being sent to a model that runs outside the \
                            owner's control)";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Speed;

    fn provider(kind: ProviderKind, base_url: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            id: "p1".into(),
            kind,
            label: "test".into(),
            base_url: base_url.map(String::from),
            model: "m".into(),
            api_key: None,
            speed: Speed::Quality,
            max_steps: None,
            max_tokens: None,
            context_tokens: None,
        }
    }

    /// The distinction the whole module exists for. "Ollama" says what dialect
    /// an endpoint speaks and nothing about where it is.
    #[test]
    fn ollama_on_another_box_is_not_this_machine() {
        let here = provider(ProviderKind::Ollama, Some("http://127.0.0.1:11434"));
        assert_eq!(locality_of(&here), Locality::ThisMachine);

        let next_room = provider(ProviderKind::Ollama, Some("http://192.168.1.207:11434"));
        assert_eq!(locality_of(&next_room), Locality::YourNetwork);
        assert!(!locality_of(&next_room).leaves_your_control(), "still the owner's own network");

        let somewhere = provider(ProviderKind::Ollama, Some("https://ollama.example.com"));
        assert_eq!(locality_of(&somewhere), Locality::SomebodyElse);
    }

    /// And the reverse: an OpenAI-compatible server on this machine is on this
    /// machine, whatever dialect it speaks.
    #[test]
    fn an_openai_dialect_on_loopback_is_still_this_machine() {
        let lm_studio = provider(ProviderKind::OpenAiCompatible, Some("http://localhost:1234/v1"));
        assert_eq!(locality_of(&lm_studio), Locality::ThisMachine);
    }

    /// Hosted providers are somebody else's by definition, whatever URL is set.
    #[test]
    fn a_hosted_provider_is_somebody_elses_regardless_of_url() {
        for kind in [ProviderKind::Anthropic, ProviderKind::Xai] {
            let p = provider(kind, Some("http://127.0.0.1:9999"));
            assert_eq!(
                locality_of(&p),
                Locality::SomebodyElse,
                "a loopback URL does not make Anthropic run on your desk"
            );
        }
    }

    /// An endpoint we cannot parse is treated as somebody else's. Being wrong
    /// in that direction over-warns; being wrong in the other sends a VIN
    /// somewhere the owner did not expect.
    #[test]
    fn an_unparseable_endpoint_fails_towards_privacy() {
        let broken = provider(ProviderKind::OpenAiCompatible, Some(""));
        assert_eq!(locality_of(&broken), Locality::SomebodyElse);
    }

    /// A host that merely looks private is not. `192.168.1.1.evil.com` is on
    /// the internet.
    #[test]
    fn a_public_host_dressed_up_as_a_private_one_is_still_public() {
        let sneaky =
            provider(ProviderKind::OpenAiCompatible, Some("https://192.168.1.1.evil.com/v1"));
        assert_eq!(locality_of(&sneaky), Locality::SomebodyElse);
    }

    /// The default withholds from somebody else's machines and nowhere nearer.
    #[test]
    fn the_default_withholds_only_where_it_leaves_your_control() {
        let d = ShareIdentifiers::default();
        assert!(d.may_send_to(Locality::ThisMachine));
        assert!(d.may_send_to(Locality::YourNetwork));
        assert!(!d.may_send_to(Locality::SomebodyElse));
    }

    /// And the two deliberate choices either side of it.
    #[test]
    fn never_and_always_mean_what_they_say() {
        for l in [Locality::ThisMachine, Locality::YourNetwork, Locality::SomebodyElse] {
            assert!(!ShareIdentifiers::Never.may_send_to(l));
            assert!(ShareIdentifiers::Always.may_send_to(l));
        }
    }

    /// Withheld, not blank. A model told nothing says the VIN could not be
    /// read, which is false and sends somebody looking for a fault that is not
    /// there.
    #[test]
    fn a_withheld_vin_says_the_vehicle_is_still_identified() {
        assert!(WITHHELD.contains("IS identified"));
        assert!(WITHHELD.contains("not being sent"));
    }

    #[test]
    fn credentials_and_ports_do_not_confuse_the_host() {
        assert_eq!(host_of("http://user:pw@192.168.0.5:8080/v1").as_deref(), Some("192.168.0.5"));
        assert_eq!(host_of("http://[::1]:11434").as_deref(), Some("[::1]"));
        assert_eq!(host_of("https://API.Example.COM/v1").as_deref(), Some("api.example.com"));
    }
    /// The whole point, end to end: the same vehicle, three providers, and the
    /// VIN reaching only the two that are the owner's own machines.
    #[test]
    fn the_same_vin_reaches_your_own_machines_and_not_a_company() {
        let setting = ShareIdentifiers::default();
        let cases = [
            (provider(ProviderKind::Ollama, Some("http://127.0.0.1:11434")), true),
            (provider(ProviderKind::Ollama, Some("http://192.168.1.207:11434")), true),
            (provider(ProviderKind::Anthropic, None), false),
            (provider(ProviderKind::Xai, None), false),
            (provider(ProviderKind::OpenAiCompatible, Some("https://openrouter.ai/api/v1")), false),
        ];
        for (p, expect_sent) in cases {
            let sent = setting.may_send_to(locality_of(&p));
            assert_eq!(sent, expect_sent, "{:?} at {:?}", p.kind, p.base_url);
        }
    }
}
