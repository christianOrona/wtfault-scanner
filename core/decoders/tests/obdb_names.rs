//! OBDb repository names from a vPIC make and model (#56).

use aim_decoders::obdb::{repository_name, repository_url, signalset_url};

#[test]
fn the_bundled_set_is_named_the_way_obdb_names_it() {
    assert_eq!(repository_name("FORD", "F-150").as_deref(), Some("Ford-F-150"));
    assert_eq!(repository_name("FORD", "F-250").as_deref(), Some("Ford-F-250"));
}

/// vPIC says "Mazda3"; OBDb says "Mazda-3". The make is not repeated.
#[test]
fn a_model_that_repeats_the_make_is_not_named_twice() {
    assert_eq!(repository_name("MAZDA", "Mazda3").as_deref(), Some("Mazda-3"));
    assert_eq!(repository_name("MAZDA", "Mazda 6").as_deref(), Some("Mazda-6"));
}

/// Words become hyphens; the make is title-cased and the model keeps its case.
#[test]
fn words_are_joined_with_hyphens() {
    assert_eq!(
        repository_name("CHEVROLET", "Silverado 1500").as_deref(),
        Some("Chevrolet-Silverado-1500")
    );
    assert_eq!(
        repository_name("LAND ROVER", "Range  Rover Sport").as_deref(),
        Some("Land-Rover-Range-Rover-Sport")
    );
    assert_eq!(
        repository_name("MERCEDES-BENZ", "GLC-Class").as_deref(),
        Some("Mercedes-Benz-GLC-Class")
    );
}

/// No make or no model is no name, never a half-name.
#[test]
fn a_missing_half_is_no_name() {
    assert_eq!(repository_name("", "F-150"), None);
    assert_eq!(repository_name("FORD", "  "), None);
    assert_eq!(repository_name("MAZDA", "Mazda"), None);
}

#[test]
fn urls_point_at_the_v3_signal_set_and_the_repository() {
    assert_eq!(
        signalset_url("Mazda-3"),
        "https://raw.githubusercontent.com/OBDb/Mazda-3/main/signalsets/v3/default.json"
    );
    assert_eq!(repository_url("Mazda-3"), "https://github.com/OBDb/Mazda-3");
}

/// Only a whole make is removed from the front of a model, never part of a word.
#[test]
fn a_model_that_merely_begins_like_the_make_keeps_its_name() {
    assert_eq!(repository_name("MINI", "Minivan").as_deref(), Some("Mini-Minivan"));
    assert_eq!(repository_name("RAM", "Rampage").as_deref(), Some("Ram-Rampage"));
}

/// Whatever vPIC returns, a name comes back or nothing does; it never panics.
#[test]
fn untidy_or_non_ascii_input_is_handled() {
    assert_eq!(repository_name("LAND  ROVER", "Defender").as_deref(), Some("Land-Rover-Defender"));
    assert_eq!(repository_name("ŠKODA", "Octavia").as_deref(), Some("Škoda-Octavia"));
    assert_eq!(repository_name("FORD", "Fé").as_deref(), Some("Ford-Fé"));
}
