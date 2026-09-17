//! Asking NHTSA vPIC about the connected vehicle, on request only (#55).
//!
//! The service never fetches anything itself. It says what to ask
//! ([`DiagnosticService::vpic_request`]), whoever was asked to look the vehicle
//! up hands the reply back ([`DiagnosticService::record_vpic_reply`]), and from
//! then on the cached reply is ranked into the vehicle's identity on every visit.

use crate::identity::VehicleIdentity;
use crate::service::DiagnosticService;
use aim_decoders::{parse_decode_vin_values, vpic::decode_vin_values_url, VpicDecode};
use aim_session::{SessionStore, StoredVpicReply};
use aim_types::{AimError, AimResult, ErrorCode};

impl DiagnosticService {
    /// The vPIC request for the connected vehicle's VIN.
    ///
    /// Fails when no single VIN is established: there is nothing to ask about.
    pub fn vpic_request(&self) -> AimResult<String> {
        let identity = self.identity();
        let vin = identity.settled("vin").ok_or_else(|| {
            AimError::new(
                ErrorCode::PreconditionFailed,
                "no single VIN has been established for the connected vehicle, so there is nothing to look up".to_string(),
            )
        })?;
        decode_vin_values_url(vin)
    }

    /// The vPIC reply already held for the connected vehicle, when there is one.
    pub fn cached_vpic(&self) -> Option<StoredVpicReply> {
        let identity = self.identity();
        let vin = identity.settled("vin")?;
        self.store().vpic_reply(vin).ok().flatten()
    }

    /// Keep a vPIC reply for the connected vehicle and return what it decodes to.
    ///
    /// A reply that does not parse is refused and nothing is kept.
    pub fn record_vpic_reply(&mut self, body: &str, source_url: &str) -> AimResult<VpicDecode> {
        let identity = self.identity();
        let vin = identity.settled("vin").ok_or_else(|| {
            AimError::new(
                ErrorCode::PreconditionFailed,
                "no single VIN has been established for the connected vehicle, so there is nothing to keep the reply against".to_string(),
            )
        })?.to_string();
        let decode = parse_decode_vin_values(body)?;
        self.store().store_vpic_reply(&vin, source_url, body)?;
        Ok(decode)
    }
}

/// Rank the vPIC reply cached for the identity's VIN into the identity.
///
/// Silent when there is no settled VIN, no cached reply, or a reply that no
/// longer parses: a lookup adds evidence and never takes any away.
pub(crate) fn merge_cached_vpic(identity: &mut VehicleIdentity, store: &SessionStore) {
    let Some(vin) = identity.settled("vin").map(str::to_string) else { return };
    if let Ok(Some(reply)) = store.vpic_reply(&vin) {
        if let Ok(decode) = parse_decode_vin_values(&reply.body) {
            identity.record_vpic(&decode);
        }
    }
}
