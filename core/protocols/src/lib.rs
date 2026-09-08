//! `aim-protocols` — framing and diagnostic services.
//!
//! Handoff §5 layering, one module per row of the responsibilities table:
//!
//! ```text
//!   can    frames, arbitration ids, OBD addressing
//!   isotp  segmentation / reassembly with flow control
//!   obd2   SAE J1979 services 01,02,03,04,07,09,0A
//!   uds    ISO 14229 request/negative-response semantics (scaffold)
//! ```
//!
//! Nothing in this crate performs I/O and nothing here knows what a value
//! *means* — `obd2` will tell you that bytes `41 0C 1A F8` are a positive
//! response to service 01 PID 0C carrying payload `1A F8`. Turning `1A F8`
//! into "1726 rpm" is `aim-decoders`' job.

#![warn(missing_docs)]

pub mod can;
pub mod isotp;
pub mod obd2;
pub mod uds;

pub use can::{CanFrame, CanId, OBD_FUNCTIONAL_REQUEST_ID};
pub use isotp::{IsoTpFrame, IsoTpReceiver, IsoTpSender, ReceiveOutcome};
pub use obd2::{
    decode_ascii_records, decode_dtc, decode_dtc_list, decode_monitor_response,
    decode_monitor_tests, decode_supported_pids, decode_vin, encode_dtc, encode_supported_pids,
    strip_dtc_count, MonitorTest, ObdRequest, ObdResponse, Service,
};
pub use uds::{
    decode_dtc_by_status_mask, NegativeResponseCode, UdsDtc, UdsRequest, UdsResponse, UdsService,
};
