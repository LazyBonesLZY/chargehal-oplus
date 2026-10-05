//! Charging data sources.
//!
//! The adapter never talks to the kernel directly. It asks a backend for a
//! snapshot and the backend decides where the numbers come from:
//!
//! * [`micharge`] proxies the Xiaomi `IMiCharge` vendor HAL over live AIDL,
//!   which already absorbs the per-model sysfs layout differences.
//! * [`lenovo`] proxies the Lenovo `IBattery` vendor HAL the same way. It is a
//!   different vendor and a different interface, but also a plain AIDL NDK
//!   service, so the same bridge shape applies.
//! * [`hidl`] serves the Xiaomi HIDL 1.0 generation by reading that
//!   generation's own node map directly. It is explicitly not an HIDL RPC
//!   client (see its module docs for why that road is closed); the vendor
//!   implementation returns those same node contents verbatim, so the map is
//!   the honest data path here.
//! * [`sysfs`] reads kernel nodes directly. It is the fallback used when
//!   none of the vendor generations is detectable.
//!
//! Selection order is live AIDL first (Xiaomi, then Lenovo), the Xiaomi HIDL
//! generation second, generic nodes last. Only the AIDL probes touch binder;
//! the HIDL-generation check is a pair of manifest file probes, so a device
//! with neither vendor pays two failed `stat` calls and nothing else.

pub mod hidl;
pub mod lenovo;
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

    /// Apply the charge-limit percentage, or clear it with `None`.
    ///
    /// Only backends whose vendor HAL owns a real limit control implement this;
    /// the default is a no-op so the others keep using [`set_charge_control`].
    fn set_charge_limit(&self, _limit_percent: Option<i32>) {}

    /// Enable or disable bypass charging.
    ///
    /// Same rule as [`set_charge_limit`]: only implemented where the vendor HAL
    /// exposes the control.
    fn set_bypass_charge(&self, _enabled: bool) {}

    /// Charge-limit percentage as the vendor HAL currently reports it.
    ///
    /// `None` when this backend has no limit control, in which case callers fall
    /// back to the value they last wrote.
    fn charge_limit_percent(&self) -> Option<i32> {
        None
    }

    /// Bypass state as the vendor HAL currently reports it. `None` as above.
    fn bypass_charge_enabled(&self) -> Option<bool> {
        None
    }

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
/// Order: a live vendor AIDL HAL first (Xiaomi, then Lenovo); when neither is
/// reachable but the device declares the Xiaomi HIDL 1.0 generation, the
/// HIDL-generation node backend; otherwise the generic kernel-node reader so
/// charging still reports data.
pub fn select() -> Arc<dyn ChargeBackend> {
    // The vendor HALs only exist on device. On the host (unit tests) there is no
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
                tracing::warn!("Xiaomi MiCharge HAL unavailable ({error}); probing generation");
            }
        }
        match lenovo::LenovoBackend::connect() {
            Ok(backend) => {
                tracing::info!("charging backend: {} (Lenovo vendor HAL)", backend.name());
                return Arc::new(backend);
            }
            Err(error) => {
                tracing::warn!("Lenovo battery HAL unavailable ({error}); probing generation");
            }
        }
        if select_kind(
            false,
            false,
            hidl::manifest_present(hidl::HIDL_MANIFEST),
            hidl::manifest_present(hidl::AIDL_MANIFEST),
        ) == BackendKind::HidlNodes
        {
            let backend = hidl::HidlBackend::new();
            tracing::info!(
                "charging backend: {} (Xiaomi HIDL-generation nodes)",
                backend.name()
            );
            return Arc::new(backend);
        }
    }

    Arc::new(sysfs::SysfsBackend::new())
}

/// Backend chosen by [`select_kind`]. Kept as a plain enum so the selection
/// rule stays unit-testable without touching binder or the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Live Xiaomi AIDL vendor HAL proxy.
    XiaomiAidl,
    /// Live Lenovo AIDL vendor HAL proxy.
    LenovoAidl,
    /// Xiaomi HIDL-generation direct-node reader.
    HidlNodes,
    /// Generic kernel-node reader.
    Sysfs,
}

/// Selection rule: a reachable vendor AIDL service always wins (Xiaomi first,
/// then Lenovo); otherwise the Xiaomi HIDL fragment (without the AIDL fragment)
/// routes to the generation backend; everything else falls through to the
/// generic reader.
pub fn select_kind(
    xiaomi_aidl_live: bool,
    lenovo_aidl_live: bool,
    hidl_manifest: bool,
    aidl_manifest: bool,
) -> BackendKind {
    if xiaomi_aidl_live {
        BackendKind::XiaomiAidl
    } else if lenovo_aidl_live {
        BackendKind::LenovoAidl
    } else if hidl_manifest && !aidl_manifest {
        BackendKind::HidlNodes
    } else {
        BackendKind::Sysfs
    }
}

#[cfg(test)]
mod tests {
    use super::{select_kind, BackendKind};

    #[test]
    fn selection_prefers_live_aidl_then_hidl_generation() {
        assert_eq!(
            select_kind(true, false, false, false),
            BackendKind::XiaomiAidl
        );
        assert_eq!(
            select_kind(true, true, false, false),
            BackendKind::XiaomiAidl
        );
        assert_eq!(
            select_kind(true, true, true, false),
            BackendKind::XiaomiAidl
        );
        assert_eq!(
            select_kind(false, true, false, false),
            BackendKind::LenovoAidl
        );
        assert_eq!(
            select_kind(false, true, true, false),
            BackendKind::LenovoAidl
        );
        assert_eq!(
            select_kind(false, false, true, false),
            BackendKind::HidlNodes
        );
        assert_eq!(select_kind(false, false, true, true), BackendKind::Sysfs);
        assert_eq!(select_kind(false, false, false, false), BackendKind::Sysfs);
        assert_eq!(select_kind(false, false, false, true), BackendKind::Sysfs);
    }
}
