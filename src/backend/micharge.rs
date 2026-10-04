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
//! | `getBatteryAuthentic` | `xm_power/fuelgauge/strategy_fg/authentic` | `authentic` | unconfirmed |
//! | `getBatteryCapacity` | `power_supply/battery/capacity` | `battery_capacity` | percent (ABI) |
//! | `getBatteryChargeFull` | `power_supply/battery/charge_full` | `charge_full` | µAh (ABI) |
//! | `getBatteryChargeType` | `xm_power/charger/charger_common/real_type` | `battery_charge_type` | enum, unconfirmed |
//! | `getBatteryCycleCount` | `power_supply/battery/cycle_count` | `cycle_count` | count (ABI) |
//! | `getBatteryIbat` | `power_supply/battery/current_now` | `battery_current_now` | µA (ABI) |
//! | `getBatterySoh` | `xm_power/fg_master/soh` | `fg_soh` | unconfirmed |
//! | `getBatteryTbat` | `power_supply/battery/temp` | `battery_temp` | 0.1 °C (ABI) |
//! | `getBatteryVbat` | `power_supply/battery/voltage_now` | `battery_voltage_now` | µV (ABI) |
//! | `getChargingPowerMax` | `xm_power/charger/charger_common/power_max` | `adapter_power_w` | unconfirmed |
//! | `getPdApdoMax` | `xm_power/typec/apdo_max` | `adapter_power_w` (fallback) | unconfirmed |
//! | `getFastChargeModeStatus` | `xm_power/fuelgauge/strategy_fg/fast_charge` | `fastchg_mode` | enum, unconfirmed |
//! | `getInputSuspendState` | `xm_power/charger/charge_interface/input_suspend` | `input_suspend` | 0/1, unconfirmed |
//! | `getNightChargingState` | `xm_power/charger/smart_charge/smart_night` | `night_charging` | 0/1, unconfirmed |
//! | `getPdAuthentication` | `xm_power/typec/strategy_pd_auth/verified` | `pd_verified` | 0/1, unconfirmed |
//! | `getQuickChargeType` | `xm_power/charger/charger_common/quick_charge_type` | `quick_charge_type` | enum, unconfirmed |
//! | `getUsbCurrent` | `power_supply/usb/current_now` | `usb_current_now` | µA (ABI) |
//! | `getUsbVoltage` | `power_supply/usb/voltage_now` | `usb_voltage_now` (+ `usb_online`) | µV (ABI) |
//! | `getWirelessChargingStatus` | `xm_power/charger/wls_rev_charge/reverse_chg_mode` | `wireless_online` | semantics doubtful |
//! | `getCarChargingType` | `xm_power/charger/wls_basic_charge/wls_car_adapter` | `wireless_type` | enum, unconfirmed |
//!
//! # Method names do not carry units
//!
//! The vendor HAL performs **no** conversion: every getter returns the first
//! line of a sysfs node verbatim. The same method name can therefore read
//! different physical quantities on different generations — `getBatteryResistance`
//! is a pack identification resistor on V2 but a cell internal resistance on the
//! HIDL generation, and `getBatteryThermaLevel` reads a thermal control limit,
//! not a temperature. Only the standard `power_supply` nodes above have a fixed
//! ABI-backed scale; the private `xm_power/*` nodes do not, so no conversion
//! factor is baked in for them. Confirm against real device readings before
//! trusting those values.
//!
//! `getWirelessChargingStatus` is mapped to `wireless_online` but actually reads
//! the reverse-charging mode node, so treat that field as approximate.
//!
//! # TODO(confirm)
//!
//! These getters exist but have no confirmed `ChargerInfo` destination, so they
//! are deliberately not called from [`MiChargeBackend::refresh`]:
//!
//! * `getBatteryResistance` — `xm_power/battery/resistance_id`, no matching field.
//! * `getBatteryThermaLevel` — `thermal_message/sconfig`, no matching field.
//! * `getBtTransferStartState` — `wireless_master/bt_transfer_start`, no field.
//! * `getCoolModeState` — no field, and confirmed unimplemented on V2: the
//!   vendor service only logs `not support coolMode` and returns an empty
//!   string. `setCoolModeState` shares that address and returns 0 without
//!   writing anything.
//! * `getPSValue`, `getSBState` — semantics unconfirmed.
//! * `getSocDecimal`, `getSocDecimalRate` — the adapter reads these outside
//!   `ChargerInfo`, so they are not part of this snapshot.
//! * `getTxAdapt` — `wireless_master/tx_adapter`, no field.
//! * `getWirelessFwStatus`, `getWirelessReverseStatus` — no matching fields.
//! * `getMiChargePath` / `get*CommonInfo` — generic key/value accessors, unused.
//!
//! `usb_online` has no getter at all; it is derived from `usb/voltage_now`
//! (non-zero only while a USB source is attached). `ac_online`, `battery_status`
//! and `battery_health` are likewise unsourced and keep their previous value.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use rsbinder::{hub, DeathRecipient, StatusCode, Strong, WIBinder};

use crate::adapter::ChargerInfo;
use crate::vendor::xiaomi::hardware::micharge::IMiCharge::IMiCharge as MiCharge;

use super::ChargeBackend;

/// Service instance registered by the Xiaomi vendor HAL.
pub const MICHARGE_SERVICE_NAME: &str = "vendor.xiaomi.hardware.micharge.IMiCharge/default";

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
    capacity: String,
    quick_charge_type: String,
    power_max: String,
}

/// Xiaomi MiCharge vendor HAL backend.
pub struct MiChargeBackend {
    service: Mutex<Option<Strong<dyn MiCharge>>>,
    alive: Arc<AtomicBool>,
    /// Kept alive for the lifetime of the link: `link_to_death` stores only a
    /// weak reference, so dropping this would silently disable death notices.
    death: Arc<MiChargeDeath>,
    probe: Mutex<ProbeSnapshot>,
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
        if self.alive.load(Ordering::Acquire) {
            if let Some(proxy) = self.service.lock().as_ref() {
                return Some(proxy.clone());
            }
        }
        self.attach().ok()?;
        self.service.lock().clone()
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
            return false;
        };

        // ── Battery stage ──
        apply_int(
            &mut info.authentic,
            fetch(&*proxy, |p| p.getBatteryAuthentic()).as_deref(),
        );
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
        apply_power_w(
            &mut info.adapter_power_w,
            fetch(&*proxy, |p| p.getChargingPowerMax()).as_deref(),
        );
        if info.adapter_power_w == 0 {
            apply_power_w(
                &mut info.adapter_power_w,
                fetch(&*proxy, |p| p.getPdApdoMax()).as_deref(),
            );
        }
        apply_string(
            &mut info.wireless_type,
            fetch(&*proxy, |p| p.getCarChargingType()).as_deref(),
        );

        if should_cancel() {
            return false;
        }

        // ── Typec / wireless stage ──
        apply_int(
            &mut info.wireless_online,
            fetch(&*proxy, |p| p.getWirelessChargingStatus()).as_deref(),
        );

        true
    }

    fn set_charge_control(&self, restrict: bool) {
        let Some(proxy) = self.proxy() else {
            tracing::warn!("MiCharge HAL unavailable; charge control not applied");
            return;
        };
        let value = if restrict { "1" } else { "0" };
        // Input suspend is the real restrict switch on the AIDL V2 generation: it
        // writes xm_power/charger/charge_interface/input_suspend.
        //
        // setCoolModeState is kept because it is a real implementation on the
        // HIDL generation, but on V2 the vendor service only logs
        // "not support coolMode" and returns 0 without touching a node — its
        // getter and setter even share one address. The call is harmless.
        let calls = [
            (
                "setInputSuspendState",
                fetch(&*proxy, |p| p.setInputSuspendState(value)),
            ),
            (
                "setCoolModeState",
                fetch(&*proxy, |p| p.setCoolModeState(value)),
            ),
        ];
        for (name, result) in calls {
            match result {
                Some(0) => {}
                Some(status) => tracing::warn!("MiCharge {name}({value}) returned {status}"),
                None => tracing::warn!("MiCharge {name}({value}) failed"),
            }
        }
    }

    fn power_source_changed(&self) -> bool {
        // Cheap probe only: four light getters, no full snapshot. A missing or
        // dead binder must escalate (return true) so the adapter can recover.
        let Some(proxy) = self.proxy() else {
            return true;
        };
        let (Some(dp_connected), Some(capacity), Some(quick_charge_type), Some(power_max)) = (
            fetch(&*proxy, |p| p.isDPConnected()),
            fetch(&*proxy, |p| p.getBatteryCapacity()),
            fetch(&*proxy, |p| p.getQuickChargeType()),
            fetch(&*proxy, |p| p.getChargingPowerMax()),
        ) else {
            return true;
        };

        let current = ProbeSnapshot {
            initialized: true,
            dp_connected,
            capacity,
            quick_charge_type,
            power_max,
        };
        let mut cache = self.probe.lock();
        let changed = !cache.initialized || *cache != current;
        *cache = current;
        changed
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
}
