//! Charging data sources.
//!
//! The adapter never talks to the kernel directly. It asks a backend for a
//! snapshot and the backend decides where the numbers come from:
//!
//! * [`micharge`] proxies the Xiaomi `IMiCharge` vendor HAL over live AIDL,
//!   which already absorbs the per-model sysfs layout differences.
//! * [`hidl`] serves the Xiaomi HIDL 1.0 generation by reading that
//!   generation's own node map directly. It is explicitly not an HIDL RPC
//!   client (see its module docs for why that road is closed); the vendor
//!   implementation returns those same node contents verbatim, so the map is
//!   the honest data path here.
//! * [`sysfs`] reads kernel nodes directly. It is the fallback used when
//!   neither vendor generation is detectable.
//!
//! Selection order is live AIDL first, HIDL generation second, generic nodes
//! last. Only the AIDL probe touches binder; the HIDL-generation check is a
//! pair of manifest file probes, so a non-Xiaomi device pays two failed
//! `stat` calls and nothing else.

pub mod hidl;
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
/// Order: live AIDL vendor HAL first; when that is unreachable but the device
/// declares the Xiaomi HIDL 1.0 generation, the HIDL-generation node backend;
/// otherwise the generic kernel-node reader so charging still reports data.
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
                tracing::warn!("Xiaomi MiCharge HAL unavailable ({error}); probing generation");
            }
        }
        if select_kind(
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
    /// Live AIDL vendor HAL proxy.
    Aidl,
    /// Xiaomi HIDL-generation direct-node reader.
    HidlNodes,
    /// Generic kernel-node reader.
    Sysfs,
}

/// Selection rule: a reachable AIDL service always wins; otherwise the HIDL
/// fragment (without the AIDL fragment) routes to the generation backend;
/// everything else falls through to the generic reader.
pub fn select_kind(aidl_live: bool, hidl_manifest: bool, aidl_manifest: bool) -> BackendKind {
    if aidl_live {
        BackendKind::Aidl
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
        assert_eq!(select_kind(true, false, false), BackendKind::Aidl);
        assert_eq!(select_kind(true, true, false), BackendKind::Aidl);
        assert_eq!(select_kind(false, true, false), BackendKind::HidlNodes);
        assert_eq!(select_kind(false, true, true), BackendKind::Sysfs);
        assert_eq!(select_kind(false, false, false), BackendKind::Sysfs);
        assert_eq!(select_kind(false, false, true), BackendKind::Sysfs);
    }
}
