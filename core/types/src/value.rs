//! Decoded values.
//!
//! A [`DecodedValue`] is the only thing that crosses out of the decoding layer.
//! It always pairs meaning (name, unit, range) with origin ([`Provenance`]).

use crate::{Provenance, Timestamp};
use serde::{Deserialize, Serialize};

/// The value payload after decoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// A scalar measurement.
    Number(f64),
    /// An integer count or enumeration index.
    Integer(i64),
    /// A boolean flag.
    Boolean(bool),
    /// Text (VIN, calibration id, enumeration label).
    Text(String),
    /// A set of named boolean flags decoded from a bitfield.
    Flags(Vec<Flag>),
    /// Bytes we deliberately did not interpret.
    Raw(String),
}

impl Value {
    /// Numeric view of the value, when one exists.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Integer(i) => Some(*i as f64),
            Value::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    /// Text view of the value, when one exists.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) | Value::Raw(s) => Some(s),
            _ => None,
        }
    }
}

/// One named bit out of a bitfield decode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    /// Stable flag identifier from the definition file.
    pub id: String,
    /// Human-readable label from the definition file.
    pub label: String,
    /// Whether the bit was set.
    pub set: bool,
}

/// Declared plausible range for a signal, from its definition file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ValidRange {
    /// Inclusive lower bound.
    pub min: f64,
    /// Inclusive upper bound.
    pub max: f64,
}

impl ValidRange {
    /// True when `v` lies inside the declared bounds.
    pub fn contains(&self, v: f64) -> bool {
        v >= self.min && v <= self.max
    }
}

/// A decoded signal reading with full provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedValue {
    /// Stable signal identifier, e.g. `engine_rpm`.
    pub signal_id: String,
    /// Human-readable name from the definition file.
    pub name: String,
    /// The decoded payload.
    pub value: Value,
    /// Unit symbol, e.g. `rpm`, `°C`, `%`. `None` for unitless/text values.
    pub unit: Option<String>,
    /// Declared plausible range, when the definition provides one.
    pub valid_range: Option<ValidRange>,
    /// True when the decoded number fell outside `valid_range`.
    pub out_of_range: bool,
    /// Where these bytes and this interpretation came from.
    pub provenance: Provenance,
    /// When the value was produced.
    pub timestamp: Timestamp,
}

impl DecodedValue {
    /// Build a value and evaluate its range check in one step.
    pub fn new(
        signal_id: impl Into<String>,
        name: impl Into<String>,
        value: Value,
        unit: Option<String>,
        valid_range: Option<ValidRange>,
        provenance: Provenance,
    ) -> Self {
        let out_of_range = match (&value.as_f64(), &valid_range) {
            (Some(v), Some(r)) => !r.contains(*v),
            _ => false,
        };
        let timestamp = provenance.observed_at;
        DecodedValue {
            signal_id: signal_id.into(),
            name: name.into(),
            value,
            unit,
            valid_range,
            out_of_range,
            provenance,
            timestamp,
        }
    }

    /// True when this reading may be presented to a user as fact: the decoder
    /// definition is verified *and* the value is inside its declared range.
    pub fn is_trustworthy(&self) -> bool {
        self.provenance.verification.is_trustworthy() && !self.out_of_range
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::VerificationStatus;

    fn prov(status: VerificationStatus) -> Provenance {
        Provenance::decoded(&[0x41, 0x05, 0x5a], "obd2.mode01.pid05", "1", status, crate::now())
    }

    #[test]
    fn range_check_runs_at_construction() {
        let v = DecodedValue::new(
            "coolant_temp",
            "Engine coolant temperature",
            Value::Number(50.0),
            Some("°C".into()),
            Some(ValidRange { min: -40.0, max: 215.0 }),
            prov(VerificationStatus::Verified),
        );
        assert!(!v.out_of_range);
        assert!(v.is_trustworthy());

        let v = DecodedValue::new(
            "coolant_temp",
            "Engine coolant temperature",
            Value::Number(900.0),
            Some("°C".into()),
            Some(ValidRange { min: -40.0, max: 215.0 }),
            prov(VerificationStatus::Verified),
        );
        assert!(v.out_of_range);
        assert!(!v.is_trustworthy());
    }

    #[test]
    fn unverified_decoders_never_produce_trustworthy_values() {
        let v = DecodedValue::new(
            "dpf_temp",
            "DPF temperature",
            Value::Number(300.0),
            Some("°C".into()),
            None,
            prov(VerificationStatus::Unverified),
        );
        assert!(!v.out_of_range);
        assert!(!v.is_trustworthy());
    }

    #[test]
    fn text_values_are_never_out_of_range() {
        let v = DecodedValue::new(
            "vin",
            "Vehicle Identification Number",
            Value::Text("1FT7W2BT6KEC00001".into()),
            None,
            Some(ValidRange { min: 0.0, max: 1.0 }),
            prov(VerificationStatus::Verified),
        );
        assert!(!v.out_of_range);
    }
}
