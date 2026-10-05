#!/bin/bash

set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/dist/vendor.oplus.hardware.charger-V11-service}"

ADAPTER="$ROOT/src/adapter.rs"
SYSFS="$ROOT/src/backend/sysfs.rs"
MICHARGE="$ROOT/src/backend/micharge.rs"
BACKEND_MOD="$ROOT/src/backend/mod.rs"
MICHARGE_AIDL="$ROOT/aidl/vendor/xiaomi/hardware/micharge/IMiCharge.aidl"

fail() {
    echo "error: $1" >&2
    exit 1
}

# ── Interface metadata ──

INTERFACE_VERSION="$(sed -n 's/^pub const INTERFACE_VERSION: i32 = \([0-9][0-9]*\);$/\1/p' "$ROOT/src/lib.rs")"
VINTF_VERSION="$(sed -n 's/^[[:space:]]*<version>\([0-9][0-9]*\)<\/version>.*$/\1/p' "$ROOT/charger-hal-service.xml")"
[ -n "$INTERFACE_VERSION" ]
[ "$INTERFACE_VERSION" = "11" ]
[ "$VINTF_VERSION" = "11" ]

if ! rg -q '^rsbinder = \{ version = "=0\.10\.0", features = \["android_10_plus"\] \}$' "$ROOT/Cargo.toml" \
    || ! rg -q '^rsbinder-aidl = "=0\.10\.0"$' "$ROOT/Cargo.toml"; then
    fail "Binder dependency no longer enables Android 10 through 17 compatibility"
fi

# ── Binder runtime scheduling ──

if rg -n 'disable_background_scheduling|setpriority|nice\(' "$ROOT/src/main.rs"; then
    fail "Binder runtime scheduling was modified"
fi

# ── Polling cadence ──

if ! rg -q '^const SCREEN_OFF_CHARGING_POLL_INTERVAL: Duration = Duration::from_secs\(5\);$' "$ADAPTER" \
    || ! rg -q '^const FULL_REFRESH_INTERVAL: Duration = Duration::from_secs\(30\);$' "$ADAPTER" \
    || ! rg -q '^const SCREEN_WAKE_SCAN_DEFER: Duration = Duration::from_millis\(750\);$' "$ADAPTER"; then
    fail "charging probe/full-refresh/wake-defer timing changed"
fi

POLL_PRIORITY_BODY="$(sed -n '/fn lower_poll_thread_priority/,/^}/p' "$ADAPTER")"
if ! printf '%s\n' "$POLL_PRIORITY_BODY" | rg -q 'setpriority.*10'; then
    fail "poll worker no longer yields to display work"
fi

# ── Backend selection: live AIDL HAL, HIDL-generation nodes, kernel nodes ──

[ -f "$MICHARGE" ] || fail "MiCharge HAL backend is missing"
[ -f "$ROOT/src/backend/hidl.rs" ] || fail "HIDL-generation node backend is missing"
[ -f "$SYSFS" ] || fail "sysfs fallback backend is missing"

LENOVO="$ROOT/src/backend/lenovo.rs"
LENOVO_AIDL="$ROOT/aidl/vendor/lenovo/hardware/battery/IBattery.aidl"
[ -f "$LENOVO" ] || fail "Lenovo battery HAL backend is missing"
[ -f "$LENOVO_AIDL" ] || fail "Lenovo IBattery AIDL declaration is missing"

if ! rg -q 'LenovoBackend::connect\(\)' "$BACKEND_MOD"; then
    fail "backend selection no longer probes the Lenovo vendor HAL"
fi
if ! rg -q 'MiChargeBackend::connect\(\)' "$BACKEND_MOD"; then
    fail "backend selection no longer probes the Xiaomi vendor HAL"
fi
if ! rg -q 'HidlBackend::new\(\)' "$BACKEND_MOD"; then
    fail "backend selection no longer routes to the HIDL-generation backend"
fi
if ! rg -q 'select_kind' "$BACKEND_MOD"; then
    fail "backend selection rule is no longer a testable function"
fi
if ! rg -q 'SysfsBackend::new\(\)' "$BACKEND_MOD"; then
    fail "backend selection has no kernel-node fallback"
fi
# The HAL probe must only run on device: on the host there is no servicemanager.
if ! rg -q -U 'cfg\(target_os = "android"\)[\s\S]{0,400}MiChargeBackend::connect' "$BACKEND_MOD"; then
    fail "vendor HAL probe is no longer gated to Android builds"
fi

for impl_block in "fn refresh" "fn set_charge_control" "fn power_source_changed"; do
    if ! rg -q "$impl_block" "$MICHARGE"; then
        fail "MiCharge backend does not implement ${impl_block#fn }"
    fi
    if ! rg -q "$impl_block" "$LENOVO"; then
        fail "Lenovo backend does not implement ${impl_block#fn }"
    fi
done

# The Lenovo bridge must talk to the vendor HAL rather than reimplement it, and
# its charge control must drive the vendor setter.
if ! rg -q 'LENOVO_SERVICE_NAME' "$LENOVO"; then
    fail "Lenovo backend no longer resolves the vendor service by name"
fi
if ! rg -q 'setUsbSupplyDisabled' "$LENOVO"; then
    fail "Lenovo charge control no longer drives the vendor HAL"
fi

# The bridges must not carry a panic path into a root service.
if rg -n 'unwrap\(\)|expect\(|panic!\(|unreachable!\(' "$MICHARGE" "$ROOT/src/backend/hidl.rs" "$LENOVO"; then
    fail "panic path found in a HAL bridge backend"
fi

# Authenticity is a fixed, documented answer, never a forwarded vendor flag.
# The OPlus contract asks whether the pack is an OPlus original; on Xiaomi and
# Lenovo hardware that can only ever be "no", and ColorOS turns a 0 into a
# non-genuine-battery warning. A backend that starts sourcing this field would
# reintroduce that false alarm, so pin the policy here.
if ! rg -q '^pub const AUTHENTIC_REPORTED: i32 = 1;$' "$ADAPTER"; then
    fail "the reported battery-authenticity constant changed"
fi
if rg -n 'update_int_from_paths\(&mut info\.authentic|&mut info\.authentic,' "$ROOT/src/backend"; then
    fail "a backend started sourcing battery authenticity from a node or HAL getter"
fi

# The HIDL-generation backend is a direct-node reader, never an RPC client:
# none of the HIDL transport symbols may appear in source. (Plain prose
# mentioning "HIDL" is fine; these tokens would mean someone started building
# the client ABI by hand.)
if rg -n 'libhidlbase|hwservicemanager|hidl_string|BpHw|BnHw|HIDL_FETCH|::getService|registerAsService|configureRpc|joinRpc|/dev/hwbinder' "$ROOT/src"; then
    fail "HIDL transport client symbols found in source"
fi

# The fallback backend must always be usable: it depends on nothing external.
SYSFS_AVAILABLE_BODY="$(sed -n '/fn is_available/,/^    }/p' "$SYSFS")"
if ! printf '%s\n' "$SYSFS_AVAILABLE_BODY" | rg -q 'true'; then
    fail "sysfs fallback backend is no longer always available"
fi

# ── OPlus interface contract ──
#
# The declaration order of these methods *is* their transaction code, and the
# ColorOS client indexes by it. A reorder or a dropped method is invisible to the
# compiler, so pin the count, both endpoints and the advertised hash.

OPLUS_AIDL="$ROOT/aidl/vendor/oplus/hardware/charger/ICharger.aidl"
[ -f "$OPLUS_AIDL" ] || fail "ICharger.aidl is missing"

OPLUS_METHODS="$(rg -c '^\s+[A-Za-z][A-Za-z0-9_]* [A-Za-z][A-Za-z0-9_]*\(' "$OPLUS_AIDL")"
if [ "$OPLUS_METHODS" != "132" ]; then
    fail "ICharger.aidl declares $OPLUS_METHODS methods, expected 130 business + 2 metadata"
fi
if ! rg -q '^\s+int VolDividerIcWorkModeSet\(in String data\);' "$OPLUS_AIDL"; then
    fail "ICharger transaction code 1 changed; the device order is fixed"
fi
if ! rg -q '^\s+String getUsbCurrentEyeDiagram\(int model\);' "$OPLUS_AIDL"; then
    fail "ICharger transaction code 130 changed; the device order is fixed"
fi
if ! rg -q '^pub const INTERFACE_HASH: &str = "046dfc7a9ca30bfca848ced6e9474f47437b0db7";$' "$ROOT/src/lib.rs"; then
    fail "ICharger interface hash no longer matches the official device library"
fi

# ── Xiaomi interface contract ──

[ -f "$MICHARGE_AIDL" ] || fail "IMiCharge.aidl is missing"
MICHARGE_METHODS="$(rg -c '^\s+(String|boolean|int) [A-Za-z][A-Za-z0-9]*\(' "$MICHARGE_AIDL")"
if [ "$MICHARGE_METHODS" != "58" ]; then
    fail "IMiCharge.aidl declares $MICHARGE_METHODS methods, expected 56 business + 2 metadata"
fi
if ! rg -q '^\s+String getBatteryAuthentic\(\);' "$MICHARGE_AIDL" \
    || ! rg -q '^\s+int setBatteryCommonInfo\(in String key, in String value\);' "$MICHARGE_AIDL"; then
    fail "IMiCharge.aidl method order no longer matches the device transaction codes"
fi
if ! rg -q 'vendor\.xiaomi\.hardware\.micharge\.IMiCharge/default' "$MICHARGE"; then
    fail "MiCharge service name changed"
fi

# ── Lenovo interface contract ──
#
# Same rule as above: declaration order *is* the transaction code. The device
# library uses codes 1..53 with no gaps, so the endpoints and the count are
# pinned. Evidence: chargehal-vendor-refs/lenovo/INTERFACE.md §3.

LENOVO_METHODS="$(rg -c '^\s+(boolean|int|long|String) [A-Za-z][A-Za-z0-9]*\(' "$LENOVO_AIDL")"
if [ "$LENOVO_METHODS" != "55" ]; then
    fail "IBattery.aidl declares $LENOVO_METHODS methods, expected 53 business + 2 metadata"
fi
if ! rg -q '^\s+boolean getBatteryMaintenanceEnabledV2\(\);' "$LENOVO_AIDL"; then
    fail "IBattery transaction code 1 changed; the device order is fixed"
fi
if ! rg -q '^\s+String getChargeAdapterType\(\);' "$LENOVO_AIDL"; then
    fail "IBattery transaction code 53 changed; the device order is fixed"
fi
if ! rg -q 'vendor\.lenovo\.hardware\.battery\.IBattery/default' "$LENOVO"; then
    fail "Lenovo service name changed"
fi
if ! rg -q '^// interface hash:  7ecc7f65d867ea475e72a3598c065dd7e5c303dc$' "$LENOVO_AIDL"; then
    fail "IBattery interface hash no longer matches the official device library"
fi

# ── Screen-transition cancellation ──

PROBE_BODY="$(sed -n '/fn power_source_probe_changed/,/^    }/p' "$SYSFS")"
[ -n "$PROBE_BODY" ] || fail "power_source_probe_changed not found; the probe assertion went stale"
if printf '%s\n' "$PROBE_BODY" | rg 'poll_once|thread::sleep|write_'; then
    fail "lightweight power-source probe became a full or blocking scan"
fi

if ! rg -q 'fn poll_once<F>.*should_cancel' "$SYSFS" \
    || ! rg -q 'backend\.refresh' "$ADAPTER" \
    || ! rg -q 'if !scan_completed' "$ADAPTER"; then
    fail "full scans are no longer cancellable during screen transitions"
fi

WAKE_CANCEL_CHECKS="$(rg -c 'screen_wake_pending\.load\(Ordering::Acquire\)' "$ADAPTER")"
if [ "$WAKE_CANCEL_CHECKS" -lt 4 ]; then
    fail "scan tail no longer yields before cache publication and charge control"
fi

# ── uevent handling ──

UEVENT_BODY="$(sed -n '/fn monitor_power_supply_uevents/,/^}/p' "$ADAPTER")"
if ! printf '%s\n' "$UEVENT_BODY" | rg -q 'request_uevent_probe' \
    || printf '%s\n' "$UEVENT_BODY" | rg -q 'request_refresh'; then
    fail "power-supply uevents can trigger an unfiltered full scan"
fi
if ! rg -q -U 'uevent_probe_pending\s*\.swap' "$ADAPTER"; then
    fail "power-supply uevent bursts are no longer coalesced"
fi

# ── Screen notification must stay a pure atomic transition marker ──

SCREEN_BODY="$(sed -n '/pub fn notify_screen_status/,/^    }/p' "$ADAPTER")"
if printf '%s\n' "$SCREEN_BODY" | rg 'read_|write_|sleep|\.lock\(|request_refresh|thread::|setpriority|tracing::'; then
    fail "blocking or I/O work found in notify_screen_status"
fi
if ! printf '%s\n' "$SCREEN_BODY" | rg -q 'screen_on\.swap' \
    || ! printf '%s\n' "$SCREEN_BODY" | rg -q 'screen_wake_pending\.store' \
    || ! printf '%s\n' "$SCREEN_BODY" | rg -q 'wake_poll_worker'; then
    fail "notify_screen_status must mark the transition and wake the poll worker"
fi

WAKE_BODY="$(sed -n '/fn wake_poll_worker/,/^    }/p' "$ADAPTER")"
if printf '%s\n' "$WAKE_BODY" | rg 'recv|sleep|\.lock\(|\.send\('; then
    fail "blocking work found in wake_poll_worker"
fi
if ! printf '%s\n' "$WAKE_BODY" | rg -q 'try_send'; then
    fail "wake_poll_worker no longer uses try_send"
fi

DECIMAL_BODY="$(sed -n '/pub fn get_decimal_soc/,/^    }/p' "$ADAPTER")"
[ -n "$DECIMAL_BODY" ] || fail "get_decimal_soc not found; the decimal-soc assertion went stale"
if printf '%s\n' "$DECIMAL_BODY" | rg 'read_|write_|sleep|request_refresh|thread::'; then
    fail "I/O or scheduling work found in get_decimal_soc"
fi

# ── Charge control: current limit, never input suspend ──
#
# Writing `input_suspend` drops the charger offline for as long as the value
# sticks, and the framework closes bypass charging the moment it stops seeing a
# charger. Both node-driven paths must limit the current instead.

HIDL_CTRL="$(sed -n '/fn set_charge_control/,/^    }/p' "$ROOT/src/backend/hidl.rs")"
[ -n "$HIDL_CTRL" ] || fail "hidl set_charge_control not found; the charge-control assertion went stale"
if printf '%s\n' "$HIDL_CTRL" | rg 'INPUT_SUSPEND'; then
    fail "hidl charge control writes input_suspend: the charger drops offline and bypass charging is closed"
fi

SYSFS_CTRL="$(sed -n '/pub fn apply_charge_control_limit/,/^}/p' "$SYSFS")"
[ -n "$SYSFS_CTRL" ] || fail "apply_charge_control_limit not found; the charge-control assertion went stale"
if printf '%s\n' "$SYSFS_CTRL" | rg 'INPUT_SUSPEND'; then
    fail "apply_charge_control_limit writes input_suspend: the charger drops offline and bypass charging is closed"
fi
if ! printf '%s\n' "$SYSFS_CTRL" | rg -q 'NIGHT_CHARGING'; then
    fail "apply_charge_control_limit no longer drives the no-charge gate; the current limit alone still charges the pack"
fi

MICHARGE_CTRL="$(sed -n '/fn set_charge_control/,/^    }/p' "$MICHARGE")"
[ -n "$MICHARGE_CTRL" ] || fail "micharge set_charge_control not found; the assertion went stale"
if printf '%s\n' "$MICHARGE_CTRL" | rg 'setInputSuspendState'; then
    fail "micharge charge control suspends the input: the charger drops offline and bypass charging is closed"
fi
if ! printf '%s\n' "$MICHARGE_CTRL" | rg -q 'setNightChargingState'; then
    fail "micharge charge control no longer uses the vendor no-charge gate"
fi

# ── Hardware bypass stays with the HAL ──
#
# A backend whose HAL implements bypass must not also run the adapter's
# node-level stand-in: on Lenovo that would suspend the input through
# `setUsbSupplyDisabled` and the charger would drop offline.

LENOVO_BYPASS="$(sed -n '/fn has_hardware_bypass/,/^    }/p' "$ROOT/src/backend/lenovo.rs")"
[ -n "$LENOVO_BYPASS" ] || fail "lenovo has_hardware_bypass not found; the assertion went stale"
if ! printf '%s\n' "$LENOVO_BYPASS" | rg -q 'true'; then
    fail "lenovo no longer reports a hardware bypass: the adapter would run its stand-in on top of setBypassLevel"
fi

# ── Kernel paths ──

if rg -n '/(sys|proc)/[^" ]*(oplus|oppo)|/proc/wireless' "$ROOT/src"; then
    fail "OPlus-only kernel path found in source"
fi

if rg -n 'group .*wakelock|write /sys/power/wake' "$ROOT/charger-hal-service.rc"; then
    fail "init service requests wake-lock access"
fi

# ── Built binary ──

if [ -f "$BIN" ]; then
    if strings -a "$BIN" | rg -i 'wake_lock|wake_unlock|alarmtimer|timerfd_create|autosuspend|suspend_blocker|/sys/power/wake'; then
        fail "wake-capable API found in service binary"
    fi
    if strings -a "$BIN" | rg '/(sys|proc)/[^ ]*(oplus|oppo)|/proc/wireless'; then
        fail "OPlus-only kernel path found in service binary"
    fi
fi

echo "PASS: metadata, backend selection, screen path, kernel paths, and wake APIs are clean"
