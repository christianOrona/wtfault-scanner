//! A fetched OBDb signal set is kept where community sets are loaded from (#56).

use aim_decoders::catalog::SignalCatalog;
use aim_decoders::obdb::{cache_signalset, cached_signalset, MAX_SIGNALSET_BYTES};
use aim_types::ErrorCode;
use std::fs;
use std::path::PathBuf;

const F150: &str = include_str!("../../../vehicle-profiles/catalog/obdb/Ford-F-150.json");

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aim-obdb-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_fetched_set_is_kept_where_community_sets_are_loaded_from() {
    let dir = temp("kept");
    assert!(cached_signalset(&dir, "Ford-F-150").is_none());

    let path = cache_signalset(&dir, "Ford-F-150", F150).unwrap();
    assert_eq!(path, dir.join("catalog").join("obdb").join("Ford-F-150.json"));
    assert_eq!(cached_signalset(&dir, "Ford-F-150"), Some(path));

    let (catalog, warnings) = SignalCatalog::load_dir(dir.join("catalog").join("obdb"));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(catalog.keys(), vec!["Ford-F-150"]);
}

#[test]
fn the_attribution_is_kept_with_the_file() {
    let dir = temp("attribution");
    cache_signalset(&dir, "Mazda-3", F150).unwrap();
    let note = fs::read_to_string(dir.join("catalog").join("obdb").join("Mazda-3.ATTRIBUTION.md"))
        .unwrap();
    assert!(note.contains("CC BY-SA 4.0"), "{note}");
    assert!(note.contains("https://github.com/OBDb/Mazda-3"), "{note}");
    assert!(note.contains("https://obdb.community"), "{note}");
}

#[test]
fn something_that_is_not_a_signal_set_is_not_kept() {
    let dir = temp("not-a-set");
    let err = cache_signalset(&dir, "Mazda-3", "<html>404: Not Found</html>").unwrap_err();
    assert_eq!(err.code, ErrorCode::DecoderInputInvalid);
    assert!(cached_signalset(&dir, "Mazda-3").is_none());
}

#[test]
fn an_oversized_reply_is_not_kept() {
    let dir = temp("oversized");
    let body = " ".repeat(MAX_SIGNALSET_BYTES + 1);
    assert_eq!(
        cache_signalset(&dir, "Mazda-3", &body).unwrap_err().code,
        ErrorCode::DecoderInputInvalid
    );
    assert!(cached_signalset(&dir, "Mazda-3").is_none());
}

/// Only letters, digits and hyphens: nothing that could name another folder.
#[test]
fn a_repository_name_cannot_point_outside_the_folder() {
    let dir = temp("escape");
    for bad in [
        "",
        "../Ford-F-150",
        "Ford/F-150",
        r"Ford\F-150",
        "Ford F-150",
        "C:Ford",
        "..",
        "Ford.F-150",
        "CON",
        "-F-150",
    ] {
        let err = cache_signalset(&dir, bad, F150).expect_err(bad);
        assert_eq!(err.code, ErrorCode::DecoderInputInvalid, "{bad}");
        assert!(cached_signalset(&dir, bad).is_none(), "{bad}");
    }
    assert!(!dir.exists() || fs::read_dir(&dir).unwrap().next().is_none(), "nothing written");
}
