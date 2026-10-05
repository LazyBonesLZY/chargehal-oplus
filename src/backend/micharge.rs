//! Xiaomi MiCharge vendor HAL backend.
//!
//! Bridges `vendor.xiaomi.hardware.micharge.IMiCharge/default` (AIDL V2) into the
//! adapter's [`ChargerInfo`] snapshot. The HAL already absorbs the per-model
//! kernel layout — `/sys/class/qcom-battery/*` on the older devices,
//! `/sys/class/xm_power/*` on the newer ones — so this backend only has to move
//! strings across binder and normalise units.
//!
//! # Units
//!
//! `ChargerInfo` keeps the kernel units the rest of the project expects:
//!
//! * voltage: µV (`getBatteryVbat`, `getUsbVoltage`)
//! * current: µA (`getBatteryIbat`, `getUsbCurrent`)
//! * temperature: 0.1 °C (`getBatteryTbat`)
//! * capacity: percent (`getBatteryCapacity`)
//! * power: auto-detected W / mW / µW (`getChargingPowerMax`)
//!
//! Every getter returns the raw sysfs contents, so values go through
//! [`parse_int`], which tolerates empty strings, junk and negative values. When a
//! getter fails or cannot be parsed the previous [`ChargerInfo`] value is kept —
//! never zeroed. That mirrors `adapter::update_int_from_paths`.
//!
//! # Method mapping
//!
//! Node evidence comes from `strings` over the shipped HAL binaries
//! (`vendor.xiaomi.hardware.micharge-service` and
//! `vendor.xiaomi.hardware.micharge@1.0-impl.so`), which embed the node paths the
//! getters read.
//!
//! Node paths below were resolved from the shipped HAL symbol tables (see
//! `chargehal-vendor-refs/MICHARGE-MAPPING.md`), not from name guessing. The
//! unit column says whether the scale is backed by a standard `power_supply`
//! ABI or is still unconfirmed.
//!
//! | IMiCharge getter | source node on the AIDL V2 generation | ChargerInfo field | unit |
//! |---|---|---|---|
//! | `getBatteryAuthentic` | `xm_power/fuelgauge/strategy_fg/authentic` | *not used* — vendor authenticity, not the OPlus question | — |
//! | `getBatteryCapacity` | `power_supply/battery/capacity` | `battery_capacity` | percent (ABI) |
//! | `getBatteryChargeFull` | `power_supply/battery/charge_full` | `charge_full` | µAh (ABI) |
//! | `getBatteryChargeType` | `xm_power/charger/charger_common/real_type` | `battery_charge_type` | enum, unconfirmed |
//! | `getBatteryCycleCount` | `power_supply/battery/cycle_count` | `cycle_count` | count (ABI) |
//! | `getBatteryIbat` | `power_supply/battery/current_now` | `battery_current_now` | µA (ABI) |
//! | `getBatterySoh` | `xm_power/fg_master/soh` | `fg_soh` | unconfirmed |
//! | `getBatteryTbat` | `power_supply/battery/temp` | `battery_temp` | 0.1 °C (ABI) |
//! | `getBatteryVbat` | `power_supply/battery/voltage_now` | `battery_voltage_now` | µV (ABI) |
//! | `getChargingPowerMax` | `xm_power/charger/charger_common/power_max` | `adapter_power_w` | unconfirmed |
//! | `getPdApdoMax` | `xm_power/typec/apdo_max` | *not used* — carries a PDO index, not a wattage | unconfirmed |
//! | `getFastChargeModeStatus` | `xm_power/fuelgauge/strategy_fg/fast_charge` | `fastchg_mode` | enum, unconfirmed |
//! | `getInputSuspendState` | `xm_power/charger/charge_interface/input_suspend` | `input_suspend` | 0/1, unconfirmed |
//! | `getNightChargingState` | `xm_power/charger/smart_charge/smart_night` | `night_charging` | 0/1, unconfirmed |
//! | `getPdAuthentication` | `xm_power/typec/strategy_pd_auth/verified` | `pd_verified` | 0/1, unconfirmed |
//! | `getQuickChargeType` | `xm_power/charger/charger_common/quick_charge_type` | `quick_charge_type` | enum, unconfirmed |
//! | `getUsbCurrent` | `power_supply/usb/current_now` | `usb_current_now` | µA (ABI) |
//! | `getUsbVoltage` | `power_supply/usb/voltage_now` | `usb_voltage_now` (+ `usb_online`) | µV (ABI) |
//! | `getSBState` | `xm_power/charger/smart_charge/smart_batt` | `smart_batt` | 0/1, unconfirmed |
//! | `getSocDecimal` | `xm_power/fuelgauge/strategy_fg/soc_decimal` | `soc_decimal` (collected, not served) | unconfirmed |
//! | `getSocDecimalRate` | `xm_power/fuelgauge/strategy_fg/soc_decimal_rate` | `soc_decimal_rate` (collected, not served) | unconfirmed |
//! | `getWirelessChargingStatus` | `xm_power/charger/wls_rev_charge/reverse_chg_mode` | *not used* — reverse-charge mode, not wireless online | — |
//! | `getCarChargingType` | `xm_power/charger/wls_basic_charge/wls_car_adapter` | `wireless_type` | enum, unconfirmed |
//!
//! # Method names do not carry units
//!
//! The vendor HAL performs **no** conversion: every getter returns the node
//! contents verbatim — it reads the whole file, joins the lines back with `\n`
//! and strips one trailing newline, so a multi-line node keeps its inner
//! newlines. The same method name can therefore read
//! different physical quantities on different generations — `getBatteryResistance`
//! is a pack identification resistor on V2 but a cell internal resistance on the
//! HIDL generation, and `getBatteryThermaLevel` reads a thermal control limit,
//! not a temperature. Only the standard `power_supply` nodes above have a fixed
//! ABI-backed scale; the private `xm_power/*` nodes do not, so no conversion
//! factor is baked in for them. Confirm against real device readings before
//! trusting those values.
//!
//! `getWirelessChargingStatus` is **not** used: on every generation it reads the
//! reverse-charging mode node (`getWirelessReverseStatus` reads the very same
//! node), so mapping it to `wireless_online` would make an active reverse-charge
//! session look like incoming wireless power. `wireless_online` comes from the
//! kernel node instead.
//!
//! # TODO(confirm)
//!
//! These getters exist but have no confirmed `ChargerInfo` destination, so they
//! are deliberately not called from [`MiChargeBackend::refresh`]:
//!
//! * `getBatteryResistance` — `xm_power/battery/resistance_id`, no matching field.
//! * `getBatteryThermaLevel` — `xm_power/charger/charger_thermal/wired_ctrl_limit`,
//!   no matching field.
//! * `getBtTransferStartState` — `wireless_master/bt_transfer_start`, no field.
//! * `getCoolModeState` — no field, and confirmed unimplemented on V2: the
//!   vendor service only logs `not support coolMode` and returns an empty
//!   string. `setCoolModeState` shares that address and returns 0 without
//!   writing anything.
//! * `getPSValue` — semantics unconfirmed.
//! * `getTxAdapt` — `wireless_master/tx_adapter`, no field.
//! * `getWirelessFwStatus`, `getWirelessReverseStatus` — no matching fields.
//! * `getMiChargePath` / `get*CommonInfo` — generic key/value accessors, unused.
//!
//! `usb_online` has no getter; it is derived from `usb/voltage_now` (non-zero
//! only while a USB source is attached). `ac_online`, `battery_status` and
//! `battery_health` have no HAL getter either and are read from their kernel
//! nodes in the supplemental stage.
//!
//! `getSocDecimal` / `getSocDecimalRate` are collected but not served: their node
//! is not the one the official decimal-SOC contract names, and its scale is
//! unconfirmed. See `get_decimal_soc` for the full reasoning.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use rsbinder::{hub, DeathRecipient, StatusCode, Strong, WIBinder};

use crate::adapter::ChargerInfo;
use crate::vendor::xiaomi::hardware::micharge::IMiCharge::IMiCharge as MiCharge;

use super::sysfs::SysfsBackend;
use super::ChargeBackend;

/// Service instance registered by the Xiaomi vendor HAL.
pub const MICHARGE_SERVICE_NAME: &str = "vendor.xiaomi.hardware.micharge.IMiCharge/default";

/// Snapshot fields the vendor HAL exposes no dedicated getter for, filled from
/// nodes the HAL itself reads.
///
/// Paths and evidence grades come from `chargehal-vendor-refs/HAL-NODES.md`,
/// which was extracted from the HAL binary. Grades:
///
/// * `A` — Android `power_supply` ABI fixes the unit, safe to read as-is.
/// * `C` — private `xm_power` node, unit unconfirmed; copied verbatim and never
///   scaled, so a wrong guess can never enter the snapshot.
///
/// These nodes are specific to the AIDL V2 generation. On other kernels the
/// reads simply fail and the previous value is kept.
const CHARGE_COUNTER: &str = "/sys/class/power_supply/battery/charge_counter"; // A, µAh
const CHARGE_FULL_DESIGN: &str = "/sys/class/power_supply/battery/charge_full_design"; // A, µAh
const FG_MASTER_VBATT: &str = "/sys/class/xm_power/fg_master/vbatt"; // C
const FG_SLAVE_VBATT: &str = "/sys/class/xm_power/fg_slave/vbatt"; // C
const FG_MASTER_RM: &str = "/sys/class/xm_power/fg_master/rm"; // C
const FG_MASTER_BATT_SN: &str = "/sys/class/xm_power/fg_master/batt_sn"; // C, text
const CONNECTOR_TEMP_1: &str = "/sys/class/xm_power/hw_monitor/connector/connector_temp_1"; // C
const CONNECTOR_TEMP_2: &str = "/sys/class/xm_power/hw_monitor/connector/connector_temp_2"; // C

/// Flips to `false` when the vendor HAL process dies.
struct MiChargeDeath {
    alive: Arc<AtomicBool>,
}

impl DeathRecipient for MiChargeDeath {
    fn binder_died(&self, _who: &WIBinder) {
        self.alive.store(false, Ordering::Release);
    }
}

/// Last cheap-probe result, compared against the next one.
#[derive(Clone, PartialEq, Eq, Default)]
struct ProbeSnapshot {
    initialized: bool,
    dp_connected: bool,
    quick_charge_type: String,
    power_max: String,
    usb_online: i32,
    ac_online: i32,
    wireless_online: i32,
}

struct HalProbeSample<'a> {
    dp_connected: bool,
    quick_charge_type: &'a str,
    power_max: &'a str,
    usb_online: Option<i32>,
    ac_online: Option<i32>,
    wireless_online: Option<i32>,
}

/// Compare one cheap probe against the previous one.
///
/// The first sample always counts as a change. An unreadable online node
/// (`None`) does not: a missing file must not look like the charger was
/// unplugged and spin the poll loop.
fn probe_sample_changed(cache: &mut ProbeSnapshot, sample: HalProbeSample<'_>) -> bool {
    let hal_changed = !cache.initialized
        || cache.dp_connected != sample.dp_connected
        || cache.quick_charge_type != sample.quick_charge_type
        || cache.power_max != sample.power_max;
    let online_changed = cache.initialized
        && (sample
            .usb_online
            .is_some_and(|value| value != cache.usb_online)
            || sample
                .ac_online
                .is_some_and(|value| value != cache.ac_online)
            || sample
                .wireless_online
                .is_some_and(|value| value != cache.wireless_online));
    cache.initialized = true;
    cache.dp_connected = sample.dp_connected;
    cache.quick_charge_type.clear();
    cache.quick_charge_type.push_str(sample.quick_charge_type);
    cache.power_max.clear();
    cache.power_max.push_str(sample.power_max);
    if let Some(value) = sample.usb_online {
        cache.usb_online = value;
    }
    if let Some(value) = sample.ac_online {
        cache.ac_online = value;
    }
    if let Some(value) = sample.wireless_online {
        cache.wireless_online = value;
    }
    hal_changed || online_changed
}

/// Xiaomi MiCharge vendor HAL backend.
pub struct MiChargeBackend {
    service: Mutex<Option<Strong<dyn MiCharge>>>,
    alive: Arc<AtomicBool>,
    /// Kept alive for the lifetime of the link: `link_to_death` stores only a
    /// weak reference, so dropping this would silently disable death notices.
    death: Arc<MiChargeDeath>,
    probe: Mutex<ProbeSnapshot>,
    fallback: SysfsBackend,
}

impl MiChargeBackend {
    /// Connect to the vendor HAL.
    ///
    /// Fails when the service is not registered; the caller then falls back to
    /// the kernel-node reader.
    pub fn connect() -> Result<Self, StatusCode> {
        let alive = Arc::new(AtomicBool::new(true));
        let death = Arc::new(MiChargeDeath {
            alive: Arc::clone(&alive),
        });
        let backend = Self {
            service: Mutex::new(None),
            alive,
            death,
            probe: Mutex::new(ProbeSnapshot::default()),
            fallback: SysfsBackend::new(),
        };
        backend.attach()?;
        Ok(backend)
    }

    /// Resolve the service and arm the death notification.
    fn attach(&self) -> Result<(), StatusCode> {
        let proxy = hub::check_interface::<dyn MiCharge>(MICHARGE_SERVICE_NAME)?;
        if let Err(error) = proxy.link_to_death_arc(&self.death) {
            tracing::warn!("MiCharge death notification unavailable: {error}");
        }
        *self.service.lock() = Some(proxy);
        self.alive.store(true, Ordering::Release);
        Ok(())
    }

    /// Current proxy, reconnecting after a HAL restart.
    fn proxy(&self) -> Option<Strong<dyn MiCharge>> {
        #[cfg(not(target_os = "android"))]
        {
            None
        }
        #[cfg(target_os = "android")]
        {
            if self.alive.load(Ordering::Acquire) {
                if let Some(proxy) = self.service.lock().as_ref() {
                    return Some(proxy.clone());
                }
            }
            self.attach().ok()?;
            self.service.lock().clone()
        }
    }
}

impl ChargeBackend for MiChargeBackend {
    fn name(&self) -> &'static str {
        "micharge"
    }

    fn is_available(&self) -> bool {
        self.proxy().is_some()
    }

    fn refresh(&self, info: &mut ChargerInfo, should_cancel: &dyn Fn() -> bool) -> bool {
        if should_cancel() {
            return false;
        }
        let Some(proxy) = self.proxy() else {
            tracing::warn!("MiCharge HAL proxy unavailable; falling back to sysfs refresh");
            // Drop HAL-only telemetry, otherwise a value from the last successful
            // HAL cycle would survive into the fallback path and be reported as
            // if it were current. The fallback backend never writes these fields.
            info.soc_decimal.clear();
            info.soc_decimal_rate.clear();
            return self.fallback.refresh(info, should_cancel);
        };

        // ── Battery stage ──
        //
        // `getBatteryAuthentic` is not called: it answers a Xiaomi-pack question,
        // not the contract's. See `ChargerInfo::authentic`.
        apply_capacity(
            &mut info.battery_capacity,
            fetch(&*proxy, |p| p.getBatteryCapacity()).as_deref(),
        );
        apply_int(
            &mut info.charge_full,
            fetch(&*proxy, |p| p.getBatteryChargeFull()).as_deref(),
        );
        apply_string(
            &mut info.battery_charge_type,
            fetch(&*proxy, |p| p.getBatteryChargeType()).as_deref(),
        );
        apply_int(
            &mut info.cycle_count,
            fetch(&*proxy, |p| p.getBatteryCycleCount()).as_deref(),
        );
        apply_int(
            &mut info.battery_current_now,
            fetch(&*proxy, |p| p.getBatteryIbat()).as_deref(),
        );
        apply_int(
            &mut info.fg_soh,
            fetch(&*proxy, |p| p.getBatterySoh()).as_deref(),
        );
        apply_int(
            &mut info.battery_temp,
            fetch(&*proxy, |p| p.getBatteryTbat()).as_deref(),
        );
        apply_int(
            &mut info.battery_voltage_now,
            fetch(&*proxy, |p| p.getBatteryVbat()).as_deref(),
        );

        if should_cancel() {
            return false;
        }

        // ── Charger / PD stage ──
        apply_string(
            &mut info.quick_charge_type,
            fetch(&*proxy, |p| p.getQuickChargeType()).as_deref(),
        );
        apply_int(
            &mut info.fastchg_mode,
            fetch(&*proxy, |p| p.getFastChargeModeStatus()).as_deref(),
        );
        apply_int(
            &mut info.input_suspend,
            fetch(&*proxy, |p| p.getInputSuspendState()).as_deref(),
        );
        apply_int(
            &mut info.night_charging,
            fetch(&*proxy, |p| p.getNightChargingState()).as_deref(),
        );
        apply_int(
            &mut info.pd_verified,
            fetch(&*proxy, |p| p.getPdAuthentication()).as_deref(),
        );
        apply_int(
            &mut info.usb_current_now,
            fetch(&*proxy, |p| p.getUsbCurrent()).as_deref(),
        );
        // usb_voltage_now drives usb_online, so update both together and only
        // when the read actually parsed.
        if let Some(voltage) = fetch(&*proxy, |p| p.getUsbVoltage())
            .as_deref()
            .and_then(parse_int)
        {
            info.usb_voltage_now = voltage;
            info.usb_online = if voltage > 0 { 1 } else { 0 };
        }
        // `getPdApdoMax` is deliberately not used as a power fallback: its node
        // carries a PDO/APDO index or a voltage/current pair, not a wattage, so
        // running it through the watt normaliser would invent a number. Keeping
        // the previous value beats that.
        apply_power_w(
            &mut info.adapter_power_w,
            fetch(&*proxy, |p| p.getChargingPowerMax()).as_deref(),
        );
        apply_string(
            &mut info.wireless_type,
            fetch(&*proxy, |p| p.getCarChargingType()).as_deref(),
        );

        if should_cancel() {
            return false;
        }

        // ── Typec / wireless stage ──
        //
        // `getWirelessChargingStatus` is deliberately not used here: on every
        // generation it reads the reverse-charging mode node, not a wireless
        // power-supply online flag (`getWirelessReverseStatus` reads the very
        // same node). Writing it into `wireless_online` made an active
        // reverse-charge session look like incoming wireless power, which can
        // surface a fast-charge badge. Wireless online comes from the kernel
        // node in the supplemental stage below.

        // ── Telemetry the HAL exposes beyond what the snapshot consumed before ──
        //
        // `getSocDecimal` / `getSocDecimalRate` are COLLECTED BUT NOT SERVED.
        // `get_decimal_soc` deliberately does not return them (see the comment
        // there): the node differs from the one the official contract names
        // (/proc/ui_soc_decimal) and its scale is unconfirmed, so serving it could
        // inject a 10× error under a call specified for a different node. They
        // surface through `build_soh_debug_info` instead, so a device reading can
        // calibrate them before anything is wired to the wire contract.
        apply_string(
            &mut info.soc_decimal,
            fetch(&*proxy, |p| p.getSocDecimal()).as_deref(),
        );
        apply_string(
            &mut info.soc_decimal_rate,
            fetch(&*proxy, |p| p.getSocDecimalRate()).as_deref(),
        );
        // Smart-battery switch state: private node, unconfirmed scale, used as a
        // flag only.
        apply_int(
            &mut info.smart_batt,
            fetch(&*proxy, |p| p.getSBState()).as_deref(),
        );

        if should_cancel() {
            return false;
        }
        // Supplemental reads for attributes the vendor HAL exposes no getter for.
        // Two groups with different provenance — do not blur them:
        //
        //   Group 1 — the HAL itself reads these nodes (each verified against the
        //             171 node inventory in chargehal-vendor-refs/HAL-NODES.md).
        //   Group 2 — the HAL reads nothing of the sort; these are the bridge's
        //             own reads for capabilities the HAL does not expose, and
        //             their presence on device is UNCONFIRMED.
        //
        // Grade: A = Android power_supply ABI fixes the unit; C = private
        // xm_power node, unit unconfirmed — copied verbatim, never scaled, and a
        // missing node leaves the previous value untouched.
        //
        // Group 1 (HAL-referenced): charge_counter(A), charge_full_design(A),
        // cell1_vol(C), cell2_vol(C), fg_rm(C), batt_sn(C), usb_temp(C),
        // connector_temp(C) — see the path constants above.
        //
        // Group 2 (bridge-local, not HAL-referenced): battery_status,
        // battery_health, ac_online, wireless_online, pc_port_online.
        //
        // `usb_type` is deliberately NOT read. The only node that could carry it
        // is power_supply/usb/usb_type (HAL-NODES.md §6) and its value domain —
        // "SDP"/"CDP" versus a bare "USB" — is unverified. A bare "USB" fed into
        // `is_data_port_usb_type` would classify every charger as a data port and
        // suppress fast-charge reporting unconditionally. With no device to
        // settle the domain, the data-port check relies on pc_port_online alone.
        let batt_status_path = format!("{}/status", crate::backend::sysfs::PSY_BATTERY);
        let batt_health_path = format!("{}/health", crate::backend::sysfs::PSY_BATTERY);
        let ac_online_path = format!("{}/online", crate::backend::sysfs::PSY_AC);
        let wireless_online_path = format!("{}/online", crate::backend::sysfs::PSY_WIRELESS);
        let dc_online_path = format!("{}/online", crate::backend::sysfs::PSY_DC);
        crate::backend::sysfs::update_non_empty_string_from_paths(
            &mut info.battery_status,
            &[&batt_status_path],
        );
        crate::backend::sysfs::update_non_empty_string_from_paths(
            &mut info.battery_health,
            &[&batt_health_path],
        );
        crate::backend::sysfs::update_int_from_paths(&mut info.ac_online, &[&ac_online_path]);
        crate::backend::sysfs::update_online_from_paths(
            &mut info.wireless_online,
            &[&wireless_online_path, &dc_online_path],
        );
        crate::backend::sysfs::update_int_from_paths(
            &mut info.pc_port_online,
            crate::backend::sysfs::PC_PORT_ONLINE_PATHS,
        );

        // Group 1, A grade: standard power_supply nodes, units fixed by the ABI.
        crate::backend::sysfs::update_int_from_paths(&mut info.charge_counter, &[CHARGE_COUNTER]);
        crate::backend::sysfs::update_int_from_paths(
            &mut info.charge_full_design,
            &[CHARGE_FULL_DESIGN],
        );
        // Group 1, C grade: private nodes, verbatim only.
        crate::backend::sysfs::update_int_from_paths(&mut info.cell1_vol, &[FG_MASTER_VBATT]);
        crate::backend::sysfs::update_int_from_paths(&mut info.cell2_vol, &[FG_SLAVE_VBATT]);
        crate::backend::sysfs::update_int_from_paths(&mut info.fg_rm, &[FG_MASTER_RM]);
        // Mirror the node-reader path so both backends agree on the number. This
        // does mix grades: an A-grade node being backfilled by a C-grade one. It
        // is kept because the alternative is reporting 0, and the reader path
        // does exactly the same.
        if info.charge_counter == 0 {
            info.charge_counter = info.fg_rm;
        }
        crate::backend::sysfs::update_non_empty_string_from_paths(
            &mut info.batt_sn,
            &[FG_MASTER_BATT_SN],
        );
        // Field assignment follows the reader path's convention (it maps the same
        // two connector temperature nodes onto these same two fields); which node
        // is "usb" versus "board" is not documented anywhere, so treat the split
        // as inherited rather than proven.
        crate::backend::sysfs::update_int_from_paths(&mut info.usb_temp, &[CONNECTOR_TEMP_1]);
        crate::backend::sysfs::update_int_from_paths(&mut info.connector_temp, &[CONNECTOR_TEMP_2]);

        if should_cancel() {
            return false;
        }

        // This path never reads `usb_type`. A previous sysfs scan can leave
        // the generic value "USB" behind, and the data-port check treats that
        // as a computer. Drop it so only `pc_port_online` applies here.
        info.usb_type.clear();
        info.usb_real_type.clear();

        let charger_online =
            info.usb_online != 0 || info.ac_online != 0 || info.wireless_online != 0;
        if !charger_online {
            crate::backend::sysfs::clear_fast_charge_session(info);
        } else if crate::backend::sysfs::is_data_port(info) {
            // A computer is on the other end of the cable: drop the stale
            // fast-charge evidence so the classifier cannot promote it.
            crate::backend::sysfs::suppress_fast_charge_evidence(info);
        }

        info.fast_charge_type = crate::backend::sysfs::classify_fast_charge(info);
        info.charge_technology = crate::backend::sysfs::classify_charge_technology(info);
        info.charge_state = crate::backend::sysfs::classify_charge_state(info);
        // The estimator returns a positive input unchanged, so the stale
        // estimate has to be cleared first or the value freezes after the first
        // successful computation. The sysfs path gets this for free because
        // read_int_any() yields 0 when the node is missing.
        info.remaining_time = 0;
        info.remaining_time = crate::backend::sysfs::estimate_remaining_time_seconds(info);

        true
    }

    fn set_charge_control(&self, restrict: bool) {
        let Some(proxy) = self.proxy() else {
            tracing::warn!("MiCharge HAL unavailable; falling back to sysfs charge control");
            self.fallback.set_charge_control(restrict);
            return;
        };
        let value = if restrict { "1" } else { "0" };
        // Input suspend is the only working restrict switch on the AIDL V2
        // generation. The vendor HAL prepends "micharge all " and writes
        // xm_power/charger/charge_interface/input_suspend.
        //
        // setCoolModeState is deliberately not called: disassembly shows it
        // shares its address with the getter (0x25aa4), is 11 instructions long
        // and only logs "not support coolMode", returning 0 without touching any
        // node. Calling it would cost a binder round trip and nothing else.
        match fetch(&*proxy, |p| p.setInputSuspendState(value)) {
            Some(0) => {}
            Some(status) => {
                // The status comes back from the HAL's own node write, so the
                // call did reach the HAL and it reported the write failing.
                // Falling back to sysfs here would apply a *different* mechanism
                // (threshold-based) for the same intent while the HAL path may
                // have partially applied. Until the non-zero semantics are
                // confirmed on a device, warn only.
                tracing::warn!("MiCharge setInputSuspendState({value}) returned {status}");
            }
            None => {
                // Transport failure: the call never reached the HAL, so the
                // node path is the only way to honour the request.
                tracing::warn!(
                    "MiCharge setInputSuspendState({value}) failed; trying sysfs fallback"
                );
                self.fallback.set_charge_control(restrict);
            }
        }
    }

    fn power_source_changed(&self) -> bool {
        // Cheap probe only: three light getters plus the online nodes the HAL
        // does not expose. Capacity is deliberately absent. It drifts upward
        // while charging, so including it would escalate to a full scan on
        // every percent.
        let Some(proxy) = self.proxy() else {
            return self.fallback.power_source_changed();
        };
        let (Some(dp_connected), Some(quick_charge_type), Some(power_max)) = (
            fetch(&*proxy, |p| p.isDPConnected()),
            fetch(&*proxy, |p| p.getQuickChargeType()),
            fetch(&*proxy, |p| p.getChargingPowerMax()),
        ) else {
            // A probe that cannot answer must escalate rather than report "no
            // change": the latter would delay noticing a newly attached charger
            // until the next poll timeout.
            return true;
        };

        let usb_online_path = format!("{}/online", crate::backend::sysfs::PSY_USB);
        let ac_online_path = format!("{}/online", crate::backend::sysfs::PSY_AC);
        let wireless_online_path = format!("{}/online", crate::backend::sysfs::PSY_WIRELESS);
        let dc_online_path = format!("{}/online", crate::backend::sysfs::PSY_DC);
        let sample = HalProbeSample {
            dp_connected,
            quick_charge_type: &quick_charge_type,
            power_max: &power_max,
            usb_online: crate::backend::sysfs::try_read_int(&usb_online_path),
            ac_online: crate::backend::sysfs::try_read_int(&ac_online_path),
            wireless_online: crate::backend::sysfs::try_read_online_any(&[
                &wireless_online_path,
                &dc_online_path,
            ]),
        };
        probe_sample_changed(&mut self.probe.lock(), sample)
    }
}

/// Run one HAL getter, logging and swallowing transport failures.
fn fetch<T>(
    proxy: &dyn MiCharge,
    call: impl FnOnce(&dyn MiCharge) -> rsbinder::BinderResult<T>,
) -> Option<T> {
    match call(proxy) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!("MiCharge call failed: {error}");
            None
        }
    }
}

fn clamp_i64_to_i32(value: i64) -> i32 {
    value.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Parse a raw HAL string, tolerating empty strings, junk and negatives.
///
/// Returns `None` for anything unparseable so the caller keeps the cached value.
fn parse_int(raw: &str) -> Option<i32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(clamp_i64_to_i32(value));
    }
    // Some getters return "89\n" plus a label, or keyed payloads like
    // "capacity=89". Fall back to the first integer token.
    trimmed
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .find(|part| !part.is_empty() && *part != "-")
        .and_then(|part| part.parse::<i32>().ok())
}

/// Percentage, tolerating the 0..10000 form some nodes use.
fn normalize_battery_capacity(value: i32) -> i32 {
    let percent = if value > 100 { value / 100 } else { value };
    percent.clamp(0, 100)
}

/// Watts, auto-detecting W / mW / µW (same heuristic as `adapter`).
fn normalize_power_value(power: i32) -> i32 {
    if power > 100_000 {
        power / 1_000_000
    } else if power > 500 {
        power / 1_000
    } else {
        power
    }
}

/// Overwrite `target` only when `raw` parses; never clears on failure.
fn apply_int(target: &mut i32, raw: Option<&str>) {
    if let Some(value) = raw.and_then(parse_int) {
        *target = value;
    }
}

fn apply_capacity(target: &mut i32, raw: Option<&str>) {
    if let Some(value) = raw.and_then(parse_int) {
        *target = normalize_battery_capacity(value);
    }
}

fn apply_power_w(target: &mut i32, raw: Option<&str>) {
    if let Some(value) = raw.and_then(parse_int) {
        *target = normalize_power_value(value);
    }
}

/// Overwrite `target` only with a non-empty trimmed string.
fn apply_string(target: &mut String, raw: Option<&str>) {
    let Some(value) = raw else {
        return;
    };
    let value = value.trim();
    if !value.is_empty() {
        target.clear();
        target.push_str(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_int, apply_string, normalize_battery_capacity, normalize_power_value, parse_int,
    };

    #[test]
    fn parses_plain_and_signed_ints() {
        assert_eq!(parse_int("89"), Some(89));
        assert_eq!(parse_int("  -42\n"), Some(-42));
        assert_eq!(parse_int("0"), Some(0));
    }

    #[test]
    fn rejects_empty_and_junk() {
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_int("   \n"), None);
        assert_eq!(parse_int("not-a-number"), None);
        assert_eq!(parse_int("N/A"), None);
    }

    #[test]
    fn falls_back_to_first_integer_token() {
        assert_eq!(parse_int("capacity=89"), Some(89));
        assert_eq!(parse_int("12abc"), Some(12));
    }

    #[test]
    fn failure_keeps_previous_value() {
        let mut value = 42;
        apply_int(&mut value, None);
        assert_eq!(value, 42);
        apply_int(&mut value, Some(""));
        assert_eq!(value, 42);
        apply_int(&mut value, Some("garbage"));
        assert_eq!(value, 42);
        apply_int(&mut value, Some("0"));
        assert_eq!(value, 0);
    }

    #[test]
    fn empty_string_keeps_previous_text() {
        let mut value = "Charging".to_string();
        apply_string(&mut value, Some("  "));
        assert_eq!(value, "Charging");
        apply_string(&mut value, Some("Discharging\n"));
        assert_eq!(value, "Discharging");
    }

    #[test]
    fn normalizes_capacity_and_power_units() {
        assert_eq!(normalize_battery_capacity(89), 89);
        assert_eq!(normalize_battery_capacity(8999), 89);
        assert_eq!(normalize_battery_capacity(10000), 100);
        assert_eq!(normalize_battery_capacity(-3), 0);
        assert_eq!(normalize_power_value(67), 67);
        assert_eq!(normalize_power_value(67_000), 67);
        assert_eq!(normalize_power_value(67_000_000), 67);
    }

    #[test]
    fn offline_proxy_falls_back_to_sysfs_without_spinning() {
        use crate::adapter::ChargerInfo;
        use crate::backend::ChargeBackend;

        let backend = super::MiChargeBackend {
            service: parking_lot::Mutex::new(None),
            alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            death: std::sync::Arc::new(super::MiChargeDeath {
                alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
            probe: parking_lot::Mutex::new(super::ProbeSnapshot::default()),
            fallback: super::SysfsBackend::new(),
        };

        let mut info = ChargerInfo::default();
        let completed = backend.refresh(&mut info, &|| false);
        assert!(completed);

        let cancelled = backend.refresh(&mut info, &|| true);
        assert!(!cancelled);
    }

    #[test]
    fn probe_reports_supply_changes_and_ignores_unreadable_nodes() {
        let mut cache = super::ProbeSnapshot::default();
        let first = super::HalProbeSample {
            dp_connected: false,
            quick_charge_type: "0",
            power_max: "0",
            usb_online: Some(0),
            ac_online: Some(0),
            wireless_online: Some(0),
        };
        assert!(super::probe_sample_changed(&mut cache, first));

        let same = super::HalProbeSample {
            dp_connected: false,
            quick_charge_type: "0",
            power_max: "0",
            usb_online: None,
            ac_online: None,
            wireless_online: None,
        };
        assert!(!super::probe_sample_changed(&mut cache, same));
        assert_eq!(cache.usb_online, 0);

        let wireless = super::HalProbeSample {
            dp_connected: false,
            quick_charge_type: "0",
            power_max: "0",
            usb_online: Some(0),
            ac_online: Some(0),
            wireless_online: Some(1),
        };
        assert!(super::probe_sample_changed(&mut cache, wireless));
        assert_eq!(cache.wireless_online, 1);
    }

    #[test]
    fn offline_proxy_delegates_power_source_and_control_to_fallback() {
        use crate::backend::ChargeBackend;

        let backend = super::MiChargeBackend {
            service: parking_lot::Mutex::new(None),
            alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            death: std::sync::Arc::new(super::MiChargeDeath {
                alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
            probe: parking_lot::Mutex::new(super::ProbeSnapshot::default()),
            fallback: super::SysfsBackend::new(),
        };

        // None proxy delegates to fallback.power_source_changed() without crashing
        let _ = backend.power_source_changed();
        backend.set_charge_control(true);
        backend.set_charge_control(false);
    }
}
