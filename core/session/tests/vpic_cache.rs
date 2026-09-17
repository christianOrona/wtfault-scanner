//! A vPIC reply is cached against the VIN, so each VIN is looked up once (#55).

use aim_session::{latest_version, SessionStore};

const VIN: &str = "1FT7W2BT7KEF78036";
const URL: &str =
    "https://vpic.nhtsa.dot.gov/api/vehicles/DecodeVinValues/1FT7W2BT7KEF78036?format=json";
const BODY: &str = r#"{"Count":1,"Results":[{"Make":"FORD"}]}"#;

fn store() -> SessionStore {
    SessionStore::open_in_memory().unwrap()
}

#[test]
fn a_reply_is_kept_against_its_vin() {
    let s = store();
    assert!(s.vpic_reply(VIN).unwrap().is_none());

    s.store_vpic_reply("1ft7w2bt7kef78036", URL, BODY).unwrap();

    let kept = s.vpic_reply(VIN).unwrap().expect("cached");
    assert_eq!(kept.vin, VIN, "kept upper case");
    assert_eq!(kept.source_url, URL);
    assert_eq!(kept.body, BODY);
    assert!(!kept.fetched_at.is_empty());
    assert!(s.vpic_reply("1ft7w2bt7kef78036").unwrap().is_some(), "looked up in any case");
}

#[test]
fn a_newer_reply_replaces_the_older_one() {
    let s = store();
    s.store_vpic_reply(VIN, URL, BODY).unwrap();
    s.store_vpic_reply(VIN, URL, r#"{"Results":[]}"#).unwrap();
    assert_eq!(s.vpic_reply(VIN).unwrap().unwrap().body, r#"{"Results":[]}"#);
}

#[test]
fn replies_do_not_leak_between_vehicles() {
    let s = store();
    s.store_vpic_reply(VIN, URL, BODY).unwrap();
    assert!(s.vpic_reply("JM1BPACL9K1100001").unwrap().is_none());
}

#[test]
fn a_cached_reply_can_be_forgotten() {
    let s = store();
    s.store_vpic_reply(VIN, URL, BODY).unwrap();
    assert!(s.forget_vpic_reply("1ft7w2bt7kef78036").unwrap());
    assert!(s.vpic_reply(VIN).unwrap().is_none());
    assert!(!s.forget_vpic_reply(VIN).unwrap(), "nothing left to forget");
}

#[test]
fn the_cache_arrives_as_a_schema_migration() {
    assert_eq!(latest_version(), 6);
}
