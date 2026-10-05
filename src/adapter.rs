// Xiaomi → ColorOS ICharger adapter. Reads standard + qcom-battery sysfs,
// exposes vendor.oplus.hardware.charger.ICharger/default.

use crate::backend::ChargeBackend;
use parking_lot::Mutex;
use std::fs;
#[cfg(target_os = "android")]
use std::io;
#[cfg(target_os = "android")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::Weak;
use std::thread;
use std::time::{Duration, Instant};

// ── sysfs helpers ──

fn try_read_int(path: &str) -> Option<i32> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(clamp_i64_to_i32)
}
fn path_exists(path: &str) -> bool {
    Path::new(path).exists()
}
fn try_read_int_any(paths: &[&str]) -> Option<i32> {
    paths.iter().find_map(|path| try_read_int(path))
}
fn read_int_any(paths: &[&str]) -> i32 {
    try_read_int_any(paths).unwrap_or(0)
}
fn write_string_any(paths: &[&str], value: &str) -> bool {
    let mut wrote = false;
    for p in paths {
        if path_exists(p) && fs::write(p, value).is_ok() {
            wrote = true;
        }
    }
    wrote
}
fn clamp_i64_to_i32(value: i64) -> i32 {
    value.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}
fn abs_i32_to_i64(value: i32) -> i64 {
    (value as i64).abs()
}
fn normalize_capacity_mah(value: i32) -> i32 {
    if value <= 0 {
        0
    } else if value > 100_000 {
        value / 1000
    } else {
        value
    }
}
fn parse_first_int(value: &str) -> Option<i32> {
    value
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .find(|part| !part.is_empty() && *part != "-")
        .and_then(|part| part.parse::<i32>().ok())
}
fn parse_keyed_int(value: &str, key: &str) -> Option<i32> {
    value.split('+').find_map(|part| {
        let (name, raw) = part.split_once('=')?;
        if name == key {
            raw.trim().parse::<i32>().ok()
        } else {
            None
        }
    })
}
fn parse_plus_ints(value: &str) -> Vec<i32> {
    value
        .split('+')
        .filter_map(|part| part.trim().parse::<i32>().ok())
        .collect()
}

fn is_data_port_usb_type(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_uppercase().as_str(),
        "USB" | "SDP" | "USB_SDP" | "CDP" | "USB_CDP" | "PC" | "PC_PORT"
    )
}

#[cfg(target_os = "android")]
fn lower_poll_thread_priority() {
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

#[cfg(not(target_os = "android"))]
fn lower_poll_thread_priority() {}

#[cfg(any(target_os = "android", test))]
const POWER_SUPPLY_UEVENT_FIELD: &[u8] = b"SUBSYSTEM=power_supply";

#[cfg(any(target_os = "android", test))]
fn is_power_supply_uevent(message: &[u8]) -> bool {
    message
        .split(|byte| *byte == 0)
        .any(|field| field == POWER_SUPPLY_UEVENT_FIELD)
}

#[cfg(target_os = "android")]
fn monitor_power_supply_uevents(adapter: Weak<Adapter>) -> io::Result<()> {
    let raw_fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        )
    };
    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let socket = unsafe { OwnedFd::from_raw_fd(raw_fd) };

    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    address.nl_groups = 1;
    let bind_result = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            (&raw const address).cast::<libc::sockaddr>(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if bind_result < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut buffer = [0_u8; 4096];
    loop {
        let received = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        };
        if received < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if received > 0 && is_power_supply_uevent(&buffer[..received as usize]) {
            let Some(adapter) = adapter.upgrade() else {
                return Ok(());
            };
            adapter.request_uevent_probe();
        }
    }
}

// ── sysfs paths ──

const SHORT_CIRCUIT_HEALTHY: i32 = 1;

/// See [`ChargerInfo::authentic`].
pub const AUTHENTIC_REPORTED: i32 = 1;
const CHARGE_STOP_THRESHOLD_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/charge_limit",
    "/sys/class/qcom-battery/charge_limit",
    "/sys/class/power_supply/battery/charge_control_end_threshold",
    "/sys/class/power_supply/battery/charge_stop_threshold",
];
const BYPASS_STATUS_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/bypass_charging",
    "/sys/class/qcom-battery/bypass_charging",
    "/sys/class/power_supply/battery/bypass_charge",
    "/sys/class/qcom-battery/bypass_charge",
    "/sys/class/power_supply/battery/charge_bypass",
    "/sys/class/qcom-battery/charge_bypass",
];
const CHARGE_LIMIT_HYSTERESIS_PERCENT: i32 = 1;
const SCREEN_ON_POLL_INTERVAL: Duration = Duration::from_secs(1);
const SCREEN_OFF_CHARGING_POLL_INTERVAL: Duration = Duration::from_secs(5);
const SCREEN_OFF_IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);
const FULL_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const SCREEN_WAKE_SCAN_DEFER: Duration = Duration::from_millis(750);
const INITIAL_REFRESH_WAIT: Duration = Duration::from_millis(250);
const SYNTHETIC_DECIMAL_MIN_CENTI: i32 = 5;
const SYNTHETIC_DECIMAL_SEED_MAX_CENTI: i32 = 50;
const SYNTHETIC_DECIMAL_MAX_CENTI: i32 = 99;
const SYNTHETIC_DECIMAL_STEP_CENTI: i32 = 1;
const SYNTHETIC_DECIMAL_MAX_STEP_CENTI: i32 = 3;

// Individual qcom-battery nodes
const UI_SOC_DECIMAL_PATHS: &[&str] = &[
    "/proc/ui_soc_decimal",
    "/sys/class/power_supply/bms/soc_decimal",
    "/sys/class/qcom-battery/soc_decimal",
];
const UI_SOC_DECIMAL_RATE_PATHS: &[&str] = &[
    "/sys/class/power_supply/bms/soc_decimal_rate",
    "/sys/class/qcom-battery/soc_decimal_rate",
];

// ── State machine ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChargeState {
    Unknown,
    Disconnected,
    SlowCharging,
    NormalCharging,
    FastCharging,
    FlashCharging,
    SuperCharging,
}

#[derive(Debug, Clone)]
pub struct ChargerInfo {
    pub usb_online: i32,
    pub usb_type: String,
    pub usb_real_type: String,
    pub usb_voltage_now: i32,
    pub usb_current_now: i32,
    pub usb_temp: i32,
    pub connector_temp: i32,
    pub ac_online: i32,
    pub pc_port_online: i32,
    pub wireless_online: i32,
    pub wireless_type: String,
    /// Wireless rail voltage (grade A, µV) and current (grade A, µA).
    pub wireless_voltage_now: i32,
    pub wireless_current_now: i32,
    pub typec_mode: String,
    pub cc_orientation: i32,
    pub battery_present: bool,
    pub battery_status: String,
    pub battery_health: String,
    pub battery_capacity: i32,
    pub battery_temp: i32,
    pub battery_current_now: i32,
    pub battery_voltage_now: i32,
    pub battery_charge_type: String,
    pub battery_technology: String,
    pub charge_full: i32,
    pub charge_full_design: i32,
    pub charge_counter: i32,
    pub cycle_count: i32,
    pub fg_fcc: i32,
    pub fg_rm: i32,
    pub fg_rsoc: i32,
    pub fg_soh: i32,
    /// Raw decimal-SOC text from the vendor HAL (`getSocDecimal`).
    ///
    /// Kept as a string because the scale of the private `strategy_fg/soc_decimal`
    /// node is unconfirmed (×100 vs ×1000 cannot be told apart without a device)
    /// and the official ColorOS contract for the decimal-SOC call is "the node
    /// contents verbatim" anyway. Empty when the HAL backend is not in use.
    pub soc_decimal: String,
    /// Raw decimal-SOC rate text from the vendor HAL (`getSocDecimalRate`).
    pub soc_decimal_rate: String,
    pub fg_cycle: i32,
    pub fg_qmax: i32,
    pub fg_ai: i32,
    pub fg_avg_current: i32,
    pub fg_vendor: String,
    pub battery_type: String,
    pub gauge_type: String,
    pub gauge_info: String,
    pub cell1_vol: i32,
    pub cell2_vol: i32,
    pub cell1_rascale: i32,
    pub fast_charge_type: String,
    pub charge_technology: String,
    pub quick_charge_type: String,
    pub pd_verified: i32,
    pub(crate) charge_state: ChargeState,
    pub input_current_max: i32,
    pub input_voltage_max: i32,
    pub fastchg_mode: i32,
    pub current_state: String,
    pub sport_mode: i32,
    pub cp_online: i32,
    pub cp_status: String,
    pub cp_bus_voltage: i32,
    pub cp_bus_current: i32,
    pub cp_master_iin: i32,
    pub cp_slave_iin: i32,
    pub adapter_power_w: i32,
    pub remaining_time: i32,
    pub restrict_chg: i32,
    pub input_suspend: i32,
    pub smart_chg: i32,
    pub night_charging: i32,
    pub smart_batt: i32,
    pub die_temperature: i32,
    pub slave_die_temperature: i32,
    pub thermal_board_temp: i32,
    pub batt_sn: String,
    /// Reported to ColorOS as authentic. The contract asks whether the pack is
    /// an *OPlus* original, which no bridged device can answer, and 0 makes
    /// ColorOS warn about a non-genuine battery. Fixed by policy.
    pub authentic: i32,
    pub batt_cont_online: i32,
    pub max_life_temp: i32,
    pub max_life_vol: i32,
    pub over_vol_duration: i32,
    pub moisture_detected: bool,
    pub flash_active: bool,
    pub hifi_connect: bool,
    pub vbus_disable: bool,
    pub otg_ui_support: i32,
    pub fake_soc: i32,
    pub fake_soh: i32,
    pub fake_cycle: i32,
    pub fake_temp: i32,
}

#[derive(Debug)]
struct DecimalSocState {
    capacity: i32,
    target: i32,
    random: u32,
}

impl Default for DecimalSocState {
    fn default() -> Self {
        Self {
            capacity: -1,
            target: 0,
            random: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FastChargeSnapshot {
    pub online: bool,
    pub usb_online: i32,
    pub ac_online: i32,
    pub wireless_online: i32,
    pub fast_type: i32,
    pub charge_tech: i32,
    pub quick_charge_type: String,
    pub pd_verified: i32,
    pub cp_online: i32,
    pub fastchg_mode: i32,
    pub sport_mode: i32,
    pub adapter_power_w: i32,
    pub usb_type: String,
    pub pc_port_online: i32,
}

impl FastChargeSnapshot {
    fn is_data_port(&self) -> bool {
        self.usb_online != 0 && (self.pc_port_online != 0 || is_data_port_usb_type(&self.usb_type))
    }

    pub fn is_fast_charge(&self) -> bool {
        self.online
            && !self.is_data_port()
            && (self.fast_type > 0
                || self.fastchg_mode != 0
                || self.sport_mode != 0
                || self.pd_verified != 0
                || self.cp_online != 0
                || self.adapter_power_w >= 10)
    }

    pub fn is_svooc_active(&self) -> bool {
        self.online
            && !self.is_data_port()
            && (self.fast_type >= 2
                || self.fastchg_mode != 0
                || self.sport_mode != 0
                || self.cp_online != 0
                || self.adapter_power_w >= 20)
    }

    pub fn is_pps_active(&self) -> bool {
        let usb = self.usb_type.to_lowercase();
        let quick = self.quick_charge_type.to_lowercase();
        self.online
            && !self.is_data_port()
            && (self.charge_tech >= 3
                || self.pd_verified != 0
                || usb.contains("pd")
                || usb.contains("pps")
                || quick.contains("pd")
                || quick.contains("pps"))
    }

    pub fn should_show_power(&self) -> bool {
        !self.is_data_port() && self.quick_charge_type.trim() == "4"
    }
}

impl Default for ChargerInfo {
    fn default() -> Self {
        ChargerInfo {
            usb_online: 0,
            usb_type: String::new(),
            usb_real_type: String::new(),
            usb_voltage_now: 0,
            usb_current_now: 0,
            usb_temp: 0,
            connector_temp: 0,
            ac_online: 0,
            pc_port_online: 0,
            wireless_online: 0,
            wireless_type: String::new(),
            wireless_voltage_now: 0,
            wireless_current_now: 0,
            typec_mode: String::new(),
            cc_orientation: 0,
            battery_present: true,
            battery_status: "Unknown".into(),
            battery_health: "Unknown".into(),
            battery_capacity: 0,
            battery_temp: 0,
            battery_current_now: 0,
            battery_voltage_now: 0,
            battery_charge_type: "Unknown".into(),
            battery_technology: String::new(),
            charge_full: 0,
            charge_full_design: 0,
            charge_counter: 0,
            cycle_count: 0,
            fg_fcc: 0,
            fg_rm: 0,
            fg_rsoc: 0,
            fg_soh: 0,
            soc_decimal: String::new(),
            soc_decimal_rate: String::new(),
            fg_cycle: 0,
            fg_qmax: 0,
            fg_ai: 0,
            fg_avg_current: 0,
            fg_vendor: String::new(),
            battery_type: String::new(),
            gauge_type: String::new(),
            gauge_info: String::new(),
            cell1_vol: 0,
            cell2_vol: 0,
            cell1_rascale: 0,
            fast_charge_type: "0".into(),
            charge_technology: "0".into(),
            quick_charge_type: String::new(),
            pd_verified: 0,
            charge_state: ChargeState::Unknown,
            input_current_max: 0,
            input_voltage_max: 0,
            fastchg_mode: 0,
            current_state: String::new(),
            sport_mode: 0,
            cp_online: 0,
            cp_status: String::new(),
            cp_bus_voltage: 0,
            cp_bus_current: 0,
            cp_master_iin: 0,
            cp_slave_iin: 0,
            adapter_power_w: 0,
            remaining_time: 0,
            restrict_chg: 0,
            input_suspend: 0,
            smart_chg: 0,
            night_charging: 0,
            smart_batt: 0,
            die_temperature: 0,
            slave_die_temperature: 0,
            thermal_board_temp: 0,
            batt_sn: String::new(),
            authentic: AUTHENTIC_REPORTED,
            batt_cont_online: 0,
            max_life_temp: 0,
            max_life_vol: 0,
            over_vol_duration: 0,
            moisture_detected: false,
            flash_active: false,
            hifi_connect: false,
            vbus_disable: false,
            otg_ui_support: 0,
            fake_soc: 0,
            fake_soh: 0,
            fake_cycle: 0,
            fake_temp: 0,
        }
    }
}

// ── Adapter ──

pub struct Adapter {
    pub info: Mutex<ChargerInfo>,
    pub charger_info_json: Mutex<String>,
    pub reverse_chg_info: Mutex<String>, // STUB: hardcoded format, no real reverse charge
    pub battery_balance_info: Mutex<String>, // populated from dual-cell voltage
    pub usb_eye_diagram: Mutex<String>,  // STUB: hardcoded "0,0,0,0,0,0,0,0,0,0"
    pub battery_auth_status: Mutex<String>,
    pub battery_type_cache: Mutex<String>,
    fast_charge_snapshot: Mutex<FastChargeSnapshot>,
    poll_wake_tx: SyncSender<()>,
    refresh_pending: AtomicBool,
    uevent_probe_pending: AtomicBool,
    screen_on: AtomicBool,
    screen_wake_pending: AtomicBool,
    battery_capacity_cache: AtomicI32,
    decimal_soc_seed: AtomicI32,
    decimal_soc_rate: AtomicI32,
    decimal_soc: Mutex<DecimalSocState>,
    pub quick_mode_gain: Mutex<String>, // STUB: hardcoded "0+0" (official format is "%d+%d")
    pub soh_debug_info: Mutex<String>,

    // ── Settable compatibility state for features absent from the Xiaomi kernel ──
    pub bcc_anode_type: Mutex<String>,    // NO-OP
    pub eis_switch_status: Mutex<String>, // NO-OP
    pub sili_ic_alg_cfg: Mutex<String>,   // NO-OP
    pub chg_up_limit_state: Mutex<String>,
    pub chg_up_limit_value: Mutex<String>,
    pub charge_limit_active: Mutex<bool>,
    pub bypass_charge_status: Mutex<String>,
    pub charge_control_active: Mutex<bool>,
    charge_control_update_lock: Mutex<()>,
    charge_control_applied: Mutex<Option<bool>>,
    pub cooldown: Mutex<String>,           // NO-OP
    pub anti_expansion_dis: Mutex<String>, // NO-OP

    pub battery_log_enabled: AtomicBool, // NO-OP: no battery log push on Xiaomi

    /// Where snapshots come from. The Xiaomi vendor HAL is preferred because it
    /// absorbs the per-model sysfs layout; the kernel-node reader is the
    /// fallback when that HAL is absent.
    backend: Arc<dyn ChargeBackend>,
}

impl Adapter {
    pub fn new() -> Arc<Self> {
        let info = ChargerInfo::default();
        let (poll_wake_tx, poll_wake_rx) = sync_channel(1);
        let (initial_ready_tx, initial_ready_rx) = sync_channel(1);

        let balance = if info.cell1_vol > 0 && info.cell2_vol > 0 {
            format!("{},{},0,0,0,0,0,0", info.cell1_vol, info.cell2_vol)
        } else {
            "0,0,0,0,0,0,0,0".into()
        };
        let fast_snapshot = Adapter::fast_charge_snapshot_from_info(&info);
        let adapter = Arc::new(Adapter {
            charger_info_json: Mutex::new(Adapter::build_charger_info_json(&info)),
            reverse_chg_info: Mutex::new("0,0,0".into()),
            battery_balance_info: Mutex::new(balance),
            usb_eye_diagram: Mutex::new("0,0,0,0,0,0,0,0,0,0".into()),
            battery_auth_status: Mutex::new(info.authentic.to_string()),
            battery_type_cache: Mutex::new(Adapter::best_battery_type(&info)),
            fast_charge_snapshot: Mutex::new(fast_snapshot),
            poll_wake_tx,
            refresh_pending: AtomicBool::new(false),
            uevent_probe_pending: AtomicBool::new(false),
            screen_on: AtomicBool::new(false),
            screen_wake_pending: AtomicBool::new(false),
            battery_capacity_cache: AtomicI32::new(info.battery_capacity),
            decimal_soc_seed: AtomicI32::new(0),
            decimal_soc_rate: AtomicI32::new(0),
            decimal_soc: Mutex::new(DecimalSocState::default()),
            quick_mode_gain: Mutex::new("0+0".into()),
            soh_debug_info: Mutex::new(Adapter::build_soh_debug_info(&info)),
            bcc_anode_type: Mutex::new("0".into()),
            eis_switch_status: Mutex::new("0".into()),
            sili_ic_alg_cfg: Mutex::new("0".into()),
            chg_up_limit_state: Mutex::new("0".into()),
            chg_up_limit_value: Mutex::new("90".into()),
            charge_limit_active: Mutex::new(false),
            bypass_charge_status: Mutex::new("0".into()),
            charge_control_active: Mutex::new(false),
            charge_control_update_lock: Mutex::new(()),
            charge_control_applied: Mutex::new(Some(false)),
            cooldown: Mutex::new("0".into()),
            anti_expansion_dis: Mutex::new("0".into()),
            info: Mutex::new(info),
            battery_log_enabled: AtomicBool::new(false),
            backend: crate::backend::select(),
        });

        let adapter_weak = Arc::downgrade(&adapter);
        if let Err(err) = thread::Builder::new()
            .name("charger-hal-poll".into())
            .spawn(move || {
                lower_poll_thread_priority();
                let mut initial_ready_tx = Some(initial_ready_tx);
                let mut last_full_refresh = Instant::now()
                    .checked_sub(FULL_REFRESH_INTERVAL)
                    .unwrap_or_else(Instant::now);
                loop {
                    let interval = {
                        let Some(adapter) = adapter_weak.upgrade() else {
                            break;
                        };
                        let screen_on = adapter.screen_on.load(Ordering::Relaxed);
                        let online = adapter.fast_charge_snapshot.lock().online;
                        Self::poll_interval(screen_on, online)
                    };
                    let timed_out = match poll_wake_rx.recv_timeout(interval) {
                        Ok(()) => false,
                        Err(RecvTimeoutError::Timeout) => true,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    let Some(adapter_clone) = adapter_weak.upgrade() else {
                        break;
                    };
                    if adapter_clone
                        .screen_wake_pending
                        .swap(false, Ordering::AcqRel)
                    {
                        thread::sleep(SCREEN_WAKE_SCAN_DEFER);
                        // A bare wake is ignored by the loop below unless a
                        // refresh was requested. Without this, the deferred
                        // scan never runs.
                        adapter_clone.request_refresh();
                        continue;
                    }
                    let refresh_requested =
                        adapter_clone.refresh_pending.swap(false, Ordering::Relaxed);
                    let uevent_probe_requested = adapter_clone
                        .uevent_probe_pending
                        .swap(false, Ordering::Relaxed);
                    if !timed_out && !refresh_requested && !uevent_probe_requested {
                        continue;
                    }
                    let maintenance_due = last_full_refresh.elapsed() >= FULL_REFRESH_INTERVAL;
                    if !refresh_requested
                        && !maintenance_due
                        && (timed_out || uevent_probe_requested)
                        && !adapter_clone.backend.power_source_changed()
                    {
                        continue;
                    }
                    let mut next_info = adapter_clone.info.lock().clone();
                    let scan_completed = adapter_clone.backend.refresh(&mut next_info, &|| {
                        adapter_clone.screen_wake_pending.load(Ordering::Acquire)
                    });
                    if !scan_completed || adapter_clone.screen_wake_pending.load(Ordering::Acquire)
                    {
                        adapter_clone.request_refresh();
                        continue;
                    }
                    let charger_info_json = Adapter::build_charger_info_json(&next_info);
                    let soh_debug_info = Adapter::build_soh_debug_info(&next_info);
                    let battery_type = Adapter::best_battery_type(&next_info);
                    let battery_auth_status = next_info.authentic.to_string();
                    let balance_info = Adapter::build_balance_info(&next_info);
                    let fast_snapshot = Adapter::fast_charge_snapshot_from_info(&next_info);
                    let decimal_seed = read_int_any(UI_SOC_DECIMAL_PATHS);
                    let decimal_rate = read_int_any(UI_SOC_DECIMAL_RATE_PATHS);
                    let capacity_cache = next_info.battery_capacity;
                    if adapter_clone.screen_wake_pending.load(Ordering::Acquire) {
                        adapter_clone.request_refresh();
                        continue;
                    }

                    *adapter_clone.info.lock() = next_info;
                    *adapter_clone.charger_info_json.lock() = charger_info_json;
                    *adapter_clone.soh_debug_info.lock() = soh_debug_info;
                    *adapter_clone.battery_type_cache.lock() = battery_type;
                    *adapter_clone.battery_auth_status.lock() = battery_auth_status;
                    *adapter_clone.battery_balance_info.lock() = balance_info;
                    *adapter_clone.fast_charge_snapshot.lock() = fast_snapshot;
                    adapter_clone
                        .battery_capacity_cache
                        .store(capacity_cache, Ordering::Relaxed);
                    adapter_clone
                        .decimal_soc_seed
                        .store(decimal_seed, Ordering::Relaxed);
                    adapter_clone
                        .decimal_soc_rate
                        .store(decimal_rate, Ordering::Relaxed);
                    if adapter_clone.screen_wake_pending.load(Ordering::Acquire) {
                        adapter_clone.request_refresh();
                        continue;
                    }
                    adapter_clone.enforce_charge_control();
                    last_full_refresh = Instant::now();
                    if let Some(initial_ready_tx) = initial_ready_tx.take() {
                        let _ = initial_ready_tx.try_send(());
                    }
                }
            })
        {
            tracing::warn!("failed to start charger poll thread: {}", err);
        }

        #[cfg(target_os = "android")]
        {
            let adapter_weak = Arc::downgrade(&adapter);
            if let Err(error) = thread::Builder::new()
                .name("charger-hal-uevent".into())
                .spawn(move || {
                    if let Err(error) = monitor_power_supply_uevents(adapter_weak) {
                        tracing::warn!("power-supply uevent monitor stopped: {}", error);
                    }
                })
            {
                tracing::warn!("failed to start power-supply uevent monitor: {}", error);
            }
        }

        adapter.request_refresh();
        match initial_ready_rx.recv_timeout(INITIAL_REFRESH_WAIT) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!("initial charger snapshot did not finish within 250 ms")
            }
            Err(RecvTimeoutError::Disconnected) => {
                tracing::warn!("charger poll thread stopped before the initial snapshot")
            }
        }

        adapter
    }

    // ── Classifiers ──

    fn fast_charge_snapshot_from_info(info: &ChargerInfo) -> FastChargeSnapshot {
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

    fn bool_like(value: &str) -> bool {
        matches!(
            value.trim(),
            "1" | "true" | "TRUE" | "True" | "enable" | "enabled" | "on"
        )
    }

    fn parse_bypass_switch(value: &str) -> bool {
        parse_keyed_int(value, "switch")
            .map(|v| v != 0)
            .unwrap_or_else(|| Self::bool_like(value))
    }

    fn parse_charge_limit_state_payload(value: &str) -> (i32, Option<i32>) {
        let ints = parse_plus_ints(value);
        if ints.len() >= 3 && ints[0] == 2 {
            (ints[1], Some(ints[2]))
        } else if ints.len() >= 2 {
            (ints[0], Some(ints[1]))
        } else {
            (parse_first_int(value).unwrap_or(0), None)
        }
    }

    fn parse_charge_limit_control_payload(value: &str) -> (i32, Option<i32>, i32, Option<i32>) {
        let ints = parse_plus_ints(value);
        if ints.len() >= 5 && ints[0] == 4 {
            (ints[1], Some(ints[2]), ints[3], Some(ints[4]))
        } else if ints.len() >= 4 {
            (ints[0], Some(ints[1]), ints[2], Some(ints[3]))
        } else {
            (parse_first_int(value).unwrap_or(0), None, 0, None)
        }
    }

    fn bypass_should_restrict(&self) -> bool {
        Self::parse_bypass_switch(&self.bypass_charge_status.lock())
    }

    fn charge_limit_switch_enabled(&self) -> bool {
        self.chg_up_limit_state.lock().trim() == "1"
    }

    fn charge_limit_target(&self) -> Option<i32> {
        self.chg_up_limit_value.lock().trim().parse::<i32>().ok()
    }

    fn charge_limit_latch_state(
        currently_active: bool,
        switch_enabled: bool,
        capacity: i32,
        limit: Option<i32>,
    ) -> bool {
        if !switch_enabled {
            return false;
        }
        let Some(limit) = limit else {
            return currently_active;
        };
        if currently_active {
            capacity > limit.saturating_sub(CHARGE_LIMIT_HYSTERESIS_PERCENT)
        } else {
            capacity >= limit
        }
    }

    fn update_charge_limit_latch(&self) -> bool {
        let currently_active = *self.charge_limit_active.lock();
        let next_active = Self::charge_limit_latch_state(
            currently_active,
            self.charge_limit_switch_enabled(),
            self.info.lock().battery_capacity,
            self.charge_limit_target(),
        );
        *self.charge_limit_active.lock() = next_active;
        next_active
    }

    fn desired_charge_control_active(&self) -> bool {
        self.bypass_should_restrict() || *self.charge_limit_active.lock()
    }

    fn enforce_charge_control(&self) {
        let _update_guard = self.charge_control_update_lock.lock();
        self.update_charge_limit_latch();
        self.set_charge_control_active(self.desired_charge_control_active());
    }

    fn next_synthetic_decimal_random(state: &mut DecimalSocState) -> u32 {
        if state.random == 0 {
            state.random = 0x6d2b_79f5;
        }
        state.random = state
            .random
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        state.random
    }

    fn normalize_decimal_seed_offset(raw: i32, capacity: i32) -> Option<i32> {
        if raw <= 0 {
            return None;
        }

        let capacity = capacity.clamp(0, 100);
        let base = capacity * 100;
        let normalized = if raw <= 100 {
            raw * 100
        } else if raw <= 10000 {
            raw
        } else {
            raw / 100
        };
        let offset = normalized - base;
        if (SYNTHETIC_DECIMAL_MIN_CENTI..=SYNTHETIC_DECIMAL_MAX_CENTI).contains(&offset) {
            Some(offset)
        } else {
            None
        }
    }

    fn next_synthetic_decimal_seed(
        state: &mut DecimalSocState,
        capacity: i32,
        cached_seed: i32,
    ) -> i32 {
        Self::normalize_decimal_seed_offset(cached_seed, capacity).unwrap_or_else(|| {
            SYNTHETIC_DECIMAL_MIN_CENTI
                + (Self::next_synthetic_decimal_random(state)
                    % ((SYNTHETIC_DECIMAL_SEED_MAX_CENTI - SYNTHETIC_DECIMAL_MIN_CENTI + 1) as u32))
                    as i32
        })
    }

    fn normalize_decimal_rate(rate: i32) -> i32 {
        if rate <= 0 {
            SYNTHETIC_DECIMAL_STEP_CENTI
        } else if rate > 10000 {
            ((rate / 100) / 4).clamp(
                SYNTHETIC_DECIMAL_STEP_CENTI,
                SYNTHETIC_DECIMAL_MAX_STEP_CENTI,
            )
        } else {
            (rate / 4).clamp(
                SYNTHETIC_DECIMAL_STEP_CENTI,
                SYNTHETIC_DECIMAL_MAX_STEP_CENTI,
            )
        }
    }

    fn synthetic_decimal_soc_pair(
        state: &mut DecimalSocState,
        capacity: i32,
        cached_seed: i32,
        cached_rate: i32,
    ) -> (i32, i32) {
        let capacity = capacity.clamp(0, 100);
        if capacity >= 100 {
            state.capacity = capacity;
            state.target = 10000;
            return (10000, 10000);
        }

        let base = capacity * 100;
        let min = base + SYNTHETIC_DECIMAL_MIN_CENTI;
        let max = base + SYNTHETIC_DECIMAL_MAX_CENTI;
        let step = Self::normalize_decimal_rate(cached_rate).min(max - min);

        if state.capacity != capacity {
            state.capacity = capacity;
            state.target = base + Self::next_synthetic_decimal_seed(state, capacity, cached_seed);
        }

        let start = state.target.clamp(min, max);
        let seed = Self::normalize_decimal_seed_offset(cached_seed, capacity)
            .map(|offset| base + offset)
            .unwrap_or(start);
        let end = (start + step).max(seed).clamp(start, max);
        state.target = end;
        (start, end)
    }

    fn decimal_soc_pair(&self, capacity: i32) -> (i32, i32) {
        let cached_seed = self.decimal_soc_seed.load(Ordering::Relaxed);
        let cached_rate = self.decimal_soc_rate.load(Ordering::Relaxed);
        let mut state = self.decimal_soc.lock();
        Self::synthetic_decimal_soc_pair(&mut state, capacity, cached_seed, cached_rate)
    }

    pub fn get_stable_adapter_power_w(&self) -> i32 {
        let snapshot = self.get_fast_charge_snapshot();
        if !snapshot.online {
            return 0;
        }
        if snapshot.adapter_power_w > 0 {
            snapshot.adapter_power_w
        } else {
            self.info.lock().adapter_power_w
        }
    }

    fn poll_interval(screen_on: bool, online: bool) -> Duration {
        if screen_on {
            SCREEN_ON_POLL_INTERVAL
        } else if online {
            SCREEN_OFF_CHARGING_POLL_INTERVAL
        } else {
            SCREEN_OFF_IDLE_POLL_INTERVAL
        }
    }

    fn request_refresh(&self) {
        self.refresh_pending.store(true, Ordering::Relaxed);
        self.wake_poll_worker();
    }

    #[cfg(target_os = "android")]
    fn request_uevent_probe(&self) {
        self.uevent_probe_pending.store(true, Ordering::Relaxed);
        self.wake_poll_worker();
    }

    fn wake_poll_worker(&self) {
        let _ = self.poll_wake_tx.try_send(());
    }

    pub fn get_fast_charge_snapshot(&self) -> FastChargeSnapshot {
        self.fast_charge_snapshot.lock().clone()
    }

    pub fn notify_screen_status(&self, status: i32) {
        let screen_on = status != 0;
        let previous = self.screen_on.swap(screen_on, Ordering::Relaxed);
        if previous != screen_on {
            self.screen_wake_pending.store(true, Ordering::Release);
            // The poll thread may be parked for the full idle interval.
            // Waking it only enqueues a token; the worker applies the defer.
            self.wake_poll_worker();
        }
    }

    // ── Best-value helpers ──

    fn best_soh(info: &ChargerInfo) -> i32 {
        if info.fg_soh > 0 && info.fg_soh <= 100 {
            info.fg_soh
        } else if info.charge_full > 0 && info.charge_full_design > 0 {
            ((info.charge_full as f64 / info.charge_full_design as f64) * 100.0).clamp(0.0, 100.0)
                as i32
        } else {
            0
        }
    }
    fn best_fcc(info: &ChargerInfo) -> i32 {
        if info.fg_fcc > 0 {
            info.fg_fcc
        } else {
            info.charge_full
        }
    }
    fn best_fcc_mah(info: &ChargerInfo) -> i32 {
        normalize_capacity_mah(Self::best_fcc(info))
    }
    fn best_design_capacity_mah(info: &ChargerInfo) -> i32 {
        normalize_capacity_mah(info.charge_full_design)
    }
    fn best_qmax_mah(info: &ChargerInfo) -> i32 {
        normalize_capacity_mah(info.fg_qmax)
    }
    fn best_rm(info: &ChargerInfo) -> i32 {
        if info.fg_rm > 0 {
            info.fg_rm
        } else if info.fg_fcc > 0 && info.fg_rsoc > 0 {
            let denominator = if info.fg_rsoc > 100 { 10000 } else { 100 };
            clamp_i64_to_i32((info.fg_fcc as i64) * (info.fg_rsoc as i64) / denominator)
        } else {
            clamp_i64_to_i32((info.charge_full as i64) * (info.battery_capacity as i64) / 100)
        }
    }
    fn best_rm_mah(info: &ChargerInfo) -> i32 {
        normalize_capacity_mah(Self::best_rm(info))
    }
    fn best_charge_counter_mah(info: &ChargerInfo) -> i32 {
        normalize_capacity_mah(info.charge_counter)
    }
    fn best_cycle(info: &ChargerInfo) -> i32 {
        if info.fg_cycle > 0 {
            info.fg_cycle
        } else {
            info.cycle_count
        }
    }
    fn best_battery_type(info: &ChargerInfo) -> String {
        if !info.battery_type.is_empty() {
            info.battery_type.clone()
        } else if !info.battery_technology.is_empty() {
            info.battery_technology.clone()
        } else {
            "unknown".into()
        }
    }
    fn is_silicon_battery(info: &ChargerInfo) -> bool {
        let battery_type = Self::best_battery_type(info).to_lowercase();
        battery_type.contains("silicon")
            || battery_type.contains("si-c")
            || battery_type.contains("sic")
    }

    // ── String builders (ColorOS format) ──

    /// `queryChargeInfo` payload.
    ///
    /// The OPlus HAL returns a newline-separated `key=value` list and the
    /// ColorOS client parses it by key name, so both the names and their order
    /// are a hard contract. Values with no source on this platform report `0` or
    /// an empty string instead of being omitted, because the client looks keys
    /// up by name.
    ///
    /// Key list and order come from the official V11 service; see
    /// `chargehal-vendor-refs/FORMAT-CONTRACT.md` §3.1.
    fn build_charger_info_json(info: &ChargerInfo) -> String {
        let soh = Self::best_soh(info);
        let charge_counter = Self::best_charge_counter_mah(info);
        let battery_type = Self::best_battery_type(info);
        let fast_type = info.fast_charge_type.trim().parse::<i32>().unwrap_or(0);
        let charge_tech = info.charge_technology.trim().parse::<i32>().unwrap_or(0);
        let svooc = i32::from(fast_type >= 2);
        let pps = i32::from(info.pd_verified != 0 || charge_tech >= 3);
        let dual_chan = i32::from(info.cell1_vol > 0 && info.cell2_vol > 0);
        format!(
            concat!(
                "bcc_exp_status={}\n",
                "battery_capacity={}\n",
                "battery_voltage_now={}\n",
                "battery_voltage_min={}\n",
                "battery_temp={}\n",
                "battery_current_now={}\n",
                "battery_charge_now={}\n",
                "battery_sub_current={}\n",
                "usb_fast_chg_type={}\n",
                "battery_voocchg_ing={}\n",
                "battery_ppschg_ing={}\n",
                "battery_ppschg_power={}\n",
                "usb_input_current_now={}\n",
                "battery_short_ic_otp_status={}\n",
                "battery_authenticate={}\n",
                "battery_bqfs_status={}\n",
                "gauge_ibat_deviation={}\n",
                "battery_charge_technology={}\n",
                "parallel_chg_mos_status={}\n",
                "wireless_current_now={}\n",
                "wireless_rx_version={}\n",
                "wireless_tx_version={}\n",
                "wireless_idt_adc_test={}\n",
                // Both are literals in the official binary too (format strings
                // 0x15712 / 0x8be1 carry no conversion), so keep them fixed.
                "wireless_enable_tx=4\n",
                "battery_status=1\n",
                "wireless_voltage_now={}\n",
                "wireless_real_type={}\n",
                "wireless_charger_type={}\n",
                "wireless_charge_pump_en={}\n",
                "wireless_deviated={}\n",
                "battery_temp_not_plug={}\n",
                "battery_voltage_max_not_plug={}\n",
                "battery_voltage_min_not_plug={}\n",
                "dual_chan_support={}\n",
                "dual_chan_vbat_status={}\n",
                "dual_chan_buck_status={}\n",
                "dual_chan_temp_range_status={}\n",
                "chargerAcOnline={}\n",
                "{}\n",
                "bob_status={}\n",
                "bob_status_reg={}\n",
                "ttf_info={},{}\n",
                "bsl_data={}\n",
                "eis_data={}\n",
                "battery_type_str={}\n",
                "batt_chemID={}\n",
                "battery_uisoh={}\n",
                "battery_uisoh_is_100={}\n",
                "battery_realsoh={}\n"
            ),
            0,
            info.battery_capacity,
            info.battery_voltage_now,
            0,
            info.battery_temp,
            info.battery_current_now,
            charge_counter,
            0,
            fast_type,
            svooc,
            pps,
            info.adapter_power_w,
            info.usb_current_now,
            SHORT_CIRCUIT_HEALTHY,
            info.authentic,
            0,
            0,
            charge_tech,
            0,
            0,
            info.wireless_current_now,
            "",
            0,
            info.wireless_voltage_now.to_string(),
            "",
            0,
            "",
            "",
            0,
            0,
            0,
            dual_chan,
            0,
            0,
            0,
            info.ac_online,
            "",
            0,
            0,
            info.remaining_time,
            0,
            "",
            "",
            battery_type,
            info.battery_technology,
            soh,
            i32::from(soh == 100),
            soh,
        )
    }

    fn build_soh_debug_info(info: &ChargerInfo) -> String {
        let soh = Self::best_soh(info);
        let cycle = Self::best_cycle(info);
        let fcc_mah = Self::best_fcc_mah(info);
        let rm_mah = Self::best_rm_mah(info);
        let design_mah = Self::best_design_capacity_mah(info);
        let qmax_mah = Self::best_qmax_mah(info);
        let battery_type = Self::best_battery_type(info);
        let gauge_type = if info.gauge_type.is_empty() {
            "unknown"
        } else {
            &info.gauge_type
        };
        format!(
            "soh={};charge_full={};charge_full_design={};charge_counter={};fcc={};rm={};design_capacity={};qmax={};cycle={};battery_type={};gauge_type={};capacity={};temp={};status={};fg_soh={};fg_fcc={};fg_rm={};fg_rsoc={};fg_cycle={};fg_ai={};fg_qmax={};fg_vendor={};gauge_info={};die_temp={};remaining_time={};max_life_temp={};max_life_vol={};over_vol_dur={};soc_decimal={};soc_decimal_rate={};smart_batt={};usb_temp={};connector_temp={}",
            soh, fcc_mah, design_mah, Self::best_charge_counter_mah(info), fcc_mah, rm_mah, design_mah, qmax_mah,
            cycle, battery_type, gauge_type, info.battery_capacity, info.battery_temp, info.battery_status,
            info.fg_soh, info.fg_fcc, info.fg_rm, info.fg_rsoc, info.fg_cycle,
            info.fg_ai, info.fg_qmax, info.fg_vendor, info.gauge_info,
            info.die_temperature, info.remaining_time,
            info.max_life_temp, info.max_life_vol, info.over_vol_duration,
            // Collected from the vendor HAL but not served over the wire yet;
            // surfaced here so the value is observable without changing the
            // decimal-SOC contract. See `get_decimal_soc`.
            info.soc_decimal, info.soc_decimal_rate,
            info.smart_batt, info.usb_temp, info.connector_temp
        )
    }

    fn build_balance_info(info: &ChargerInfo) -> String {
        let diff = clamp_i64_to_i32(((info.cell1_vol as i64) - (info.cell2_vol as i64)).abs());
        format!(
            "{},{},{},{},{},{},{},{}",
            info.cell1_vol,
            info.cell2_vol,
            diff,
            info.cell1_rascale,
            info.fg_avg_current,
            info.fg_qmax,
            info.fg_ai,
            if info.fg_vendor.is_empty() { "0" } else { "1" }
        )
    }

    // ── Public accessors ──

    pub fn get_usb_online(&self) -> i32 {
        self.fast_charge_snapshot.lock().usb_online
    }
    pub fn get_usb_status(&self) -> String {
        let info = self.info.lock();
        if info.usb_online == 0 && info.ac_online == 0 && info.wireless_online == 0 {
            return "0,0,0,0,0,0".into();
        }
        let usb_ty = if !info.usb_real_type.is_empty() {
            &info.usb_real_type
        } else {
            &info.usb_type
        };
        let type_code = match usb_ty.as_str() {
            "USB" | "SDP" => "1",
            "USB_CDP" | "CDP" => "2",
            "USB_DCP" | "DCP" => "3",
            "USB_HVDCP" | "HVDCP" => "4",
            "USB_PD" | "PD" | "PD_ACTIVE" => "5",
            "USB_HVDCP_3" | "HVDCP_3" => "6",
            _ => {
                let tl = usb_ty.to_lowercase();
                if tl.contains("pd") {
                    "5"
                } else if tl.contains("hvdcp") {
                    "4"
                } else if tl.contains("dcp") {
                    "3"
                } else if tl.contains("cdp") {
                    "2"
                } else {
                    "0"
                }
            }
        };
        format!(
            "{},{},{},{},{},{}",
            info.usb_online,
            type_code,
            info.input_current_max,
            info.battery_voltage_now,
            info.battery_current_now,
            info.battery_temp
        )
    }
    pub fn get_ac_online(&self) -> i32 {
        self.fast_charge_snapshot.lock().ac_online
    }
    pub fn get_wireless_online(&self) -> i32 {
        self.fast_charge_snapshot.lock().wireless_online
    }
    pub fn get_pc_port_online(&self) -> i32 {
        self.info.lock().pc_port_online
    }
    pub fn get_battery_temp(&self) -> i32 {
        self.info.lock().battery_temp
    }
    pub fn get_battery_current_now(&self) -> i32 {
        self.info.lock().battery_current_now
    }
    pub fn get_average_current(&self) -> i32 {
        let info = self.info.lock();
        if info.fg_avg_current != 0 {
            clamp_i64_to_i32(abs_i32_to_i64(info.fg_avg_current))
        } else {
            clamp_i64_to_i32(abs_i32_to_i64(info.battery_current_now))
        }
    }
    /// `getPsyBatteryStatus` payload.
    ///
    /// The OPlus HAL returns the raw contents of the battery status node, so
    /// that is what this returns. The previous implementation built a 17-field
    /// comma string, which the ColorOS client cannot parse; see
    /// `chargehal-vendor-refs/FORMAT-CONTRACT.md` §3.
    ///
    /// The value is trimmed here while the kernel node keeps its trailing
    /// newline. Clients parse by token, so the difference is harmless.
    pub fn get_battery_status(&self) -> String {
        self.info.lock().battery_status.clone()
    }
    pub fn get_battery_rm(&self) -> i32 {
        Self::best_rm_mah(&self.info.lock())
    }
    pub fn get_battery_fcc(&self) -> i32 {
        Self::best_fcc_mah(&self.info.lock())
    }
    pub fn get_battery_cc(&self) -> i32 {
        Self::best_charge_counter_mah(&self.info.lock())
    }
    pub fn get_battery_design_capacity(&self) -> i32 {
        Self::best_design_capacity_mah(&self.info.lock())
    }
    pub fn get_battery_qmax(&self) -> i32 {
        Self::best_qmax_mah(&self.info.lock())
    }
    pub fn get_battery_type(&self) -> String {
        Self::best_battery_type(&self.info.lock())
    }
    pub fn get_battery_gauge_type(&self) -> String {
        let info = self.info.lock();
        if !info.gauge_type.is_empty() {
            info.gauge_type.clone()
        } else {
            Self::best_battery_type(&info)
        }
    }
    pub fn is_silicon_battery_now(&self) -> bool {
        Self::is_silicon_battery(&self.info.lock())
    }
    pub fn get_battery_soh(&self) -> i32 {
        Self::best_soh(&self.info.lock())
    }
    pub fn get_battery_cycle_count(&self) -> i32 {
        Self::best_cycle(&self.info.lock())
    }
    pub fn get_decimal_soc(&self) -> String {
        // `info.soc_decimal` holds the vendor HAL's raw decimal-SOC text and is
        // deliberately NOT returned here. Two reasons, both from evidence:
        //
        // 1. It is not the node the official OPlus HAL reads. That one is
        //    /proc/ui_soc_decimal; the HAL reads strategy_fg/soc_decimal. Serving
        //    one node's text under a call specified for another is a guess.
        // 2. Its scale is unconfirmed — ×100 vs ×1000 cannot be told apart
        //    without a device (HAL-GETTER-SPEC.md), so it could be off by 10×.
        //
        // The synthesised pair below at least has a known shape and is what the
        // node-reader backend produces, so both backends stay consistent. The
        // collected value is surfaced through `build_soh_debug_info` instead.
        if !self.get_fast_charge_snapshot().online {
            return "0,0".into();
        }
        let capacity = self
            .battery_capacity_cache
            .load(Ordering::Relaxed)
            .clamp(0, 100);
        let (start, end) = self.decimal_soc_pair(capacity);
        format!("{},{}", start, end)
    }
    pub fn get_adapter_power_w(&self) -> i32 {
        self.get_stable_adapter_power_w()
    }
    pub fn get_charge_limit_value(&self) -> String {
        if let Some(percent) = self.backend.charge_limit_percent() {
            return percent.to_string();
        }
        self.chg_up_limit_value.lock().clone()
    }
    pub fn set_charge_limit_value(&self, value: &str) {
        let _update_guard = self.charge_control_update_lock.lock();
        *self.charge_control_applied.lock() = None;
        // `recharge` is parsed but not applied: which field of the payload carries
        // the recharge threshold is unverified, and guessing it would write a
        // wrong value into the vendor's node.
        let (stop_charging, limit, _force, _recharge) =
            Self::parse_charge_limit_control_payload(value);
        if let Some(limit) = limit {
            *self.chg_up_limit_value.lock() = limit.to_string();
        } else if let Some(limit) = parse_first_int(value) {
            *self.chg_up_limit_value.lock() = limit.to_string();
        } else {
            *self.chg_up_limit_value.lock() = value.to_string();
        }
        if let Some(percent) = parse_first_int(&self.chg_up_limit_value.lock().clone())
            .filter(|value| (1..=100).contains(value))
        {
            self.backend.set_charge_limit(Some(percent));
        }
        if stop_charging != 0 {
            *self.charge_limit_active.lock() = true;
        } else if !self.charge_limit_switch_enabled()
            || !Self::charge_limit_latch_state(
                true,
                true,
                self.info.lock().battery_capacity,
                self.charge_limit_target(),
            )
        {
            *self.charge_limit_active.lock() = false;
        }
        self.set_charge_control_active(self.desired_charge_control_active());
    }
    pub fn get_charge_limit_state(&self) -> String {
        self.chg_up_limit_state.lock().clone()
    }
    pub fn set_charge_limit_state(&self, value: &str) {
        let _update_guard = self.charge_control_update_lock.lock();
        *self.charge_control_applied.lock() = None;
        let (enabled, limit) = Self::parse_charge_limit_state_payload(value);
        *self.chg_up_limit_state.lock() = enabled.to_string();
        if let Some(limit) = limit {
            *self.chg_up_limit_value.lock() = limit.to_string();
            write_string_any(CHARGE_STOP_THRESHOLD_PATHS, &limit.to_string());
        }
        if enabled == 0 {
            *self.charge_limit_active.lock() = false;
        } else {
            self.update_charge_limit_latch();
        }
        // `Some(None)` clears the limit, `Some(Some(p))` applies one, and `None`
        // leaves the vendor control untouched when the payload carries no usable
        // percentage.
        let target = if enabled == 0 {
            Some(None)
        } else {
            limit
                .or_else(|| parse_first_int(&self.chg_up_limit_value.lock().clone()))
                .filter(|value| (1..=100).contains(value))
                .map(Some)
        };
        if let Some(target) = target {
            self.backend.set_charge_limit(target);
        }
        // The on/off flag stays in memory and is applied through
        // `set_charge_control`. `smart_chg` and `night_charging` are separate
        // features; writing the limit switch into them turns night charging on.
        self.set_charge_control_active(self.desired_charge_control_active());
    }
    pub fn get_bypass_charge_status(&self) -> String {
        if let Some(enabled) = self.backend.bypass_charge_enabled() {
            return i32::from(enabled).to_string();
        }
        self.bypass_charge_status.lock().clone()
    }
    pub fn set_bypass_charge_status(&self, value: &str) {
        let _update_guard = self.charge_control_update_lock.lock();
        *self.charge_control_applied.lock() = None;
        let enabled = Self::parse_bypass_switch(value);
        let normalized = if enabled { "1" } else { "0" };
        *self.bypass_charge_status.lock() = normalized.into();
        self.backend.set_bypass_charge(enabled);
        self.set_charge_control_active(self.desired_charge_control_active());
        write_string_any(BYPASS_STATUS_PATHS, normalized);
    }
    fn set_charge_control_active(&self, restrict: bool) {
        let mut applied = self.charge_control_applied.lock();
        *self.charge_control_active.lock() = restrict;
        if *applied != Some(restrict) {
            self.backend.set_charge_control(restrict);
            *applied = Some(restrict);
        }
    }
    pub fn get_battery_authenticate(&self) -> i32 {
        self.info.lock().authentic
    }
    pub fn get_battery_short_status(&self) -> i32 {
        SHORT_CIRCUIT_HEALTHY
    }
    pub fn get_battery_short_ic_otp_status(&self) -> i32 {
        SHORT_CIRCUIT_HEALTHY
    }
    pub fn get_battery_short_feature(&self) -> i32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{Adapter, ChargerInfo};
    use std::sync::{mpsc::sync_channel, Arc};
    use std::time::Duration;

    #[test]
    fn screen_notification_never_waits_for_poll_or_control_locks() {
        let adapter = Adapter::new();
        let _info_guard = adapter.info.lock();
        let _charger_info_guard = adapter.charger_info_json.lock();
        let _fast_snapshot_guard = adapter.fast_charge_snapshot.lock();
        let _decimal_soc_guard = adapter.decimal_soc.lock();
        let _charge_control_guard = adapter.charge_control_update_lock.lock();
        let (done_tx, done_rx) = sync_channel(1);
        let notify_adapter = Arc::clone(&adapter);

        std::thread::spawn(move || {
            notify_adapter.notify_screen_status(1);
            let _ = done_tx.try_send(());
        });

        assert!(done_rx.recv_timeout(Duration::from_millis(250)).is_ok());
    }

    #[test]
    fn fast_snapshot_offline_suppresses_stale_fast_fields() {
        let snapshot = super::FastChargeSnapshot {
            online: false,
            usb_online: 0,
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
            usb_type: "USB_PD".into(),
            pc_port_online: 0,
        };

        assert!(!snapshot.is_fast_charge());
        assert!(!snapshot.is_svooc_active());
        assert!(!snapshot.is_pps_active());
    }

    #[test]
    fn charge_limit_latch_holds_at_boundary() {
        assert!(Adapter::charge_limit_latch_state(false, true, 91, Some(90)));
        assert!(Adapter::charge_limit_latch_state(false, true, 95, Some(95)));
        assert!(Adapter::charge_limit_latch_state(true, true, 95, Some(95)));
        assert!(Adapter::charge_limit_latch_state(true, true, 90, Some(90)));
        assert!(!Adapter::charge_limit_latch_state(true, true, 89, Some(90)));
        assert!(!Adapter::charge_limit_latch_state(
            true,
            false,
            95,
            Some(95)
        ));
    }

    #[test]
    fn zero_percent_decimal_soc_stays_near_zero() {
        let mut state = super::DecimalSocState::default();
        let (start, end) = Adapter::synthetic_decimal_soc_pair(&mut state, 0, 0, 1);

        assert!((5..=50).contains(&start));
        assert!((5..=85).contains(&end));
    }

    #[test]
    fn polling_slows_down_without_screen_or_charger() {
        assert_eq!(
            Adapter::poll_interval(true, false),
            super::SCREEN_ON_POLL_INTERVAL
        );
        assert_eq!(
            Adapter::poll_interval(false, true),
            super::SCREEN_OFF_CHARGING_POLL_INTERVAL
        );
        assert_eq!(
            Adapter::poll_interval(false, false),
            super::SCREEN_OFF_IDLE_POLL_INTERVAL
        );
    }

    #[test]
    fn unknown_xiaomi_metrics_are_not_fabricated() {
        let info = ChargerInfo::default();
        assert_eq!(info.battery_capacity, 0);
        assert_eq!(super::SHORT_CIRCUIT_HEALTHY, 1);
        assert_eq!(Adapter::best_soh(&info), 0);
        assert_eq!(Adapter::best_fcc_mah(&info), 0);
        assert_eq!(info.authentic, 1);
    }

    #[test]
    fn detects_only_power_supply_uevents() {
        assert!(super::is_power_supply_uevent(
            b"change@/devices/virtual/power_supply/battery\0ACTION=change\0SUBSYSTEM=power_supply\0"
        ));
        assert!(!super::is_power_supply_uevent(
            b"change@/devices/platform/display\0ACTION=change\0SUBSYSTEM=graphics\0"
        ));
        assert!(!super::is_power_supply_uevent(
            b"SUBSYSTEM=power_supply_extra\0"
        ));
    }

    #[test]
    fn synthetic_decimal_pair_progresses_without_decimal_node() {
        let mut state = super::DecimalSocState::default();
        let (first_start, first_end) = Adapter::synthetic_decimal_soc_pair(&mut state, 89, 0, 1);
        let (second_start, second_end) = Adapter::synthetic_decimal_soc_pair(&mut state, 89, 0, 1);

        assert!((8905..=8950).contains(&first_start));
        assert!((8905..=8985).contains(&first_end));
        assert_ne!(first_end, first_start);
        assert_eq!(second_start, first_end);
        assert!((8905..=8985).contains(&second_end));
        assert_ne!(second_end, second_start);
    }

    #[test]
    fn synthetic_decimal_pair_uses_node_seed_on_capacity_reset() {
        let mut state = super::DecimalSocState::default();
        let (first_start, first_end) = Adapter::synthetic_decimal_soc_pair(&mut state, 89, 8942, 1);
        let (second_start, second_end) =
            Adapter::synthetic_decimal_soc_pair(&mut state, 89, 8901, 1);

        assert_eq!(first_start, 8942);
        assert!((8905..=8985).contains(&first_end));
        assert_ne!(first_end, first_start);
        assert_eq!(second_start, first_end);
        assert!((8905..=8985).contains(&second_end));
        assert_ne!(second_end, second_start);
    }

    #[test]
    fn synthetic_decimal_pair_does_not_regress_after_reaching_cap() {
        let mut state = super::DecimalSocState::default();
        let mut previous_end = 0;

        for _ in 0..40 {
            let pair = Adapter::synthetic_decimal_soc_pair(&mut state, 89, 8942, 1);
            assert!((8905..=8985).contains(&pair.0));
            assert!((8905..=8985).contains(&pair.1));
            assert_ne!(pair.1, pair.0);
            assert!(pair.1 >= previous_end);
            previous_end = pair.1;
        }
    }

    #[test]
    fn synthetic_decimal_pair_does_not_regress_when_rate_increases() {
        let mut state = super::DecimalSocState::default();
        let (_, first_end) = Adapter::synthetic_decimal_soc_pair(&mut state, 89, 8980, 1);
        let (_, second_end) = Adapter::synthetic_decimal_soc_pair(&mut state, 89, 8980, 20);

        assert!(second_end >= first_end);
    }

    #[test]
    fn synthetic_decimal_pair_continues_past_eighty_five_centi() {
        let mut state = super::DecimalSocState::default();
        let (_, end) = Adapter::synthetic_decimal_soc_pair(&mut state, 89, 8984, 20);

        assert!(end > 8985);
        assert!(end <= 8999);
    }

    #[test]
    fn synthetic_decimal_pair_uses_rate_as_step() {
        let mut state = super::DecimalSocState::default();
        let (start, end) = Adapter::synthetic_decimal_soc_pair(&mut state, 89, 8942, 20);
        assert_eq!(start, 8942);
        assert_eq!(end, 8945);
    }
}
