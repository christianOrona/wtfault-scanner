//! Links that only the host shell can reach.
//!
//! On a desktop an adapter is a serial port, and [`crate::list_ports`] finds it
//! through the OS. A phone has no serial ports: on Android a paired Bluetooth
//! ELM327 is reached through the platform's own Bluetooth API, which only the
//! app shell can call. The shell installs a [`PlatformLinks`] provider once at
//! startup, and from then on its links are listed beside serial ports and
//! opened by name like any other — the rest of the core never learns the
//! difference.
//!
//! A provider owns a namespace of names (Android uses `bt:<address>`), so a
//! name it does not recognise is never mistaken for one it does.

use std::sync::OnceLock;

use aim_types::AimResult;

use crate::{PortInfo, Transport};

/// A source of adapter links the OS does not expose as serial ports.
pub trait PlatformLinks: Send + Sync {
    /// The links available right now, already classified.
    fn list(&self) -> Vec<PortInfo>;

    /// Whether `name` belongs to this provider.
    fn owns(&self, name: &str) -> bool;

    /// A transport for `name`, not yet opened.
    fn transport(&self, name: &str) -> AimResult<Box<dyn Transport>>;
}

/// Holds at most one provider. A process only has one shell.
pub struct Registry {
    provider: OnceLock<Box<dyn PlatformLinks>>,
}

impl Registry {
    /// An empty registry.
    pub const fn new() -> Self {
        Self { provider: OnceLock::new() }
    }

    /// Install `provider`. Returns false, keeping the first, when one is
    /// already installed.
    pub fn install(&self, provider: Box<dyn PlatformLinks>) -> bool {
        self.provider.set(provider).is_ok()
    }

    /// The provider's links, or none.
    pub fn list(&self) -> Vec<PortInfo> {
        self.provider.get().map(|p| p.list()).unwrap_or_default()
    }

    /// A transport for `name` when the provider owns it; `None` when nothing
    /// installed claims the name, so the caller falls back to a serial port.
    pub fn transport(&self, name: &str) -> Option<AimResult<Box<dyn Transport>>> {
        let provider = self.provider.get()?;
        provider.owns(name).then(|| provider.transport(name))
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL: Registry = Registry::new();

/// Install the process's provider. See [`Registry::install`].
pub fn install(provider: Box<dyn PlatformLinks>) -> bool {
    GLOBAL.install(provider)
}

/// The installed provider's links. See [`Registry::list`].
pub fn list() -> Vec<PortInfo> {
    GLOBAL.list()
}

/// A transport from the installed provider. See [`Registry::transport`].
pub fn transport(name: &str) -> Option<AimResult<Box<dyn Transport>>> {
    GLOBAL.transport(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LoopbackTransport, PortKind};

    struct Fake;

    impl PlatformLinks for Fake {
        fn list(&self) -> Vec<PortInfo> {
            vec![PortInfo {
                name: "bt:00:11:22:33:44:55".into(),
                kind: PortKind::Bluetooth,
                vid: None,
                pid: None,
                serial_number: None,
                manufacturer: None,
                product: Some("OBDII".into()),
                likely_obd_adapter: true,
                unusable_because: None,
            }]
        }

        fn owns(&self, name: &str) -> bool {
            name.starts_with("bt:")
        }

        fn transport(&self, _name: &str) -> AimResult<Box<dyn Transport>> {
            Ok(Box::new(LoopbackTransport::new(vec![("ATZ", vec!["ELM327 v1.5"])])))
        }
    }

    #[test]
    fn an_empty_registry_lists_nothing_and_claims_nothing() {
        let r = Registry::new();
        assert!(r.list().is_empty());
        assert!(r.transport("bt:00:11:22:33:44:55").is_none());
    }

    #[test]
    fn an_installed_provider_lists_and_opens_its_own_names() {
        let r = Registry::new();
        assert!(r.install(Box::new(Fake)));
        assert_eq!(r.list()[0].name, "bt:00:11:22:33:44:55");
        assert!(r.transport("bt:00:11:22:33:44:55").unwrap().is_ok());
    }

    #[test]
    fn a_name_the_provider_does_not_own_falls_through() {
        let r = Registry::new();
        r.install(Box::new(Fake));
        // COM5 is a serial port; the provider must not swallow it.
        assert!(r.transport("COM5").is_none());
    }

    #[test]
    fn the_first_provider_stays_installed() {
        let r = Registry::new();
        assert!(r.install(Box::new(Fake)));
        assert!(!r.install(Box::new(Fake)));
    }
}
