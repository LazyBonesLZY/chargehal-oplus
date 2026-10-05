//! Xiaomi HIDL-generation (1.0) direct-node backend.
//!
//! Covers the older Xiaomi charging HAL generation (the `@1.0` one, as opposed
//! to the AIDL V2 generation served by [`super::micharge`]). It reads the same
//! `power_supply` / `qcom-battery` nodes that the vendor implementation reads,
//! in the same per-method mapping, and applies the same branch rule.
//!
//! What this is NOT: it does not speak the HIDL transport. That transport lives
//! behind device-only C++ proxy classes and a second binder domain which this
//! crate's AIDL-only binder stack cannot open (the process already owns exactly
//! one binder domain for the OPlus service it publishes). Reconstructing that
//! client ABI from stripped binaries, without headers and without a device to
//! test against, would be a guess dressed as a bridge — so it is deliberately
//! not attempted. The honest data path for this generation in this repo is the
//! node map below, and the vendor implementation itself returns those node
//! contents verbatim with no conversion, so nothing is lost in translation.
//!
//! # Branch rule
//!
//! The vendor implementation picks one of two node families at construction
//! from the `ro.board.platform` system property: the listed Qualcomm platforms
//! take the `qcom-battery` family, everything else takes `power_supply`. This
//! backend mirrors that choice exactly ([`qcom_branch_for_platform`]), reading
//! the selected branch only — not both — for every dual-family field.
//!
//! # Coverage and grades
//!
//! Only fields the vendor generation actually backs are read; anything without
//! a mapping keeps its previous value, exactly like a failed HAL getter does:
//!
//! * Unique standard nodes (grade A, ABI units): `battery/capacity` (%),
//!   `battery/charge_full` (µAh), `battery/cycle_count` (count),
//!   `battery/current_now` (µA), `battery/temp` (0.1 °C),
//!   `battery/voltage_now` (µV), `usb/voltage_now` (µV).
//! * Dual-family nodes, branch-selected, copied verbatim (grade C, never
//!   scaled): `authentic`, `real_type`, `quick_charge_type`, `pd_authentication`
//!   / `pd_verifed`, `fastcharge_mode` / `fastchg_mode`, `soh`, `input_suspend`,
//!   `night_charging`, `smart_batt`, `soc_decimal`(+`_rate`), car-adapter type,
//!   `power_max`.
//! * Data-port detection uses this generation's own signal: `has_dp` — the node
//!   `isDPConnected` reads — feeds `pc_port_online`, and `real_type` feeds the
//!   USB-type check. `usb_type` itself is not read because that node exists on
//!   neither generation.
//! * Deliberately NOT mapped: anything reading the reverse-charge mode node is
//!   never written into `wireless_online` (same trap as the AIDL backend: an
//!   active reverse-charge session would pose as incoming wireless power).
//! * Not mapped because `ChargerInfo` has no destination for them — not because
//!   they are missing on this generation: `getBatteryResistance` (cell
//!   resistance), `getBatteryThermaLevel` (charge-control limit level),
//!   `getPdApdoMax` (PDO/APDO index, not a wattage), `getPSValue` (reverse-pen
//!   SOC), `getBtTransferStartState`, `getTxAdapt`, `getWirelessFwStatus`,
//!   `getCoolModeState`, `isUSB32`, and the key/value accessors
//!   (`getMiChargePath`, `isFunctionSupported`, `setMiChargePath`). Each has a
//!   node in this generation's set; none has a field here.
//! * `soc_decimal` / `soc_decimal_rate` are collected but not served, for the
//!   same scale reason documented on the AIDL path.
//!
//! # Control path
//!
//! Charge restriction writes the branch `input_suspend` node with `"1"` / `"0"`
//! verbatim. Two things are worth stating plainly rather than implying:
//!
//! * The `"1"` / `"0"` value domain is inherited, not proven. `input_suspend` is
//!   a vendor-private attribute (absent from the upstream `power_supply` ABI),
//!   so its accepted values are unconfirmed. The AIDL path hands the same pair
//!   to the vendor setter; this backend matches that behaviour instead of
//!   inventing a different encoding.
//! * No `"micharge all "` prefix is added: that prefix is evidenced only on the
//!   AIDL generation's setter, and adding it here would corrupt the node this
//!   generation parses.
//!
//! `setCoolModeState` is deliberately not used even though this generation
//! implements it (unlike the AIDL generation, where it is an empty shell): its
//! value domain is equally unconfirmed, and `input_suspend` already carries the
//! adapter's restrict/release intent. Driving two unconfirmed encodings for one
//! intent would double the ways a device can reach an unexpected state.

use parking_lot::Mutex;

use crate::adapter::{ChargerInfo, FastChargeSnapshot};

use super::sysfs;
use super::ChargeBackend;

/// Vendor manifest fragment declaring the HIDL 1.0 generation of the Xiaomi
/// charging HAL. Presence of this file (without the AIDL fragment) is what
/// routes backend selection here once the live AIDL service is unreachable.
pub const HIDL_MANIFEST: &str =
    "/vendor/etc/vintf/manifest/vendor.xiaomi.hardware.micharge@1.0.xml";
/// Vendor manifest fragment declaring the AIDL generation. Takes precedence:
/// a live AIDL service always wins over this backend.
pub const AIDL_MANIFEST: &str = "/vendor/etc/vintf/manifest/vendor.xiaomi.hardware.micharge.xml";

// ── 222 node map (absolute paths; the vendor literals missing a leading `/`
//    are repaired here so reads never depend on the process working dir) ──

const PSY_BATTERY: &str = "/sys/class/power_supply/battery";
const PSY_USB: &str = "/sys/class/power_supply/usb";
const PSY_WIRELESS: &str = "/sys/class/power_supply/wireless";
const PSY_AC: &str = "/sys/class/power_supply/ac";
const PSY_DC: &str = "/sys/class/power_supply/dc";

// Unique standard nodes (grade A): same path on both branches.
const CAPACITY: &str = "/sys/class/power_supply/battery/capacity";
const CHARGE_FULL: &str = "/sys/class/power_supply/battery/charge_full";
const CYCLE_COUNT: &str = "/sys/class/power_supply/battery/cycle_count";
const IBAT: &str = "/sys/class/power_supply/battery/current_now";
const TBAT: &str = "/sys/class/power_supply/battery/temp";
const VBAT: &str = "/sys/class/power_supply/battery/voltage_now";
const USB_VOLTAGE: &str = "/sys/class/power_supply/usb/voltage_now";

// Dual-family nodes (grade C): index 0 = power_supply branch, 1 = qcom branch.
const AUTHENTIC: [&str; 2] = [
    "/sys/class/power_supply/bms/authentic",
    "/sys/class/qcom-battery/authentic",
];
const REAL_TYPE: [&str; 2] = [
    "/sys/class/power_supply/usb/real_type",
    "/sys/class/qcom-battery/real_type",
];
const QUICK_CHARGE_TYPE: [&str; 2] = [
    "/sys/class/power_supply/usb/quick_charge_type",
    "/sys/class/qcom-battery/quick_charge_type",
];
const PD_VERIFIED: [&str; 2] = [
    "/sys/class/power_supply/usb/pd_authentication",
    "/sys/class/qcom-battery/pd_verifed",
];
const FASTCHG_MODE: [&str; 2] = [
    "/sys/class/power_supply/bms/fastcharge_mode",
    "/sys/class/qcom-battery/fastchg_mode",
];
const SOH: [&str; 2] = [
    "/sys/class/power_supply/bms/soh",
    "/sys/class/qcom-battery/soh",
];
const INPUT_SUSPEND: [&str; 2] = [
    "/sys/class/power_supply/battery/input_suspend",
    "/sys/class/qcom-battery/input_suspend",
];
const NIGHT_CHARGING: [&str; 2] = [
    "/sys/class/power_supply/battery/night_charging",
    "/sys/class/qcom-battery/night_charging",
];
const SMART_BATT: [&str; 2] = [
    "/sys/class/power_supply/battery/smart_batt",
    "/sys/class/qcom-battery/smart_batt",
];
const SOC_DECIMAL: [&str; 2] = [
    "/sys/class/power_supply/bms/soc_decimal",
    "/sys/class/qcom-battery/soc_decimal",
];
const SOC_DECIMAL_RATE: [&str; 2] = [
    "/sys/class/power_supply/bms/soc_decimal_rate",
    "/sys/class/qcom-battery/soc_decimal_rate",
];
const CAR_ADAPTER: [&str; 2] = [
    "/sys/class/power_supply/wireless/wls_car_adapter",
    "/sys/class/qcom-battery/wls_car_adapter",
];
const POWER_MAX: [&str; 2] = [
    "/sys/class/power_supply/usb/power_max",
    "/sys/class/qcom-battery/power_max",
];
// USB current is the odd one: both variants live under power_supply/usb, and
// the constructor flag picks which leaf the vendor implementation reads.
const USB_CURRENT: [&str; 2] = [
    "/sys/class/power_supply/usb/input_current_now",
    "/sys/class/power_supply/usb/current_now",
];
// Data-port signal. This generation has **no** `pc_port_online` node anywhere
// in its node set (HAL-NODES 222 table, 63 entries): its `isDPConnected` reads
// the DP-alt-mode flag instead. That flag is therefore what has to feed the
// adapter's data-port check — probing `pc_port_online` here would always fail
// and silently disable data-port suppression on this generation.
const HAS_DP: [&str; 2] = [
    // A grade. The vendor literal is missing its leading '/'; repaired here so
    // the read does not depend on the process working directory.
    "/sys/class/power_supply/usb/has_dp",
    // C grade, private qcom class.
    "/sys/class/qcom-battery/has_dp",
];

/// Whether a vendor manifest fragment exists. Plain file probe, no parsing —
/// the fragment name alone identifies the generation.
pub fn manifest_present(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

/// Platforms whose vendor implementation selects the `qcom-battery` branch.
/// Recovered from the constructor's platform comparison, in listing order.
const QCOM_PLATFORMS: [&str; 7] = [
    "lahaina",
    "taro",
    "kalama",
    "pineapple",
    "holi",
    "bengal",
    "parrot",
];

/// Mirror of the vendor constructor flag: `true` selects the `qcom-battery`
/// branch, `false` the `power_supply` branch. Pure so it stays unit-testable;
/// the only input is the platform string.
pub fn qcom_branch_for_platform(platform: &str) -> bool {
    QCOM_PLATFORMS.contains(&platform.trim())
}

/// Read `ro.board.platform` on device. `None` off-device or when unreadable;
/// callers treat that as the `power_supply` branch and document it.
fn board_platform() -> Option<String> {
    #[cfg(target_os = "android")]
    {
        use std::ffi::{CStr, CString};
        use std::os::raw::{c_char, c_int};
        unsafe extern "C" {
            fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
        }
        let name = CString::new("ro.board.platform").ok()?;
        let mut value = [0 as c_char; 93];
        let len = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
        if len <= 0 {
            return None;
        }
        let text = unsafe { CStr::from_ptr(value.as_ptr()) }.to_string_lossy();
        if text.trim().is_empty() {
            None
        } else {
            Some(text.into_owned())
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// Xiaomi HIDL-generation direct-node backend. See the module docs for what
/// this is and, just as importantly, what it is not.
pub struct HidlBackend {
    /// Branch resolved once at construction from `ro.board.platform`.
    qcom_branch: bool,
    /// Snapshot of the power-source nodes as of the last completed scan.
    last_probe: Mutex<FastChargeSnapshot>,
}

impl HidlBackend {
    pub fn new() -> Self {
        let qcom_branch = board_platform()
            .as_deref()
            .map(qcom_branch_for_platform)
            .unwrap_or(false);
        Self {
            qcom_branch,
            last_probe: Mutex::new(FastChargeSnapshot::default()),
        }
    }

    /// Branch index into the dual-family tables: 0 = power_supply, 1 = qcom.
    fn branch(&self) -> usize {
        usize::from(self.qcom_branch)
    }
}

impl Default for HidlBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ChargeBackend for HidlBackend {
    fn name(&self) -> &'static str {
        "hidl-nodes"
    }

    fn is_available(&self) -> bool {
        // A node reader depends on nothing external; selection already gated
        // on the generation markers, so this stays unconditionally true like
        // the generic fallback.
        true
    }

    fn refresh(&self, info: &mut ChargerInfo, should_cancel: &dyn Fn() -> bool) -> bool {
        if should_cancel() {
            return false;
        }
        let b = self.branch();

        // ── Online / identity stage ──
        sysfs::update_int_from_paths(&mut info.usb_online, &[&format!("{PSY_USB}/online")]);
        sysfs::update_int_from_paths(&mut info.ac_online, &[&format!("{PSY_AC}/online")]);
        sysfs::update_int_from_paths(
            &mut info.wireless_online,
            &[
                &format!("{PSY_WIRELESS}/online"),
                &format!("{PSY_DC}/online"),
            ],
        );
        sysfs::update_non_empty_string_from_paths(
            &mut info.battery_status,
            &[&format!("{PSY_BATTERY}/status")],
        );
        sysfs::update_non_empty_string_from_paths(
            &mut info.battery_health,
            &[&format!("{PSY_BATTERY}/health")],
        );
        // Data-port signal comes from `has_dp`, not `pc_port_online`: see the
        // constant's note. Both fields the adapter's data-port check reads are
        // therefore populated from this generation's own evidence.
        sysfs::update_int_from_paths(&mut info.pc_port_online, &[HAS_DP[b]]);
        if should_cancel() {
            return false;
        }

        // ── Unique standard nodes (grade A) ──
        if let Some(capacity) = sysfs::try_read_int_any(&[CAPACITY]) {
            info.battery_capacity = sysfs::normalize_battery_capacity(capacity);
        }
        sysfs::update_int_from_paths(&mut info.charge_full, &[CHARGE_FULL]);
        sysfs::update_int_from_paths(&mut info.cycle_count, &[CYCLE_COUNT]);
        sysfs::update_int_from_paths(&mut info.battery_current_now, &[IBAT]);
        sysfs::update_int_from_paths(&mut info.battery_temp, &[TBAT]);
        sysfs::update_int_from_paths(&mut info.battery_voltage_now, &[VBAT]);
        sysfs::update_int_from_paths(&mut info.usb_voltage_now, &[USB_VOLTAGE]);
        sysfs::update_int_from_paths(&mut info.usb_current_now, &[USB_CURRENT[b]]);
        if should_cancel() {
            return false;
        }

        // ── Branch-selected nodes (grade C, verbatim) ──
        sysfs::update_int_from_paths(&mut info.authentic, &[AUTHENTIC[b]]);
        let real_type = sysfs::read_string_any(&[REAL_TYPE[b]]);
        if !real_type.is_empty() {
            info.usb_real_type = real_type.clone();
            info.battery_charge_type = real_type.clone();
            info.usb_type = real_type;
        }
        sysfs::update_non_empty_string_from_paths(
            &mut info.quick_charge_type,
            &[QUICK_CHARGE_TYPE[b]],
        );
        sysfs::update_int_from_paths(&mut info.pd_verified, &[PD_VERIFIED[b]]);
        sysfs::update_int_from_paths(&mut info.fastchg_mode, &[FASTCHG_MODE[b]]);
        sysfs::update_int_from_paths(&mut info.fg_soh, &[SOH[b]]);
        sysfs::update_int_from_paths(&mut info.input_suspend, &[INPUT_SUSPEND[b]]);
        sysfs::update_int_from_paths(&mut info.night_charging, &[NIGHT_CHARGING[b]]);
        sysfs::update_int_from_paths(&mut info.smart_batt, &[SMART_BATT[b]]);
        // Collected but not served: unconfirmed scale, same policy as AIDL.
        sysfs::update_non_empty_string_from_paths(&mut info.soc_decimal, &[SOC_DECIMAL[b]]);
        sysfs::update_non_empty_string_from_paths(
            &mut info.soc_decimal_rate,
            &[SOC_DECIMAL_RATE[b]],
        );
        sysfs::update_non_empty_string_from_paths(&mut info.wireless_type, &[CAR_ADAPTER[b]]);
        if let Some(power) = sysfs::try_read_int_any(&[POWER_MAX[b]]) {
            info.adapter_power_w = sysfs::normalize_power_value(power);
        }
        if should_cancel() {
            return false;
        }

        // ── Session hygiene + classification (shared with the other backends) ──
        let charger_online =
            info.usb_online != 0 || info.ac_online != 0 || info.wireless_online != 0;
        if !charger_online {
            sysfs::clear_fast_charge_session(info);
        } else if sysfs::is_data_port(info) {
            sysfs::suppress_fast_charge_evidence(info);
        }
        info.fast_charge_type = sysfs::classify_fast_charge(info);
        info.charge_technology = sysfs::classify_charge_technology(info);
        info.charge_state = sysfs::classify_charge_state(info);
        // No remaining-time node is referenced by this generation: clear first
        // so the estimator cannot freeze on a stale value.
        info.remaining_time = 0;
        info.remaining_time = sysfs::estimate_remaining_time_seconds(info);

        *self.last_probe.lock() = sysfs::fast_charge_snapshot_from_info(info);
        true
    }

    fn set_charge_control(&self, restrict: bool) {
        // Mirrors the vendor setter's target node: the branch `input_suspend`
        // node, written verbatim with "1" / "0". No prefix: the only evidenced
        // prefix belongs to the AIDL generation. The value domain itself is an
        // inherited assumption, not a proven one — see the module docs.
        let value = if restrict { "1" } else { "0" };
        sysfs::write_string_any(&[INPUT_SUSPEND[self.branch()]], value);
    }

    fn power_source_changed(&self) -> bool {
        // Cheap probe only: online flags plus the two branch-selected strings
        // the classifier actually consumes. Never sleeps, never writes.
        let usb_online_path = format!("{PSY_USB}/online");
        let ac_online_path = format!("{PSY_AC}/online");
        let wireless_online_path = format!("{PSY_WIRELESS}/online");
        let dc_online_path = format!("{PSY_DC}/online");
        // Must read the same nodes `refresh` fills, or a change this probe
        // cannot see would never escalate to a full scan.
        let b = self.branch();
        let quick_charge_type = sysfs::read_string_any(&[QUICK_CHARGE_TYPE[b]]);
        let real_type = sysfs::read_string_any(&[REAL_TYPE[b]]);
        sysfs::power_source_probe_values_changed(
            &self.last_probe.lock(),
            sysfs::try_read_int(&usb_online_path),
            sysfs::try_read_int(&ac_online_path),
            sysfs::try_read_int_any(&[&wireless_online_path, &dc_online_path]),
            sysfs::try_read_int(HAS_DP[b]),
            (!quick_charge_type.is_empty()).then_some(quick_charge_type.as_str()),
            (!real_type.is_empty()).then_some(real_type.as_str()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::ChargerInfo;
    use crate::backend::ChargeBackend;

    #[test]
    fn platform_branch_matches_vendor_list() {
        for platform in [
            "lahaina",
            "taro",
            "kalama",
            "pineapple",
            "holi",
            "bengal",
            "parrot",
        ] {
            assert!(qcom_branch_for_platform(platform), "{platform}");
        }
        for platform in ["", "sun", "pineapple-sm8750", " mt6886 ", "unknown"] {
            assert!(!qcom_branch_for_platform(platform), "{platform}");
        }
    }

    #[test]
    fn missing_nodes_complete_without_panicking() {
        // Host has no kernel nodes: every read fails and the snapshot keeps
        // its defaults, but the scan still completes and stays cancellable.
        let backend = HidlBackend::new();
        assert_eq!(backend.name(), "hidl-nodes");
        assert!(backend.is_available());
        let mut info = ChargerInfo::default();
        assert!(backend.refresh(&mut info, &|| false));
        assert!(!backend.refresh(&mut info, &|| true));
        let _ = backend.power_source_changed();
        backend.set_charge_control(true);
        backend.set_charge_control(false);
    }

    #[test]
    fn every_node_path_is_absolute() {
        // The vendor binaries contain six path literals that are missing their
        // leading '/' (HAL-NODES §5.4): they only resolve because the vendor
        // service runs with cwd=/. Copying one of those literals verbatim would
        // silently make a read depend on our working directory, which is the
        // one class of bug in this file a host test can actually catch.
        let dual: [&[&str]; 15] = [
            &AUTHENTIC,
            &REAL_TYPE,
            &QUICK_CHARGE_TYPE,
            &PD_VERIFIED,
            &FASTCHG_MODE,
            &SOH,
            &INPUT_SUSPEND,
            &NIGHT_CHARGING,
            &SMART_BATT,
            &SOC_DECIMAL,
            &SOC_DECIMAL_RATE,
            &CAR_ADAPTER,
            &POWER_MAX,
            &USB_CURRENT,
            &HAS_DP,
        ];
        for table in dual {
            for path in table {
                assert!(path.starts_with('/'), "not absolute: {path}");
            }
        }
        let unique: [&str; 7] = [
            CAPACITY,
            CHARGE_FULL,
            CYCLE_COUNT,
            IBAT,
            TBAT,
            VBAT,
            USB_VOLTAGE,
        ];
        for path in unique {
            assert!(path.starts_with('/'), "not absolute: {path}");
        }
    }

    #[test]
    fn data_port_signal_comes_from_has_dp_not_pc_port_online() {
        // This generation's node set has no `pc_port_online` at all; the
        // data-port signal is `has_dp`. Pin the branch order so a future edit
        // cannot silently swap the two families.
        assert_eq!(HAS_DP[0], "/sys/class/power_supply/usb/has_dp");
        assert_eq!(HAS_DP[1], "/sys/class/qcom-battery/has_dp");
        assert!(!HAS_DP.iter().any(|p| p.contains("pc_port_online")));
    }
}
