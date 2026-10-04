//! Charging data sources.
//!
//! The adapter never talks to the kernel directly. It asks a backend for a
//! snapshot and the backend decides where the numbers come from:
//!
//! * [`micharge`] proxies the Xiaomi `IMiCharge` vendor HAL, which already
//!   absorbs the per-model sysfs layout differences.
//! * [`sysfs`] reads kernel nodes directly. It is the fallback used when the
//!   vendor HAL is missing or has died.
//!
//! The vendor HAL is preferred: node names, units and permissions differ
//! between kernel generations, and the HAL hides that. The node reader stays
//! available so a device without the HAL still reports sane charging data.

pub mod micharge;
pub mod sysfs;

use std::sync::Arc;

use crate::adapter::ChargerInfo;

/// A source of charging data.
pub trait ChargeBackend: Send + Sync {
    /// Stable identifier used in logs.
    fn name(&self) -> &'static str;

    /// Whether the backend can currently serve a snapshot. The adapter
    /// re-probes before a scan when this reports false.
    fn is_available(&self) -> bool;

    /// Fill `info` from the backend.
    ///
    /// Returns `false` when the scan was cancelled because the screen is
    /// transitioning. The adapter then re-queues the refresh instead of
    /// publishing a half-read snapshot.
    fn refresh(&self, info: &mut ChargerInfo, should_cancel: &dyn Fn() -> bool) -> bool;

    /// Restrict charging (`true`) or release it (`false`).
    fn set_charge_control(&self, restrict: bool);

    /// Cheap power-source probe, used to decide whether a power-supply uevent
    /// has to escalate to a full scan.
    ///
    /// The node reader answers this from a handful of files, so a charger that
    /// is merely plugged and left alone does not trigger full scans. The HAL
    /// backend asks the vendor HAL directly. Returning `true` is always safe;
    /// it only costs a full scan.
    fn power_source_changed(&self) -> bool;
}

/// Probe the available backends and return the preferred one.
///
/// The Xiaomi vendor HAL wins when it is reachable; otherwise the kernel-node
/// reader takes over so that charging still reports sane data.
pub fn select() -> Arc<dyn ChargeBackend> {
    // The vendor HAL only exists on device. On the host (unit tests) there is no
    // servicemanager to talk to, so go straight to the node reader instead of
    // tripping over binder process-state initialization.
    #[cfg(target_os = "android")]
    {
        match micharge::MiChargeBackend::connect() {
            Ok(backend) => {
                tracing::info!("charging backend: {} (Xiaomi vendor HAL)", backend.name());
                return Arc::new(backend);
            }
            Err(error) => {
                tracing::warn!("Xiaomi MiCharge HAL unavailable ({error}); falling back to sysfs");
            }
        }
    }

    Arc::new(sysfs::SysfsBackend::new())
}
