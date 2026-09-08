//! What changed between two visits to the same vehicle.
//!
//! Every scan this app has ever done is already on disk. Nothing compared them,
//! and that left the most useful diagnostic question unanswerable: *is this
//! getting worse?*
//!
//! A single scan can only ever say what is true right now. Long-term fuel trim
//! reading 9% is mildly interesting on its own and is a diagnosis when you can
//! see it was 2% in March — something has been developing for six months. A
//! catalyst self-test at 88% of its limit means little until you know it was at
//! 61% last year. No amount of cleverness in a single reading recovers that;
//! only a second reading does.
//!
//! Two rules, both about not overstating what a comparison shows:
//!
//! * **A signal read once in each session is not a trend.** It is two readings.
//!   The comparison says how much they differ and leaves the conclusion alone.
//! * **Conditions are not controlled.** A cold engine and a warm one produce
//!   very different numbers for honest reasons, so a difference is reported
//!   with whatever context is available and never as a verdict.

use crate::store::SessionStore;
use aim_types::{AimResult, DtcRecord, SessionId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a fault differs between two scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultChange {
    /// Present in the later scan and not in the earlier one.
    Appeared,
    /// Present in the earlier scan and not in the later one.
    ///
    /// Worth distinguishing from "fixed": a code also disappears when somebody
    /// clears it, and this cannot tell the difference. The readiness monitors
    /// can, which is why they are reported alongside.
    Gone,
    /// In both.
    Unchanged,
}

/// One fault, and what happened to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaultDelta {
    /// SAE code.
    pub code: String,
    /// Module that reported it, in whichever scan had it.
    pub module: String,
    /// Description carried over from the record.
    pub description: Option<String>,
    /// What changed.
    pub change: FaultChange,
}

/// One signal read in both scans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalDelta {
    /// Signal identifier.
    pub signal_id: String,
    /// Unit, when the readings carry one.
    pub unit: Option<String>,
    /// Mean value in the earlier scan.
    pub before: f64,
    /// Mean value in the later scan.
    pub after: f64,
    /// Absolute change.
    pub delta: f64,
    /// Change as a fraction of the earlier value. `None` when the earlier value
    /// was zero, because a percentage of nothing is not a number.
    pub relative: Option<f64>,
    /// How many readings each mean was taken over, so a single-sample
    /// comparison is never mistaken for a measured average.
    pub samples_before: usize,
    /// Readings behind the later mean.
    pub samples_after: usize,
}

impl SignalDelta {
    /// Whether this is worth a person's attention.
    ///
    /// A threshold, not a diagnosis. Sensor noise, a warmer engine and a
    /// different altitude all move readings without anything being wrong, so
    /// this only decides what gets listed first.
    pub fn notable(&self) -> bool {
        match self.relative {
            Some(r) => r.abs() >= 0.20,
            // No baseline to be relative to; any movement from zero is worth
            // seeing.
            None => self.delta.abs() > 0.0,
        }
    }
}

/// The whole comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionComparison {
    /// The earlier session.
    pub before: SessionId,
    /// The later session.
    pub after: SessionId,
    /// When the earlier one started.
    pub before_at: Timestamp,
    /// When the later one started.
    pub after_at: Timestamp,
    /// Days between them, for context on how much drift to expect.
    pub days_apart: f64,
    /// Faults that appeared, went, or stayed.
    pub faults: Vec<FaultDelta>,
    /// Signals present in both, most-changed first.
    pub signals: Vec<SignalDelta>,
    /// Whether the two sessions are the same vehicle, when both recorded a VIN.
    ///
    /// `None` when either scan did not read one. Comparing two different
    /// vehicles produces confident nonsense, so this is checked rather than
    /// assumed.
    pub same_vehicle: Option<bool>,
}

impl SessionComparison {
    /// Faults that were not there last time.
    pub fn appeared(&self) -> impl Iterator<Item = &FaultDelta> {
        self.faults.iter().filter(|f| f.change == FaultChange::Appeared)
    }

    /// Faults that are no longer reported.
    pub fn gone(&self) -> impl Iterator<Item = &FaultDelta> {
        self.faults.iter().filter(|f| f.change == FaultChange::Gone)
    }
}

/// Compare two recorded sessions.
pub fn compare_sessions(
    store: &SessionStore,
    before: &SessionId,
    after: &SessionId,
) -> AimResult<SessionComparison> {
    let a = store.get_session(before)?;
    let b = store.get_session(after)?;

    let same_vehicle = match (&a.vehicle_id, &b.vehicle_id) {
        (Some(x), Some(y)) => Some(x == y),
        _ => None,
    };

    let faults = compare_faults(&store.dtcs(before, None)?, &store.dtcs(after, None)?);

    let mut signals = compare_signals(
        &means(&store.measurements(before, None, 5_000)?),
        &means(&store.measurements(after, None, 5_000)?),
    );
    // Biggest relative movement first: the point of the screen is "what is
    // different", not "here is everything again".
    signals.sort_by(|x, y| {
        y.relative
            .unwrap_or(f64::MAX)
            .abs()
            .partial_cmp(&x.relative.unwrap_or(f64::MAX).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let days_apart =
        (b.started_at.unix_millis() - a.started_at.unix_millis()) as f64 / 86_400_000.0;

    Ok(SessionComparison {
        before: before.clone(),
        after: after.clone(),
        before_at: a.started_at,
        after_at: b.started_at,
        days_apart,
        faults,
        signals,
        same_vehicle,
    })
}

fn compare_faults(before: &[DtcRecord], after: &[DtcRecord]) -> Vec<FaultDelta> {
    let key = |d: &DtcRecord| d.code.clone();
    let old: BTreeMap<String, &DtcRecord> = before.iter().map(|d| (key(d), d)).collect();
    let new: BTreeMap<String, &DtcRecord> = after.iter().map(|d| (key(d), d)).collect();

    let mut out = Vec::new();
    for (code, d) in &new {
        out.push(FaultDelta {
            code: code.clone(),
            module: d.module_id.as_str().to_string(),
            description: d.description.clone(),
            change: if old.contains_key(code) {
                FaultChange::Unchanged
            } else {
                FaultChange::Appeared
            },
        });
    }
    for (code, d) in &old {
        if !new.contains_key(code) {
            out.push(FaultDelta {
                code: code.clone(),
                module: d.module_id.as_str().to_string(),
                description: d.description.clone(),
                change: FaultChange::Gone,
            });
        }
    }
    out
}

/// Mean value per signal, with the sample count and unit kept.
fn means(rows: &[aim_types::Measurement]) -> BTreeMap<String, (f64, usize, Option<String>)> {
    let mut acc: BTreeMap<String, (f64, usize, Option<String>)> = BTreeMap::new();
    for m in rows {
        let Some(v) = m.value else { continue };
        let e = acc.entry(m.signal_id.clone()).or_insert((0.0, 0, m.unit.clone()));
        e.0 += v;
        e.1 += 1;
    }
    for e in acc.values_mut() {
        if e.1 > 0 {
            e.0 /= e.1 as f64;
        }
    }
    acc
}

fn compare_signals(
    before: &BTreeMap<String, (f64, usize, Option<String>)>,
    after: &BTreeMap<String, (f64, usize, Option<String>)>,
) -> Vec<SignalDelta> {
    before
        .iter()
        .filter_map(|(id, (b, nb, unit))| {
            // Only signals present in BOTH. A reading taken once has nothing to
            // be compared against, and listing it as "changed" would be a lie.
            let (a, na, _) = after.get(id)?;
            Some(SignalDelta {
                signal_id: id.clone(),
                unit: unit.clone(),
                before: *b,
                after: *a,
                delta: a - b,
                relative: if *b == 0.0 { None } else { Some((a - b) / b.abs()) },
                samples_before: *nb,
                samples_after: *na,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_types::{DtcStatus, ModuleId, Value};

    fn dtc(code: &str, module: &str) -> DtcRecord {
        DtcRecord {
            session_id: SessionId("s".into()),
            module_id: ModuleId(module.into()),
            code: code.into(),
            status: DtcStatus::Confirmed,
            description: Some(format!("description of {code}")),
            occurrence: 1,
            freeze_frame_ref: None,
            read_at: aim_types::now(),
        }
    }

    #[test]
    fn a_new_fault_is_distinguished_from_one_that_was_already_there() {
        let before = vec![dtc("P0420", "m1")];
        let after = vec![dtc("P0420", "m1"), dtc("P0301", "m1")];
        let d = compare_faults(&before, &after);

        let new = d.iter().find(|f| f.code == "P0301").unwrap();
        assert_eq!(new.change, FaultChange::Appeared);
        let old = d.iter().find(|f| f.code == "P0420").unwrap();
        assert_eq!(old.change, FaultChange::Unchanged);
    }

    #[test]
    fn a_fault_that_stopped_being_reported_is_gone_not_fixed() {
        // The wording matters. A code disappears when it is repaired and when
        // somebody clears it, and this comparison cannot tell those apart.
        let d = compare_faults(&[dtc("P0420", "m1")], &[]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change, FaultChange::Gone);
    }

    #[test]
    fn only_signals_present_in_both_scans_are_compared() {
        let mut before = BTreeMap::new();
        before.insert("a".to_string(), (10.0, 3, Some("%".into())));
        before.insert("only_before".to_string(), (1.0, 1, None));
        let mut after = BTreeMap::new();
        after.insert("a".to_string(), (12.0, 4, Some("%".into())));
        after.insert("only_after".to_string(), (5.0, 1, None));

        let d = compare_signals(&before, &after);
        assert_eq!(d.len(), 1, "a reading with no counterpart cannot have changed");
        assert_eq!(d[0].signal_id, "a");
        assert_eq!(d[0].delta, 2.0);
        assert!((d[0].relative.unwrap() - 0.2).abs() < 1e-9);
        assert_eq!(d[0].samples_before, 3);
        assert_eq!(d[0].samples_after, 4);
    }

    #[test]
    fn a_change_from_zero_has_no_percentage_rather_than_infinity() {
        let mut before = BTreeMap::new();
        before.insert("a".to_string(), (0.0, 1, None));
        let mut after = BTreeMap::new();
        after.insert("a".to_string(), (4.0, 1, None));

        let d = compare_signals(&before, &after);
        assert_eq!(d[0].relative, None);
        assert!(d[0].notable(), "movement off zero is still worth showing");
    }

    #[test]
    fn a_negative_baseline_still_yields_a_sensible_percentage() {
        // Fuel trims are routinely negative. Dividing by the signed value would
        // flip the sign of the change and report a worsening trim as an
        // improvement.
        let mut before = BTreeMap::new();
        before.insert("trim".to_string(), (-10.0, 2, Some("%".into())));
        let mut after = BTreeMap::new();
        after.insert("trim".to_string(), (-12.0, 2, Some("%".into())));

        let d = compare_signals(&before, &after);
        assert_eq!(d[0].delta, -2.0);
        assert!(
            d[0].relative.unwrap() < 0.0,
            "trim moved further negative, so the relative change must be negative"
        );
    }

    #[test]
    fn small_movement_is_not_flagged_as_notable() {
        let mut before = BTreeMap::new();
        before.insert("a".to_string(), (100.0, 5, None));
        let mut after = BTreeMap::new();
        after.insert("a".to_string(), (103.0, 5, None));
        assert!(!compare_signals(&before, &after)[0].notable());
    }

    #[test]
    fn means_ignore_readings_that_have_no_number() {
        let rows = vec![
            aim_types::Measurement {
                session_id: SessionId("s".into()),
                module_id: ModuleId("m".into()),
                timestamp: aim_types::now(),
                signal_id: "a".into(),
                value: Some(10.0),
                text_value: None,
                unit: Some("%".into()),
                raw_value: "0a".into(),
            },
            aim_types::Measurement {
                session_id: SessionId("s".into()),
                module_id: ModuleId("m".into()),
                timestamp: aim_types::now(),
                signal_id: "a".into(),
                value: None,
                text_value: Some("text".into()),
                unit: None,
                raw_value: "ff".into(),
            },
        ];
        let m = means(&rows);
        let (mean, n, _) = m.get("a").unwrap();
        assert_eq!(*mean, 10.0);
        assert_eq!(*n, 1, "the textual reading must not dilute the average");
        let _ = Value::Text(String::new());
    }
}
