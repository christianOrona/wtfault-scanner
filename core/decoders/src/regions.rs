//! Roughly where on a vehicle a finding lives.
//!
//! # The constraint that shapes the whole thing
//!
//! This project has part locations for no vehicle, and there is no source it
//! can ship that does. A diesel particulate filter sits in a different place on
//! an F-250 than on a Golf, and even two F-250s of different years differ. So a
//! rendering that put a glowing dot at a precise point would be inventing a
//! fact — the exact thing the rest of this application refuses to do.
//!
//! What *is* true across vehicles is coarser: an evaporative-emissions fault is
//! somewhere between the fuel tank and the engine, a wheel-speed sensor fault
//! is at a wheel, an airbag fault is in the cabin. Those hold for every vehicle
//! built, because they follow from what the part is for rather than from how
//! one manufacturer laid it out.
//!
//! So this answers **zones, not coordinates**, and the interface must say so:
//! "roughly here on a vehicle of this shape", never "your filter is at this
//! point".
//!
//! # Where the mapping comes from
//!
//! SAE J2012 assigns ranges of the generic powertrain codes to subsystems —
//! `P0030–P0064` is oxygen-sensor heaters, `P0440–P0455` is evaporative
//! emissions, and so on. Those assignments are published and apply to every
//! vehicle sold, so deriving a zone from them is reading a standard rather than
//! guessing.
//!
//! Outside that, the honest answer is nothing:
//!
//! * **Manufacturer-specific codes** (`P1xxx`, `P3xxx`, most `B`/`C`/`U`) mean
//!   whatever one manufacturer decided. No zone.
//! * **Generic codes the standard does not place**, such as an internal control
//!   module fault, are not *at* anywhere useful. No zone.
//!
//! [`Region::Unknown`] is therefore a common and correct answer, and the
//! interface shows nothing rather than a shrug.

use crate::dtc::DtcSystem;
use serde::{Deserialize, Serialize};

/// A rough area of a vehicle, true regardless of make or model.
///
/// Deliberately few and deliberately coarse. Every one of these is a place a
/// person could point at on any vehicle of any shape, which is the test a zone
/// has to pass to be worth showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// Under the bonnet: engine, its sensors, its accessories.
    EngineBay,
    /// Along the exhaust, from the manifold back.
    Exhaust,
    /// The fuel tank and the lines and vapour system around it.
    FuelSystem,
    /// The gearbox and what drives through it.
    Transmission,
    /// Inside, where the person sits.
    Cabin,
    /// At the wheels — brakes, wheel-speed sensors, suspension.
    Wheels,
    /// The battery, alternator and the harness between them.
    Electrical,
    /// Nowhere this project can honestly place.
    Unknown,
}

impl Region {
    /// Stable identifier for interfaces and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Region::EngineBay => "engine_bay",
            Region::Exhaust => "exhaust",
            Region::FuelSystem => "fuel_system",
            Region::Transmission => "transmission",
            Region::Cabin => "cabin",
            Region::Wheels => "wheels",
            Region::Electrical => "electrical",
            Region::Unknown => "unknown",
        }
    }

    /// What to call it on screen.
    pub fn label(&self) -> &'static str {
        match self {
            Region::EngineBay => "Engine bay",
            Region::Exhaust => "Exhaust",
            Region::FuelSystem => "Fuel system",
            Region::Transmission => "Transmission",
            Region::Cabin => "Cabin",
            Region::Wheels => "At the wheels",
            Region::Electrical => "Battery and charging",
            Region::Unknown => "Not placeable",
        }
    }

    /// Whether there is anywhere to point at.
    pub fn is_known(&self) -> bool {
        !matches!(self, Region::Unknown)
    }
}

/// Roughly where a trouble code's subject lives, when the standard says.
///
/// Returns [`Region::Unknown`] for everything the standard does not place,
/// which is most manufacturer-specific codes and a fair number of generic
/// ones. That is the answer, not a failure to find one.
pub fn region_for_dtc(code: &str, system: DtcSystem, is_generic: bool) -> Region {
    // A manufacturer-specific code means whatever that manufacturer decided it
    // means. There is no published subsystem to read a zone from, and inferring
    // one from the number would be guessing at another company's scheme.
    if !is_generic {
        return Region::Unknown;
    }

    match system {
        // Wheel-speed, brake and suspension faults are at a wheel or at the
        // axle, on every vehicle, because that is where the parts have to be.
        DtcSystem::Chassis => Region::Wheels,
        // Body covers seats, airbags, lighting, locks, climate. Most of it is
        // in or on the cabin; none of it is placeable more precisely than that
        // without knowing the vehicle.
        DtcSystem::Body => Region::Cabin,
        // A network fault is a conversation that did not happen. It has no
        // location — the wire runs the length of the vehicle and the fault is
        // as likely to be at either end.
        DtcSystem::Network => Region::Unknown,
        DtcSystem::Powertrain => powertrain_region(code),
    }
}

/// The SAE J2012 generic powertrain ranges, and where each lives.
///
/// Only the ranges whose subject has a place. `P0600–P06FF` is the control
/// module's own internals and `P0700` upward is reported *by* the transmission
/// controller rather than being at any one spot, so they are handled where they
/// differ from that.
fn powertrain_region(code: &str) -> Region {
    let Some(n) = code.get(1..).and_then(|d| u16::from_str_radix(d, 16).ok()) else {
        return Region::Unknown;
    };

    match n {
        // Fuel metering components — volume regulator, rail pressure, injector
        // metering. All bolted to the engine.
        0x0001..=0x00FF => Region::EngineBay,
        // Fuel and air metering: mass airflow, intake air temperature, manifold
        // pressure, throttle position. All on the engine.
        0x0100..=0x0129 => Region::EngineBay,
        // Oxygen sensors and their heaters. In the exhaust, before and after
        // the catalyst, on every vehicle that has them.
        0x0130..=0x0167 => Region::Exhaust,
        // Fuel trim and fuel system pressure, at the engine end.
        0x0170..=0x0199 => Region::EngineBay,
        // Injector circuits.
        0x0200..=0x0217 => Region::EngineBay,
        // Ignition and misfire.
        0x0300..=0x0319 => Region::EngineBay,
        // Exhaust gas recirculation and secondary air — plumbed between the
        // exhaust and the intake, and conventionally found at the engine.
        0x0400..=0x0419 => Region::EngineBay,
        // Catalyst efficiency. In the exhaust by definition.
        0x0420..=0x043F => Region::Exhaust,
        // Evaporative emissions: the tank, its vapour lines and the purge and
        // vent valves. Mostly at the back of the vehicle.
        0x0440..=0x045F => Region::FuelSystem,
        // Fuel level, fuel pump, fuel temperature. At the tank.
        0x0460..=0x046F => Region::FuelSystem,
        // Exhaust pressure and temperature sensors, and the particulate filter.
        0x0470..=0x047F => Region::Exhaust,
        // Cooling fans, thermostat, coolant temperature.
        0x0480..=0x04FF => Region::EngineBay,
        // Charging and the battery.
        0x0560..=0x0563 => Region::Electrical,
        // Vehicle speed, idle control, cruise.
        0x0500..=0x0559 => Region::EngineBay,
        // Transmission-range, ratio, torque converter and shift solenoids.
        0x0700..=0x07FF => Region::Transmission,

        // --- the second generic block, P2000 upward ---
        //
        // Added after a simulated DPF fault came back unplaced: `P2463` is a
        // particulate filter, which is in the exhaust on every vehicle that has
        // one, and the first draft of this function stopped at P07FF.
        //
        // Only the ranges whose subject is unambiguous. P2xxx is not laid out
        // as neatly as P0xxx — the block from P2430 to P244F mixes secondary
        // air with evaporative emissions — and the ranges that mix are left
        // unplaced rather than assigned to whichever of the two came to mind.
        //
        // Aftertreatment: NOx, SCR, reductant, exhaust gas temperature.
        0x2000..=0x209F => Region::Exhaust,
        // Throttle actuator, pedal position, fuel trim.
        0x2100..=0x219F => Region::EngineBay,
        // NOx and oxygen sensors and their control.
        0x2200..=0x22FF => Region::Exhaust,
        // Ignition coil primary and secondary circuits.
        0x2300..=0x239F => Region::EngineBay,
        // Evaporative leak-detection pump and vent valve, at the tank.
        0x2400..=0x2429 => Region::FuelSystem,
        // Exhaust gas temperature sensors.
        0x242A..=0x242F => Region::Exhaust,
        // Particulate filter: pressure, restriction, regeneration, soot and
        // ash accumulation.
        0x2450..=0x246F => Region::Exhaust,
        // The control module's own memory and processor. A real fault and not
        // a place: it is wherever the module is, which is not something we can
        // point at and not something anybody needs to.
        _ => Region::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases the standard actually places, which is the only reason this
    /// module is allowed to answer at all.
    #[test]
    fn the_published_ranges_place_what_the_standard_places() {
        for (code, expected) in [
            ("P0420", Region::Exhaust),    // catalyst efficiency
            ("P0131", Region::Exhaust),    // oxygen sensor
            ("P0455", Region::FuelSystem), // evaporative leak
            ("P0462", Region::FuelSystem), // fuel level sender
            ("P0301", Region::EngineBay),  // cylinder 1 misfire
            ("P0128", Region::EngineBay),  // thermostat
            ("P0715", Region::Transmission),
            ("P0562", Region::Electrical), // system voltage low
        ] {
            assert_eq!(region_for_dtc(code, DtcSystem::Powertrain, true), expected, "{code}");
        }
    }

    /// The second generic block, added after a simulated DPF fault came back
    /// unplaced. A particulate filter is in the exhaust on every vehicle that
    /// has one.
    #[test]
    fn the_p2000_block_places_aftertreatment_where_it_lives() {
        for (code, expected) in [
            ("P2463", Region::Exhaust),    // DPF soot accumulation
            ("P242F", Region::Exhaust),    // DPF ash accumulation
            ("P2002", Region::Exhaust),    // DPF efficiency
            ("P2201", Region::Exhaust),    // NOx sensor
            ("P2135", Region::EngineBay),  // throttle position correlation
            ("P2301", Region::EngineBay),  // ignition coil secondary
            ("P2401", Region::FuelSystem), // evaporative leak detection pump
        ] {
            assert_eq!(region_for_dtc(code, DtcSystem::Powertrain, true), expected, "{code}");
        }
    }

    /// P2xxx is not laid out as neatly as P0xxx: the block from P2430 to P244F
    /// mixes secondary air with evaporative emissions. Unplaced beats assigning
    /// it to whichever of the two came to mind first.
    #[test]
    fn a_mixed_range_is_left_unplaced_rather_than_guessed() {
        assert_eq!(region_for_dtc("P2440", DtcSystem::Powertrain, true), Region::Unknown);
    }

    /// A manufacturer-specific code means whatever that manufacturer decided.
    /// There is no published subsystem to read, and inferring one from the
    /// number would be guessing at another company's scheme.
    #[test]
    fn a_manufacturer_code_is_not_placed() {
        assert_eq!(region_for_dtc("P1450", DtcSystem::Powertrain, false), Region::Unknown);
        // Even though P0450 would have been placed.
        assert!(region_for_dtc("P0450", DtcSystem::Powertrain, true).is_known());
    }

    /// A network fault is a conversation that did not happen. The wire runs the
    /// length of the vehicle and the fault is as likely to be at either end, so
    /// pointing anywhere would be worse than pointing nowhere.
    #[test]
    fn a_network_fault_has_no_place() {
        assert_eq!(region_for_dtc("U0100", DtcSystem::Network, true), Region::Unknown);
    }

    /// A control module's own internal fault is a real finding and not a
    /// location. "Not placeable" is the honest answer.
    #[test]
    fn a_module_internal_fault_is_not_placeable() {
        assert_eq!(region_for_dtc("P0601", DtcSystem::Powertrain, true), Region::Unknown);
        assert!(!Region::Unknown.is_known());
    }

    /// Chassis and body are placed coarsely and correctly: brakes and wheel
    /// speed are at the wheels on every vehicle ever built, and airbags and
    /// locks are in the cabin.
    #[test]
    fn chassis_and_body_are_placed_by_what_the_parts_are_for() {
        assert_eq!(region_for_dtc("C0035", DtcSystem::Chassis, true), Region::Wheels);
        assert_eq!(region_for_dtc("B0001", DtcSystem::Body, true), Region::Cabin);
    }

    /// A malformed code is not placed rather than parsed into a wrong one.
    #[test]
    fn nonsense_is_not_placed() {
        assert_eq!(region_for_dtc("PZZZZ", DtcSystem::Powertrain, true), Region::Unknown);
        assert_eq!(region_for_dtc("P", DtcSystem::Powertrain, true), Region::Unknown);
    }
}
