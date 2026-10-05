//! Kernel node (`sysfs`) fallback backend.
//!
//! Reads charging data straight from the power-supply and qcom-battery nodes.
//! It is the fallback used when the vendor HAL is missing or has died: node
//! names, units and permissions differ between kernel generations, and the HAL
//! hides that, so the HAL is preferred whenever it is reachable.
//!
//! This module is a verbatim move of the node reader that used to live in
//! `adapter.rs`. The classifiers, unit heuristics, cancellation contract and
//! "keep the previous value when a node cannot be read" semantics of the
//! original full scan are preserved as-is.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;

use crate::adapter::{ChargeState, ChargerInfo, FastChargeSnapshot};

use super::ChargeBackend;

// ── sysfs helpers ──

pub fn try_read_int(path: &str) -> Option<i32> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(clamp_i64_to_i32)
}
pub fn read_int(path: &str) -> i32 {
    try_read_int(path).unwrap_or(0)
}
pub fn read_string(path: &str) -> String {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
pub fn path_exists(path: &str) -> bool {
    Path::new(path).exists()
}
pub fn try_read_int_any(paths: &[&str]) -> Option<i32> {
    paths.iter().find_map(|path| try_read_int(path))
}
pub fn read_int_any(paths: &[&str]) -> i32 {
    try_read_int_any(paths).unwrap_or(0)
}
pub fn update_int_from_paths(target: &mut i32, paths: &[&str]) {
    if let Some(value) = try_read_int_any(paths) {
        *target = value;
    }
}
pub fn update_non_empty_string_from_paths(target: &mut String, paths: &[&str]) {
    for path in paths {
        if let Ok(value) = fs::read_to_string(path) {
            let value = value.trim();
            if !value.is_empty() {
                target.clear();
                target.push_str(value);
                return;
            }
        }
    }
}

/// Online flag that lives on more than one supply.
///
/// A readable `0` must not hide a later supply that is online, and a miss
/// must not look like "offline". `None` means every path was unreadable.
pub fn try_read_online_any(paths: &[&str]) -> Option<i32> {
    merge_online_readings(paths.iter().map(|path| try_read_int(path)))
}

pub fn update_online_from_paths(target: &mut i32, paths: &[&str]) {
    if let Some(value) = try_read_online_any(paths) {
        *target = value;
    }
}

fn merge_online_readings(readings: impl IntoIterator<Item = Option<i32>>) -> Option<i32> {
    let mut saw_any = false;
    for value in readings.into_iter().flatten() {
        saw_any = true;
        if value != 0 {
            return Some(value);
        }
    }
    saw_any.then_some(0)
}

fn update_bool_from_path(target: &mut bool, path: &str) {
    if let Some(value) = try_read_int(path) {
        *target = value != 0;
    }
}

/// Apply a USB identity reading without letting a failed `real_type` read
/// erase the previous one.
///
/// `real_type` wins when it is non-empty. When that read fails and a previous
/// real type is still cached, the generic `type` node is ignored: on these
/// kernels it often says `USB` for every USB source, and feeding that into
/// the data-port check would suppress fast charge. The generic node is only
/// a fallback while no real type is known.
pub fn apply_usb_type_reading(info: &mut ChargerInfo, real_type: &str, fallback_type: &str) {
    let real_type = real_type.trim();
    if !real_type.is_empty() {
        info.usb_real_type.clear();
        info.usb_real_type.push_str(real_type);
        info.usb_type.clear();
        info.usb_type.push_str(real_type);
        return;
    }
    if !info.usb_real_type.is_empty() {
        return;
    }
    let fallback_type = fallback_type.trim();
    if !fallback_type.is_empty() {
        info.usb_type.clear();
        info.usb_type.push_str(fallback_type);
    }
}
pub fn read_positive_int_any(paths: &[&str]) -> i32 {
    for p in paths {
        let value = read_int(p);
        if value > 0 {
            return value;
        }
    }
    0
}
pub fn read_string_any(paths: &[&str]) -> String {
    for p in paths {
        if let Ok(value) = fs::read_to_string(p) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    String::new()
}
pub fn read_non_empty_string_any(paths: &[&str]) -> String {
    for p in paths {
        if path_exists(p) {
            let value = read_string(p);
            if !value.is_empty() {
                return value;
            }
        }
    }
    String::new()
}
pub fn write_string_any(paths: &[&str], value: &str) -> bool {
    let mut wrote = false;
    for p in paths {
        if path_exists(p) && fs::write(p, value).is_ok() {
            wrote = true;
        }
    }
    wrote
}
pub fn clamp_i64_to_i32(value: i64) -> i32 {
    value.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}
pub fn clamp_u64_to_i32(value: u64) -> i32 {
    value.min(i32::MAX as u64) as i32
}
pub fn abs_i32_to_i64(value: i32) -> i64 {
    (value as i64).abs()
}
pub fn normalize_capacity_mah(value: i32) -> i32 {
    if value <= 0 {
        0
    } else if value > 100_000 {
        value / 1000
    } else {
        value
    }
}
pub fn normalize_battery_capacity(value: i32) -> i32 {
    let percent = if value > 100 { value / 100 } else { value };
    percent.clamp(0, 100)
}
pub fn parse_first_int(value: &str) -> Option<i32> {
    value
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .find(|part| !part.is_empty() && *part != "-")
        .and_then(|part| part.parse::<i32>().ok())
}
pub fn parse_keyed_int(value: &str, key: &str) -> Option<i32> {
    value.split('+').find_map(|part| {
        let (name, raw) = part.split_once('=')?;
        if name == key {
            raw.trim().parse::<i32>().ok()
        } else {
            None
        }
    })
}
pub fn parse_plus_ints(value: &str) -> Vec<i32> {
    value
        .split('+')
        .filter_map(|part| part.trim().parse::<i32>().ok())
        .collect()
}
pub fn bool_like(value: &str) -> bool {
    matches!(
        value.trim(),
        "1" | "true" | "TRUE" | "True" | "enable" | "enabled" | "on"
    )
}
pub fn parse_bypass_switch(value: &str) -> bool {
    parse_keyed_int(value, "switch")
        .map(|v| v != 0)
        .unwrap_or_else(|| bool_like(value))
}
pub fn parse_charge_limit_state_payload(value: &str) -> (i32, Option<i32>) {
    let ints = parse_plus_ints(value);
    if ints.len() >= 3 && ints[0] == 2 {
        (ints[1], Some(ints[2]))
    } else if ints.len() >= 2 {
        (ints[0], Some(ints[1]))
    } else {
        (parse_first_int(value).unwrap_or(0), None)
    }
}
pub fn parse_charge_limit_control_payload(value: &str) -> (i32, Option<i32>, i32, Option<i32>) {
    let ints = parse_plus_ints(value);
    if ints.len() >= 5 && ints[0] == 4 {
        (ints[1], Some(ints[2]), ints[3], Some(ints[4]))
    } else if ints.len() >= 4 {
        (ints[0], Some(ints[1]), ints[2], Some(ints[3]))
    } else {
        (parse_first_int(value).unwrap_or(0), None, 0, None)
    }
}
pub fn restricted_charge_control_value(max_value: i32) -> String {
    if max_value > 1 {
        (max_value - 1).to_string()
    } else {
        CHARGE_CONTROL_LIMIT_RESTRICTED_FALLBACK.to_string()
    }
}

pub fn is_data_port_usb_type(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_uppercase().as_str(),
        "USB" | "SDP" | "USB_SDP" | "CDP" | "USB_CDP" | "PC" | "PC_PORT"
    )
}

/// Whether the attached USB source is a data port (SDP/CDP) rather than a
/// charger.
///
/// Both backends need this: fast-charge evidence must be dropped when a
/// computer is on the other end of the cable. The vendor HAL has no getter for
/// `usb_type` or `pc_port_online`, so the HAL backend fills those from the
/// kernel nodes and then asks the same question here.
pub fn is_data_port(info: &ChargerInfo) -> bool {
    info.usb_online != 0
        && (info.pc_port_online != 0
            || is_data_port_usb_type(&info.usb_real_type)
            || (info.usb_real_type.is_empty() && is_data_port_usb_type(&info.usb_type)))
}

/// Drop every fast-charge claim while keeping the connection state.
///
/// Used when the attached port turns out to be a data port: whatever the
/// charger nodes reported beforehand is stale evidence and must not reach the
/// classifier.
pub fn suppress_fast_charge_evidence(info: &mut ChargerInfo) {
    info.quick_charge_type = "0".into();
    info.pd_verified = 0;
    info.cp_online = 0;
    info.cp_status.clear();
    info.cp_master_iin = 0;
    info.cp_slave_iin = 0;
    info.fastchg_mode = 0;
    info.sport_mode = 0;
    info.adapter_power_w = 0;
}

// ── sysfs paths ──

pub const PSY_BATTERY: &str = "/sys/class/power_supply/battery";
pub const PSY_USB: &str = "/sys/class/power_supply/usb";
pub const PSY_AC: &str = "/sys/class/power_supply/ac";
pub const PSY_WIRELESS: &str = "/sys/class/power_supply/wireless";
pub const PSY_CP: &str = "/sys/class/power_supply/cp";
pub const PSY_DC: &str = "/sys/class/power_supply/dc";
pub const QCOM_BATT: &str = "/sys/class/qcom-battery";

pub const PD_VERIFIED_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/pd_verifed",
    "/sys/class/power_supply/usb/pd_authentication",
    "/sys/class/subpmic-battery/pd_verifed",
    "/sys/class/Charging_Adapter/pd_adapter/usbpd_verifed",
];
pub const QUICK_CHG_TYPE_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/quick_charge_type",
    "/sys/class/power_supply/usb/quick_charge_type",
    "/sys/class/power_supply/battery/quick_charge_type",
];
pub const QCOM_REAL_TYPE_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/real_type",
    "/sys/class/power_supply/usb/real_type",
    "/sys/class/qcom-battery/usb_real_type",
];
pub const PC_PORT_ONLINE_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/pc_port_online",
    "/sys/class/power_supply/usb/pc_port_online",
    "/sys/class/qcom-battery/pc_port_online",
];
pub const USB_CURRENT_NOW_PATHS: &[&str] = &[
    "/sys/class/power_supply/usb/current_now",
    "/sys/class/power_supply/usb/input_current_now",
];
pub const USB_CONNECTOR_TEMP_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/connector_temp",
    "/sys/class/power_supply/usb/usb_temp",
];
pub const CP_BUS_VOLTAGE_PATHS: &[&str] = &["/sys/class/qcom-battery/bq2597x_bus_voltage"];
pub const CP_BUS_CURRENT_PATHS: &[&str] = &["/sys/class/qcom-battery/bq2597x_bus_current"];
pub const CP_ONLINE_PATHS: &[&str] = &[
    "/sys/class/power_supply/cp/online",
    "/sys/class/qcom-battery/bq2597x_chip_ok",
    "/sys/class/qcom-battery/master_smb1396_online",
    "/sys/class/qcom-battery/slave_smb1396_online",
];
pub const FG_FCC_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/batt_fcc",
    "/sys/class/qcom-battery/fg1_fcc",
    "/sys/class/power_supply/battery/charge_full",
];
pub const CHARGE_FULL_DESIGN_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/charge_full_design",
    "/sys/class/qcom-battery/fg1_design_capacity",
    "/sys/class/qcom-battery/fg2_design_capacity",
];
pub const FG_RM_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/batt_rm",
    "/sys/class/qcom-battery/fg1_rm",
    "/sys/class/power_supply/battery/charge_counter",
];
pub const CHARGE_COUNTER_PATHS: &[&str] = &["/sys/class/power_supply/battery/charge_counter"];
pub const FG_RSOC: &str = "/sys/class/qcom-battery/fg1_rsoc";
pub const BATTERY_CAPACITY_PATHS: &[&str] = &["/sys/class/power_supply/battery/capacity", FG_RSOC];
pub const FG_CYCLE_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/fg1_cycle",
    "/sys/class/power_supply/battery/cycle_count",
];
pub const FG_SOH_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/fg1_soh",
    "/sys/class/qcom-battery/soh",
    "/sys/class/power_supply/bms/soh",
];
pub const FG_QMAX_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/fg1_qmax",
    "/sys/class/power_supply/battery/qmax",
];
pub const BATTERY_TYPE_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/battery_type",
    "/sys/class/power_supply/battery/technology",
];
pub const INPUT_CURRENT_MAX_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/fg1_current_max",
    "/sys/class/qcom-battery/constant_power",
    "/sys/class/qcom-battery/restrict_cur",
    "/sys/class/power_supply/usb_main/constant_charge_current_max",
    "/sys/class/power_supply/usb/current_max",
    "/sys/class/power_supply/usb_main/input_current_max",
];
pub const ADAPTER_POWER_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/apdo_max",
    "/sys/class/qcom-battery/power_max",
    "/sys/class/power_supply/usb/apdo_max",
    "/sys/class/power_supply/usb/power_max",
    "/sys/class/qcom-battery/referance_power",
];
pub const REMAINING_TIME_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/remaining_time",
    "/sys/class/power_supply/battery/time_to_full_now",
];
pub const FASTCHG_MODE_PATHS: &[&str] = &[
    "/sys/class/qcom-battery/fastchg_mode",
    "/sys/class/power_supply/bms/fastcharge_mode",
];
pub const CHARGE_CONTROL_LIMIT_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/charge_control_limit",
    "/sys/class/qcom-battery/charge_control_limit",
];
pub const CHARGE_CONTROL_LIMIT_MAX_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/charge_control_limit_max",
    "/sys/class/qcom-battery/charge_control_limit_max",
];
pub const INPUT_SUSPEND_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/input_suspend",
    "/sys/class/qcom-battery/input_suspend",
];
pub const CHARGE_CONTROL_LIMIT_RESTRICTED_FALLBACK: i32 = 15;
pub const CHARGE_CONTROL_LIMIT_RELEASED: &str = "0";
pub const POWER_RECHECK_MIN_W: i32 = 30;
pub const POWER_RECHECK_MAX_W: i32 = 35;
pub const POWER_RECHECK_DELAY_MS: u64 = 10;

// Individual qcom-battery nodes
pub const CP_MASTER_IIN: &str = "/sys/class/qcom-battery/master_smb1396_iin";
pub const CP_SLAVE_IIN: &str = "/sys/class/qcom-battery/slave_smb1396_iin";
pub const TYPEC_MODE: &str = "/sys/class/qcom-battery/typec_mode";
pub const CC_ORIENTATION: &str = "/sys/class/qcom-battery/cc_orientation";
pub const CURRENT_STATE: &str = "/sys/class/qcom-battery/current_state";
pub const SPORT_MODE: &str = "/sys/class/qcom-battery/sport_mode";
pub const WIRELESS_TYPE: &str = "/sys/class/qcom-battery/wireless_type";
pub const SMART_CHG: &str = "/sys/class/qcom-battery/smart_chg";
pub const NIGHT_CHARGING: &str = "/sys/class/qcom-battery/night_charging";
pub const SMART_BATT: &str = "/sys/class/qcom-battery/smart_batt";
pub const RESTRICT_CHG: &str = "/sys/class/qcom-battery/restrict_chg";
pub const BATT_SN_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/battery_sn",
    "/sys/class/qcom-battery/batt_sn",
    "/sys/class/power_supply/bms/serial_number",
    "/sys/class/power_supply/battery/serial_number",
];
pub const BATT_CONT_ONLINE: &str = "/sys/class/qcom-battery/battcont_online";
pub const FG_AI: &str = "/sys/class/qcom-battery/fg1_ai";
pub const FG_AVG_CURRENT: &str = "/sys/class/qcom-battery/fg1_avg_current";
pub const FG_VENDOR: &str = "/sys/class/qcom-battery/fg_vendor";
pub const FG_CELL1_VOL: &str = "/sys/class/qcom-battery/fg1_cell1_vol";
pub const FG_CELL2_VOL: &str = "/sys/class/qcom-battery/fg1_cell2_vol";
pub const FG_CELL1_RASCALE: &str = "/sys/class/qcom-battery/fg1_cell1_rascale";
pub const MAX_LIFE_TEMP: &str = "/sys/class/qcom-battery/max_life_temp";
pub const MAX_LIFE_VOL: &str = "/sys/class/qcom-battery/max_life_vol";
pub const OVER_VOL_DURATION: &str = "/sys/class/qcom-battery/over_vol_duration";
pub const MOISTURE_STATUS: &str = "/sys/class/qcom-battery/moisture_detection_status";
pub const THERMAL_BOARD_TEMP: &str = "/sys/class/qcom-battery/thermal_board_temp";
pub const DIE_TEMPERATURE: &str = "/sys/class/qcom-battery/die_temperature";
pub const SLAVE_DIE_TEMPERATURE: &str = "/sys/class/qcom-battery/slave_die_temperature";
pub const FLASH_ACTIVE: &str = "/sys/class/qcom-battery/flash_active";
pub const HIFI_CONNECT: &str = "/sys/class/qcom-battery/hifi_connect";
pub const VBUS_DISABLE: &str = "/sys/class/qcom-battery/vbus_disable";
pub const OTG_UI_SUPPORT: &str = "/sys/class/qcom-battery/otg_ui_support";
pub const FAKE_SOC: &str = "/sys/class/qcom-battery/fake_soc";
pub const FAKE_SOH: &str = "/sys/class/qcom-battery/fake_soh";
pub const FAKE_CYCLE: &str = "/sys/class/qcom-battery/fake_cycle";
pub const FAKE_TEMP: &str = "/sys/class/qcom-battery/fake_temp";

// ── Full scan ──

/// Read every node into `info`.
///
/// Returns `false` as soon as `should_cancel` reports that the screen is
/// transitioning, so the caller can re-queue the refresh instead of
/// publishing a half-read snapshot. A node that cannot be read leaves the
/// corresponding `info` field untouched.
pub fn poll_once<F>(info: &mut ChargerInfo, should_cancel: F) -> bool
where
    F: Fn() -> bool,
{
    if should_cancel() {
        return false;
    }
    let b = PSY_BATTERY;
    update_int_from_paths(&mut info.usb_online, &[&format!("{}/online", PSY_USB)]);
    update_int_from_paths(&mut info.ac_online, &[&format!("{}/online", PSY_AC)]);
    update_online_from_paths(
        &mut info.wireless_online,
        &[
            &format!("{}/online", PSY_WIRELESS),
            &format!("{}/online", PSY_DC),
        ],
    );
    update_non_empty_string_from_paths(&mut info.wireless_type, &[WIRELESS_TYPE]);
    if should_cancel() {
        return false;
    }

    update_int_from_paths(
        &mut info.usb_voltage_now,
        &[&format!("{}/voltage_now", PSY_USB)],
    );
    update_int_from_paths(&mut info.cp_bus_voltage, CP_BUS_VOLTAGE_PATHS);
    update_int_from_paths(&mut info.cp_bus_current, CP_BUS_CURRENT_PATHS);
    if info.usb_voltage_now == 0 {
        info.usb_voltage_now = info.cp_bus_voltage;
    }
    update_int_from_paths(&mut info.usb_current_now, USB_CURRENT_NOW_PATHS);
    if info.usb_current_now == 0 {
        info.usb_current_now = info.cp_bus_current;
    }

    update_int_from_paths(&mut info.usb_temp, USB_CONNECTOR_TEMP_PATHS);
    update_int_from_paths(&mut info.connector_temp, USB_CONNECTOR_TEMP_PATHS);
    let usb_type_path = format!("{}/type", PSY_USB);
    apply_usb_type_reading(
        info,
        &read_string_any(QCOM_REAL_TYPE_PATHS),
        &read_string(&usb_type_path),
    );
    update_non_empty_string_from_paths(&mut info.typec_mode, &[TYPEC_MODE]);
    update_int_from_paths(&mut info.cc_orientation, &[CC_ORIENTATION]);
    if should_cancel() {
        return false;
    }

    update_non_empty_string_from_paths(&mut info.battery_status, &[&format!("{}/status", b)]);
    update_non_empty_string_from_paths(&mut info.battery_health, &[&format!("{}/health", b)]);
    update_int_from_paths(&mut info.battery_temp, &[&format!("{}/temp", b)]);
    update_int_from_paths(
        &mut info.battery_current_now,
        &[&format!("{}/current_now", b)],
    );
    update_int_from_paths(
        &mut info.battery_voltage_now,
        &[&format!("{}/voltage_now", b)],
    );
    update_non_empty_string_from_paths(
        &mut info.battery_charge_type,
        &[&format!("{}/charge_type", b)],
    );
    update_non_empty_string_from_paths(
        &mut info.battery_technology,
        &[&format!("{}/technology", b)],
    );
    if let Some(capacity) = try_read_int_any(BATTERY_CAPACITY_PATHS) {
        info.battery_capacity = normalize_battery_capacity(capacity);
    }
    if should_cancel() {
        return false;
    }

    update_int_from_paths(&mut info.fg_fcc, &[FG_FCC_PATHS[0]]);
    update_int_from_paths(&mut info.charge_full, FG_FCC_PATHS);
    update_int_from_paths(&mut info.fg_rm, &[FG_RM_PATHS[0]]);
    update_int_from_paths(&mut info.fg_rsoc, &[FG_RSOC]);
    update_int_from_paths(&mut info.fg_cycle, &[FG_CYCLE_PATHS[0]]);
    update_int_from_paths(&mut info.fg_soh, FG_SOH_PATHS);
    update_int_from_paths(&mut info.fg_qmax, FG_QMAX_PATHS);
    update_int_from_paths(&mut info.fg_ai, &[FG_AI]);
    update_int_from_paths(&mut info.fg_avg_current, &[FG_AVG_CURRENT]);
    update_non_empty_string_from_paths(&mut info.fg_vendor, &[FG_VENDOR]);
    update_non_empty_string_from_paths(&mut info.battery_type, BATTERY_TYPE_PATHS);
    if !info.fg_vendor.is_empty() {
        info.gauge_type = info.fg_vendor.clone();
    }
    info.gauge_info.clear();
    update_int_from_paths(&mut info.charge_full_design, CHARGE_FULL_DESIGN_PATHS);
    if info.charge_full_design == 0 {
        info.charge_full_design = info.charge_full;
    }
    update_int_from_paths(&mut info.charge_counter, CHARGE_COUNTER_PATHS);
    if info.charge_counter == 0 {
        info.charge_counter = info.fg_rm;
    }
    update_int_from_paths(&mut info.cycle_count, FG_CYCLE_PATHS);
    if should_cancel() {
        return false;
    }

    update_int_from_paths(&mut info.cell1_vol, &[FG_CELL1_VOL]);
    update_int_from_paths(&mut info.cell2_vol, &[FG_CELL2_VOL]);
    update_int_from_paths(&mut info.cell1_rascale, &[FG_CELL1_RASCALE]);

    update_int_from_paths(&mut info.input_current_max, INPUT_CURRENT_MAX_PATHS);
    update_int_from_paths(
        &mut info.input_voltage_max,
        &[&format!("{}/voltage_max", PSY_USB)],
    );
    update_int_from_paths(&mut info.fastchg_mode, FASTCHG_MODE_PATHS);
    update_non_empty_string_from_paths(&mut info.current_state, &[CURRENT_STATE]);
    update_int_from_paths(&mut info.sport_mode, &[SPORT_MODE]);
    update_non_empty_string_from_paths(&mut info.quick_charge_type, QUICK_CHG_TYPE_PATHS);
    update_int_from_paths(&mut info.pd_verified, PD_VERIFIED_PATHS);
    if should_cancel() {
        return false;
    }

    update_int_from_paths(&mut info.cp_online, CP_ONLINE_PATHS);
    update_non_empty_string_from_paths(&mut info.cp_status, &[&format!("{}/status", PSY_CP)]);
    update_int_from_paths(&mut info.cp_master_iin, &[CP_MASTER_IIN]);
    update_int_from_paths(&mut info.cp_slave_iin, &[CP_SLAVE_IIN]);

    // A failed read must become 0. `estimate_remaining_time_seconds` returns a
    // positive cached value unchanged, so preserving the last node reading
    // would freeze the estimate after the node disappears.
    info.remaining_time = read_int_any(REMAINING_TIME_PATHS);
    update_int_from_paths(&mut info.restrict_chg, &[RESTRICT_CHG]);
    update_int_from_paths(&mut info.input_suspend, INPUT_SUSPEND_PATHS);
    update_int_from_paths(&mut info.smart_chg, &[SMART_CHG]);
    update_int_from_paths(&mut info.night_charging, &[NIGHT_CHARGING]);
    update_int_from_paths(&mut info.smart_batt, &[SMART_BATT]);
    if should_cancel() {
        return false;
    }

    update_int_from_paths(&mut info.die_temperature, &[DIE_TEMPERATURE]);
    update_int_from_paths(&mut info.slave_die_temperature, &[SLAVE_DIE_TEMPERATURE]);
    update_int_from_paths(&mut info.thermal_board_temp, &[THERMAL_BOARD_TEMP]);

    update_non_empty_string_from_paths(&mut info.batt_sn, BATT_SN_PATHS);
    // This backend has no OPPO auth node. Keep reporting a genuine battery so
    // ColorOS does not flag every Xiaomi pack. The HAL backends overwrite
    // this with the vendor node on their own refresh path.
    info.authentic = 1;
    update_int_from_paths(&mut info.batt_cont_online, &[BATT_CONT_ONLINE]);
    update_int_from_paths(&mut info.max_life_temp, &[MAX_LIFE_TEMP]);
    update_int_from_paths(&mut info.max_life_vol, &[MAX_LIFE_VOL]);
    update_int_from_paths(&mut info.over_vol_duration, &[OVER_VOL_DURATION]);
    update_bool_from_path(&mut info.moisture_detected, MOISTURE_STATUS);
    if should_cancel() {
        return false;
    }

    update_bool_from_path(&mut info.flash_active, FLASH_ACTIVE);
    update_bool_from_path(&mut info.hifi_connect, HIFI_CONNECT);
    update_bool_from_path(&mut info.vbus_disable, VBUS_DISABLE);
    update_int_from_paths(&mut info.otg_ui_support, &[OTG_UI_SUPPORT]);

    update_int_from_paths(&mut info.fake_soc, &[FAKE_SOC]);
    update_int_from_paths(&mut info.fake_soh, &[FAKE_SOH]);
    update_int_from_paths(&mut info.fake_cycle, &[FAKE_CYCLE]);
    update_int_from_paths(&mut info.fake_temp, &[FAKE_TEMP]);

    update_int_from_paths(&mut info.pc_port_online, PC_PORT_ONLINE_PATHS);
    if should_cancel() {
        return false;
    }

    let charger_online = info.usb_online != 0 || info.ac_online != 0 || info.wireless_online != 0;
    let data_port = is_data_port(info);
    if !charger_online {
        clear_fast_charge_session(info);
    } else if data_port {
        suppress_fast_charge_evidence(info);
    } else {
        let adapter_power_w = estimate_power(info);
        if should_cancel() {
            return false;
        }
        info.adapter_power_w = stable_adapter_power_w_with(
            adapter_power_w,
            &info.quick_charge_type,
            current_quick_charge_type,
            read_adapter_power_direct_w,
            || thread::sleep(Duration::from_millis(POWER_RECHECK_DELAY_MS)),
        );
        if should_cancel() {
            return false;
        }
    }
    info.fast_charge_type = classify_fast_charge(info);
    info.charge_technology = classify_charge_technology(info);
    info.charge_state = classify_charge_state(info);
    info.remaining_time = estimate_remaining_time_seconds(info);
    true
}

pub fn clear_fast_charge_session(info: &mut ChargerInfo) {
    info.usb_type.clear();
    info.usb_real_type.clear();
    info.quick_charge_type.clear();
    info.pd_verified = 0;
    info.cp_online = 0;
    info.cp_status.clear();
    info.cp_bus_voltage = 0;
    info.cp_bus_current = 0;
    info.cp_master_iin = 0;
    info.cp_slave_iin = 0;
    info.fastchg_mode = 0;
    info.sport_mode = 0;
    info.adapter_power_w = 0;
    info.fast_charge_type = "0".into();
    info.charge_technology = "0".into();
}

// ── Classifiers ──

/// Fast-charge inputs pulled out of a snapshot.
pub(crate) struct FastChargeInputs<'a> {
    quick_charge_type: &'a str,
    fastchg_mode: i32,
    sport_mode: i32,
    pd_verified: i32,
    cp_online: i32,
    usb_type: &'a str,
    adapter_power_w: i32,
    online: bool,
    usb_online: i32,
    pc_port_online: i32,
}

/// Vendor quick_charge_type → fast_charge_type (0-3)
pub(crate) fn classify_fast_charge_values(inputs: FastChargeInputs<'_>) -> i32 {
    let FastChargeInputs {
        quick_charge_type,
        fastchg_mode,
        sport_mode,
        pd_verified,
        cp_online,
        usb_type,
        adapter_power_w,
        online,
        usb_online,
        pc_port_online,
    } = inputs;
    if !online {
        return 0;
    }
    if usb_online != 0 && (pc_port_online != 0 || is_data_port_usb_type(usb_type)) {
        return 0;
    }
    let qct_num: i32 = quick_charge_type.trim().parse().unwrap_or(-1);
    match qct_num {
        1 => return 1,
        2 => return 2,
        3 | 4 => return 3,
        _ => {}
    }
    let qct = quick_charge_type;
    if qct.contains("Super")
        || qct.contains("SUPER")
        || qct.contains("Turbo")
        || qct.contains("TURBO")
    {
        return 3;
    }
    if qct.contains("Flash") || qct.contains("FLASH") {
        return 2;
    }
    if qct.contains("Fast") || qct.contains("FAST") {
        return 1;
    }
    if qct.contains("Normal") || qct.contains("NORMAL") {
        return 0;
    }
    if fastchg_mode != 0 || sport_mode != 0 || pd_verified != 0 || cp_online != 0 {
        return 3;
    }
    let usb_type = usb_type.to_lowercase();
    if usb_type.contains("pd") || usb_type.contains("pps") {
        return 3;
    }
    if usb_type.contains("qc") || usb_type.contains("hvdcp") || usb_type.contains("quick") {
        return 2;
    }
    if usb_type.contains("dcp") {
        return 1;
    }
    if usb_type.contains("sdp") || usb_type.contains("cdp") {
        return 0;
    }
    // `online` was already handled above. Folding it into this branch made
    // every attached source — including a 0 W read — report fast-charge
    // type 1, so the power thresholds below never applied.
    if adapter_power_w > 20 {
        3
    } else if adapter_power_w > 10 {
        2
    } else if adapter_power_w > 3 {
        1
    } else {
        0
    }
}

pub fn classify_fast_charge(info: &ChargerInfo) -> String {
    let ut = if !info.usb_real_type.is_empty() {
        &info.usb_real_type
    } else {
        &info.usb_type
    };
    classify_fast_charge_values(FastChargeInputs {
        quick_charge_type: &info.quick_charge_type,
        fastchg_mode: info.fastchg_mode,
        sport_mode: info.sport_mode,
        pd_verified: info.pd_verified,
        cp_online: info.cp_online,
        usb_type: ut,
        adapter_power_w: info.adapter_power_w,
        online: info.usb_online != 0 || info.ac_online != 0 || info.wireless_online != 0,
        usb_online: info.usb_online,
        pc_port_online: info.pc_port_online,
    })
    .to_string()
}

/// Vendor quick_charge_type → charge_technology (0=normal,1=QC,2=HVDCP,3=PD_PPS)
pub fn classify_charge_technology_values(quick_charge_type: &str, fast_type: i32) -> i32 {
    let qct_num: i32 = quick_charge_type.trim().parse().unwrap_or(-1);
    match qct_num {
        1 => return 1,
        2 => return 2,
        3 | 4 => return 3,
        _ => {}
    }
    if quick_charge_type.contains("Super")
        || quick_charge_type.contains("SUPER")
        || quick_charge_type.contains("Turbo")
        || quick_charge_type.contains("TURBO")
    {
        return 3;
    }
    if quick_charge_type.contains("Flash") || quick_charge_type.contains("FLASH") {
        return 2;
    }
    if quick_charge_type.contains("Fast") || quick_charge_type.contains("FAST") {
        return 1;
    }
    fast_type
}

pub fn classify_charge_technology(info: &ChargerInfo) -> String {
    classify_charge_technology_values(
        &info.quick_charge_type,
        info.fast_charge_type.trim().parse().unwrap_or(0),
    )
    .to_string()
}

pub(crate) fn classify_charge_state(info: &ChargerInfo) -> ChargeState {
    if info.usb_online == 0 && info.ac_online == 0 && info.wireless_online == 0 {
        return ChargeState::Disconnected;
    }
    let pw = info.adapter_power_w;
    if pw > 30 {
        ChargeState::SuperCharging
    } else if pw > 20 {
        ChargeState::FlashCharging
    } else if pw > 10 {
        ChargeState::FastCharging
    } else if pw > 3 {
        ChargeState::NormalCharging
    } else {
        ChargeState::SlowCharging
    }
}

pub fn estimate_remaining_time_seconds(info: &ChargerInfo) -> i32 {
    if info.remaining_time > 0 {
        return if info.remaining_time > 86_400 {
            info.remaining_time / 1000
        } else {
            info.remaining_time
        };
    }
    if info.battery_status.eq_ignore_ascii_case("Full") || info.battery_capacity >= 100 {
        return 0;
    }
    if info.usb_online == 0 && info.ac_online == 0 && info.wireless_online == 0 {
        return 0;
    }

    let remaining_mah = if info.fg_fcc > 0 && info.fg_rm > 0 && info.fg_fcc > info.fg_rm {
        normalize_capacity_mah(info.fg_fcc - info.fg_rm)
    } else {
        let fcc_mah = best_fcc_mah(info);
        if fcc_mah <= 0 {
            return 0;
        }
        clamp_i64_to_i32((fcc_mah as i64) * ((100 - info.battery_capacity).max(0) as i64) / 100)
    };
    if remaining_mah <= 0 {
        return 0;
    }

    let current_abs = abs_i32_to_i64(info.battery_current_now);
    let current_ma = if current_abs > 100_000 {
        current_abs / 1000
    } else {
        current_abs
    };
    if current_ma <= 0 {
        return 0;
    }
    clamp_i64_to_i32((remaining_mah as i64) * 3600 / current_ma)
}

/// Power in watts. Auto-detects W / mW / µW units.
pub fn estimate_power(info: &ChargerInfo) -> i32 {
    let direct_pw = read_positive_int_any(ADAPTER_POWER_PATHS);
    if direct_pw > 0 {
        return normalize_power_value(direct_pw);
    }
    let const_pw = read_int(&format!("{}/constant_power", QCOM_BATT));
    if const_pw > 0 {
        return normalize_power_value(const_pw);
    }
    if info.cp_bus_voltage > 0 && info.cp_bus_current > 0 {
        return power_watts(info.cp_bus_voltage, info.cp_bus_current);
    }
    if info.cp_master_iin > 0 || info.cp_slave_iin > 0 {
        let v = if info.usb_voltage_now > 0 {
            info.usb_voltage_now
        } else {
            info.battery_voltage_now
        };
        let total_i = info.cp_master_iin.saturating_add(info.cp_slave_iin);
        if v > 0 && total_i > 0 {
            return power_watts(v, total_i);
        }
    }
    let v = if info.usb_voltage_now > 0 {
        info.usb_voltage_now
    } else {
        info.battery_voltage_now
    };
    let c = if info.input_current_max > 0 {
        info.input_current_max
    } else if info.usb_current_now > 0 {
        info.usb_current_now
    } else {
        clamp_i64_to_i32(abs_i32_to_i64(info.battery_current_now))
    };
    power_watts(v, c)
}

pub fn power_watts(voltage: i32, current: i32) -> i32 {
    if voltage <= 0 || current <= 0 {
        return 0;
    }
    let voltage = voltage as u64;
    let current = current as u64;
    let divisor = match (voltage > 100_000, current > 100_000) {
        (true, true) => 1_000_000_000_000,
        (true, false) | (false, true) => 1_000_000_000,
        (false, false) => 1_000,
    };
    clamp_u64_to_i32(voltage.saturating_mul(current) / divisor)
}

pub fn normalize_power_value(power: i32) -> i32 {
    if power > 100_000 {
        power / 1_000_000
    } else if power > 500 {
        power / 1_000
    } else {
        power
    }
}

pub fn read_adapter_power_direct_w() -> i32 {
    let direct_pw = read_positive_int_any(ADAPTER_POWER_PATHS);
    if direct_pw > 0 {
        normalize_power_value(direct_pw)
    } else {
        0
    }
}

pub fn current_quick_charge_type() -> String {
    read_string_any(QUICK_CHG_TYPE_PATHS)
}

pub fn is_quick_charge_type_4(value: &str) -> bool {
    value.trim() == "4" || value.to_ascii_lowercase().contains("super")
}

pub fn fast_charge_snapshot_from_info(info: &ChargerInfo) -> FastChargeSnapshot {
    let usb_type = if !info.usb_real_type.is_empty() {
        info.usb_real_type.clone()
    } else {
        info.usb_type.clone()
    };
    FastChargeSnapshot {
        online: info.usb_online != 0 || info.ac_online != 0 || info.wireless_online != 0,
        usb_online: info.usb_online,
        ac_online: info.ac_online,
        wireless_online: info.wireless_online,
        fast_type: info.fast_charge_type.trim().parse().unwrap_or(0),
        charge_tech: info.charge_technology.trim().parse().unwrap_or(0),
        quick_charge_type: info.quick_charge_type.clone(),
        pd_verified: info.pd_verified,
        cp_online: info.cp_online,
        fastchg_mode: info.fastchg_mode,
        sport_mode: info.sport_mode,
        adapter_power_w: info.adapter_power_w,
        usb_type,
        pc_port_online: info.pc_port_online,
    }
}

pub fn best_fcc(info: &ChargerInfo) -> i32 {
    if info.fg_fcc > 0 {
        info.fg_fcc
    } else {
        info.charge_full
    }
}

pub fn best_fcc_mah(info: &ChargerInfo) -> i32 {
    normalize_capacity_mah(best_fcc(info))
}

pub fn should_recheck_power(power_w: i32) -> bool {
    (POWER_RECHECK_MIN_W..=POWER_RECHECK_MAX_W).contains(&power_w)
}

pub fn stable_adapter_power_w_with<F, G, H>(
    current_power_w: i32,
    quick_charge_type: &str,
    mut read_quick_charge_type: F,
    mut read_power_w: G,
    mut wait: H,
) -> i32
where
    F: FnMut() -> String,
    G: FnMut() -> i32,
    H: FnMut(),
{
    if !should_recheck_power(current_power_w) {
        return current_power_w;
    }
    let quick_charge_type = if quick_charge_type.trim().is_empty() {
        read_quick_charge_type()
    } else {
        quick_charge_type.to_string()
    };
    if !is_quick_charge_type_4(&quick_charge_type) {
        return current_power_w;
    }
    let mut best_power_w = current_power_w;
    for _ in 0..3 {
        wait();
        best_power_w = best_power_w.max(read_power_w());
    }
    best_power_w
}

// ── Power-source probe ──

/// Compare a fresh, cheap node probe against `snapshot`.
///
/// Only `Some` values are compared: an empty or unreadable node never counts
/// as a change, so a missing node cannot spin the poll loop.
pub fn power_source_probe_values_changed(
    snapshot: &FastChargeSnapshot,
    usb_online: Option<i32>,
    ac_online: Option<i32>,
    wireless_online: Option<i32>,
    pc_port_online: Option<i32>,
    quick_charge_type: Option<&str>,
    usb_type: Option<&str>,
) -> bool {
    usb_online.is_some_and(|value| value != snapshot.usb_online)
        || ac_online.is_some_and(|value| value != snapshot.ac_online)
        || wireless_online.is_some_and(|value| value != snapshot.wireless_online)
        || pc_port_online.is_some_and(|value| value != snapshot.pc_port_online)
        || quick_charge_type.is_some_and(|value| value != snapshot.quick_charge_type)
        || usb_type.is_some_and(|value| value != snapshot.usb_type)
}

/// Cheap power-source probe: reads a handful of nodes and compares them with
/// `snapshot`. Never sleeps, never writes and never runs a full scan.
pub fn power_source_probe_changed(snapshot: &FastChargeSnapshot) -> bool {
    let usb_online_path = format!("{}/online", PSY_USB);
    let ac_online_path = format!("{}/online", PSY_AC);
    let wireless_online_path = format!("{}/online", PSY_WIRELESS);
    let dc_online_path = format!("{}/online", PSY_DC);
    let quick_charge_type = read_string_any(QUICK_CHG_TYPE_PATHS);
    let usb_type = read_string_any(QCOM_REAL_TYPE_PATHS);
    power_source_probe_values_changed(
        snapshot,
        try_read_int(&usb_online_path),
        try_read_int(&ac_online_path),
        try_read_online_any(&[&wireless_online_path, &dc_online_path]),
        try_read_int_any(PC_PORT_ONLINE_PATHS),
        (!quick_charge_type.is_empty()).then_some(quick_charge_type.as_str()),
        (!usb_type.is_empty()).then_some(usb_type.as_str()),
    )
}

// ── Charge control ──

/// Restrict or release charging.
///
/// Writes the standard `charge_control_limit` node (`max - 1`, or the
/// fallback when `charge_control_limit_max` is missing) and the vendor
/// `input_suspend` node. Releasing writes `0` to both. Cool-mode nodes are
/// left alone: their value domain is unconfirmed, and writing them on top of
/// input suspend drives two controls for one request.
pub fn apply_charge_control_limit(restrict: bool) {
    let restricted_value;
    let value = if restrict {
        restricted_value =
            restricted_charge_control_value(read_int_any(CHARGE_CONTROL_LIMIT_MAX_PATHS));
        restricted_value.as_str()
    } else {
        CHARGE_CONTROL_LIMIT_RELEASED
    };
    write_string_any(CHARGE_CONTROL_LIMIT_PATHS, value);
    // `input_suspend` is the restrict switch the vendor generations actually
    // use. `cool_mode` / `cool_down` are a different control whose value
    // domain is unconfirmed, so they are not written here.
    write_string_any(INPUT_SUSPEND_PATHS, if restrict { "1" } else { "0" });
}

// ── Backend ──

/// Kernel node fallback backend.
pub struct SysfsBackend {
    /// Snapshot of the power-source nodes as of the last completed scan. The
    /// cheap uevent probe compares the live nodes against it.
    last_probe: Mutex<FastChargeSnapshot>,
}

impl SysfsBackend {
    pub fn new() -> Self {
        Self {
            last_probe: Mutex::new(FastChargeSnapshot::default()),
        }
    }
}

impl Default for SysfsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ChargeBackend for SysfsBackend {
    fn name(&self) -> &'static str {
        "sysfs"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn refresh(&self, info: &mut ChargerInfo, should_cancel: &dyn Fn() -> bool) -> bool {
        if !poll_once(info, should_cancel) {
            return false;
        }
        *self.last_probe.lock() = fast_charge_snapshot_from_info(info);
        true
    }

    fn set_charge_control(&self, restrict: bool) {
        apply_charge_control_limit(restrict);
    }

    fn power_source_changed(&self) -> bool {
        let snapshot = self.last_probe.lock().clone();
        power_source_probe_changed(&snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::FastChargeSnapshot;

    fn fast_inputs<'a>(
        quick_charge_type: &'a str,
        pd_verified: i32,
        cp_online: i32,
        adapter_power_w: i32,
        online: bool,
    ) -> FastChargeInputs<'a> {
        FastChargeInputs {
            quick_charge_type,
            fastchg_mode: 0,
            sport_mode: 0,
            pd_verified,
            cp_online,
            usb_type: "",
            adapter_power_w,
            online,
            usb_online: online as i32,
            pc_port_online: 0,
        }
    }

    #[test]
    fn qct4_rechecks_33w_and_returns_highest_power() {
        let mut powers = [33, 33, 67, 65, 67].into_iter();
        let power = stable_adapter_power_w_with(
            33,
            "4",
            || "4".into(),
            || powers.next().unwrap_or(33),
            || {},
        );
        assert_eq!(power, 67);
    }

    #[test]
    fn non_qct4_keeps_33w_without_power_reads() {
        let mut reads = 0;
        let power = stable_adapter_power_w_with(
            33,
            "3",
            || "3".into(),
            || {
                reads += 1;
                67
            },
            || {},
        );
        assert_eq!(power, 33);
        assert_eq!(reads, 0);
    }

    #[test]
    fn non_33w_power_does_not_recheck() {
        let mut reads = 0;
        let power = stable_adapter_power_w_with(
            67,
            "4",
            || "4".into(),
            || {
                reads += 1;
                33
            },
            || {},
        );
        assert_eq!(power, 67);
        assert_eq!(reads, 0);
    }

    #[test]
    fn direct_fast_classifier_detects_qct4_immediately() {
        assert_eq!(
            classify_fast_charge_values(fast_inputs("4", 0, 0, 0, true)),
            3
        );
        assert_eq!(classify_charge_technology_values("4", 3), 3);
        assert_eq!(
            classify_fast_charge_values(fast_inputs("4", 1, 1, 67, false)),
            0
        );
    }

    #[test]
    fn direct_fast_classifier_uses_pd_cp_and_power_fallbacks() {
        assert_eq!(
            classify_fast_charge_values(fast_inputs("", 1, 0, 0, true)),
            3
        );
        assert_eq!(
            classify_fast_charge_values(fast_inputs("", 0, 1, 0, true)),
            3
        );
        assert_eq!(
            classify_fast_charge_values(fast_inputs("", 0, 0, 21, true)),
            3
        );
        assert_eq!(
            classify_fast_charge_values(fast_inputs("0", 1, 0, 0, true)),
            3
        );
        assert_eq!(classify_charge_technology_values("0", 3), 3);
    }

    #[test]
    fn computer_usb_overrides_all_stale_fast_charge_evidence() {
        let inputs = FastChargeInputs {
            quick_charge_type: "4",
            fastchg_mode: 1,
            sport_mode: 1,
            pd_verified: 1,
            cp_online: 1,
            usb_type: "USB_SDP",
            adapter_power_w: 67,
            online: true,
            usb_online: 1,
            pc_port_online: 1,
        };
        assert_eq!(classify_fast_charge_values(inputs), 0);

        let snapshot = FastChargeSnapshot {
            online: true,
            usb_online: 1,
            ac_online: 0,
            wireless_online: 0,
            fast_type: 3,
            charge_tech: 3,
            quick_charge_type: "4".into(),
            pd_verified: 1,
            cp_online: 1,
            fastchg_mode: 1,
            sport_mode: 1,
            adapter_power_w: 67,
            usb_type: "USB_SDP".into(),
            pc_port_online: 1,
        };
        assert!(!snapshot.is_fast_charge());
        assert!(!snapshot.is_svooc_active());
        assert!(!snapshot.is_pps_active());
        assert!(!snapshot.should_show_power());
    }

    #[test]
    fn disconnect_clears_the_entire_fast_charge_session() {
        let mut info = ChargerInfo {
            usb_type: "USB_PD".into(),
            usb_real_type: "USB_PD".into(),
            quick_charge_type: "4".into(),
            pd_verified: 1,
            cp_online: 1,
            cp_status: "Charging".into(),
            cp_bus_voltage: 20_000_000,
            cp_bus_current: 3_000_000,
            cp_master_iin: 1_500_000,
            cp_slave_iin: 1_500_000,
            fastchg_mode: 1,
            sport_mode: 1,
            adapter_power_w: 67,
            fast_charge_type: "3".into(),
            charge_technology: "3".into(),
            ..Default::default()
        };
        clear_fast_charge_session(&mut info);
        assert!(info.usb_type.is_empty());
        assert!(info.usb_real_type.is_empty());
        assert!(info.quick_charge_type.is_empty());
        assert_eq!(info.pd_verified, 0);
        assert_eq!(info.cp_online, 0);
        assert_eq!(info.adapter_power_w, 0);
        assert_eq!(info.fast_charge_type, "0");
        assert_eq!(info.charge_technology, "0");
    }

    #[test]
    fn estimates_remaining_time_from_capacity_and_current() {
        let info = ChargerInfo {
            usb_online: 1,
            battery_status: "Charging".into(),
            battery_capacity: 50,
            fg_fcc: 4000,
            fg_rm: 2000,
            battery_current_now: -2_000_000,
            ..Default::default()
        };

        assert_eq!(estimate_remaining_time_seconds(&info), 3600);
    }

    #[test]
    fn normalizes_remaining_time_node_units() {
        let info = ChargerInfo {
            usb_online: 1,
            remaining_time: 3_600_000,
            ..Default::default()
        };

        assert_eq!(estimate_remaining_time_seconds(&info), 3600);
    }

    #[test]
    fn bypass_bool_parser_accepts_common_enabled_values() {
        assert!(bool_like("1"));
        assert!(bool_like("enable"));
        assert!(bool_like("on"));
        assert!(!bool_like("0"));
        assert!(!bool_like("disable"));
    }

    #[test]
    fn parses_coloros_bypass_switch_payload() {
        assert!(parse_bypass_switch("1+switch=1"));
        assert!(!parse_bypass_switch("1+switch=0"));
    }

    #[test]
    fn parses_coloros_charge_limit_state_payload() {
        assert_eq!(parse_charge_limit_state_payload("2++1+80"), (1, Some(80)));
        assert_eq!(parse_charge_limit_state_payload("2++0+90"), (0, Some(90)));
    }

    #[test]
    fn parses_coloros_charge_limit_control_payload() {
        assert_eq!(
            parse_charge_limit_control_payload("4++1+80+1+80"),
            (1, Some(80), 1, Some(80))
        );
        assert_eq!(
            parse_charge_limit_control_payload("4++0+80+0+80"),
            (0, Some(80), 0, Some(80))
        );
    }

    #[test]
    fn normalizes_capacity_without_decimal_pollution() {
        assert_eq!(normalize_battery_capacity(89), 89);
        assert_eq!(normalize_battery_capacity(8999), 89);
        assert_eq!(normalize_battery_capacity(10000), 100);
    }

    #[test]
    fn integer_fallback_skips_unreadable_or_invalid_nodes() {
        let directory =
            std::env::temp_dir().join(format!("chargerhal-read-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let invalid = directory.join("invalid");
        let valid = directory.join("valid");
        std::fs::write(&invalid, "not-a-number\n").unwrap();
        std::fs::write(&valid, "67\n").unwrap();

        assert_eq!(
            read_int_any(&[invalid.to_str().unwrap(), valid.to_str().unwrap()]),
            67
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn transient_sysfs_failures_preserve_cached_values() {
        let directory =
            std::env::temp_dir().join(format!("chargerhal-cache-preserve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let missing = directory.join("missing");
        let value_path = directory.join("value");

        let mut integer = 42;
        update_int_from_paths(&mut integer, &[missing.to_str().unwrap()]);
        assert_eq!(integer, 42);

        std::fs::write(&value_path, "0\n").unwrap();
        update_int_from_paths(&mut integer, &[value_path.to_str().unwrap()]);
        assert_eq!(integer, 0);

        let mut text = "cached".to_string();
        update_non_empty_string_from_paths(&mut text, &[missing.to_str().unwrap()]);
        assert_eq!(text, "cached");

        std::fs::write(&value_path, "Charging\n").unwrap();
        update_non_empty_string_from_paths(&mut text, &[value_path.to_str().unwrap()]);
        assert_eq!(text, "Charging");

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn lightweight_probe_only_escalates_on_power_source_changes() {
        let snapshot = FastChargeSnapshot {
            usb_online: 1,
            ac_online: 0,
            wireless_online: 0,
            pc_port_online: 0,
            quick_charge_type: "4".into(),
            usb_type: "USB_PD".into(),
            ..Default::default()
        };

        assert!(!power_source_probe_values_changed(
            &snapshot,
            Some(1),
            Some(0),
            Some(0),
            Some(0),
            Some("4"),
            Some("USB_PD"),
        ));
        assert!(power_source_probe_values_changed(
            &snapshot,
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some("4"),
            Some("USB_PD"),
        ));
        assert!(power_source_probe_values_changed(
            &snapshot,
            None,
            None,
            None,
            None,
            Some("0"),
            None,
        ));
    }

    #[test]
    fn full_scan_honors_screen_transition_cancellation() {
        let backend = SysfsBackend::new();
        let checks = std::cell::Cell::new(0);
        let mut info = ChargerInfo::default();
        let completed = backend.refresh(&mut info, &|| {
            let next = checks.get() + 1;
            checks.set(next);
            next >= 2
        });

        assert!(!completed);
        assert!(checks.get() >= 2);
    }

    #[test]
    fn restricted_charge_control_uses_max_minus_one() {
        assert_eq!(restricted_charge_control_value(16), "15");
        assert_eq!(restricted_charge_control_value(2), "1");
        assert_eq!(restricted_charge_control_value(0), "15");
    }

    #[test]
    fn slow_or_unknown_power_is_not_reported_as_fast_charge() {
        assert_eq!(
            classify_fast_charge_values(fast_inputs("", 0, 0, 0, true)),
            0
        );
        assert_eq!(
            classify_fast_charge_values(fast_inputs("", 0, 0, 3, true)),
            0
        );
        assert_eq!(
            classify_fast_charge_values(fast_inputs("", 0, 0, 5, true)),
            1
        );
    }

    #[test]
    fn failed_real_type_read_does_not_become_a_generic_usb_data_port() {
        let mut info = ChargerInfo {
            usb_online: 1,
            usb_real_type: "USB_PD".into(),
            usb_type: "USB_PD".into(),
            ..Default::default()
        };
        apply_usb_type_reading(&mut info, "", "USB");
        assert_eq!(info.usb_real_type, "USB_PD");
        assert_eq!(info.usb_type, "USB_PD");
        assert!(!is_data_port(&info));

        apply_usb_type_reading(&mut info, "USB_DCP", "USB");
        assert_eq!(info.usb_real_type, "USB_DCP");
        assert!(!is_data_port(&info));
    }

    #[test]
    fn online_readings_do_not_let_a_leading_zero_hide_a_later_supply() {
        assert_eq!(merge_online_readings([None, Some(0), Some(1)]), Some(1));
        assert_eq!(merge_online_readings([Some(0), None]), Some(0));
        assert_eq!(merge_online_readings([None, None]), None);
    }
}
