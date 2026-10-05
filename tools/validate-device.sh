#!/bin/bash

set -eu

ADB="${ADB:-adb}"
CYCLES="${1:-10}"
MAX_WAKE_MS="${MAX_WAKE_MS:-1000}"
REQUIRE_CHARGING="${REQUIRE_CHARGING:-0}"
SERVICE_NAME="vendor.oplus.hardware.charger.ICharger/default"
PROCESS_NAME="vendor.oplus.hardware.charger-V11-service"

case "$CYCLES:$MAX_WAKE_MS" in
    *[!0-9:]*|0:*|*:0)
        echo "error: cycles and MAX_WAKE_MS must be positive integers" >&2
        exit 2
        ;;
esac

case "$REQUIRE_CHARGING" in
    0|1) ;;
    *)
        echo "error: REQUIRE_CHARGING must be 0 or 1" >&2
        exit 2
        ;;
esac

if ! "$ADB" get-state >/dev/null 2>&1; then
    echo "error: no adb device connected" >&2
    exit 1
fi

service_check() {
    "$ADB" shell service check "$SERVICE_NAME" 2>/dev/null | tr -d '\r'
}

process_pid() {
    "$ADB" shell pidof "$PROCESS_NAME" 2>/dev/null | tr -d '\r'
}

process_stat_field() {
    pid="$1"
    field="$2"
    "$ADB" shell "cat /proc/$pid/stat 2>/dev/null" | awk -v field="$field" '{ print $field }' | tr -d '\r'
}

process_cpu_ticks() {
    pid="$1"
    "$ADB" shell "cat /proc/$pid/stat 2>/dev/null" | awk '{ print $14 + $15 }' | tr -d '\r'
}

wait_for_awake() {
    attempt=0
    while [ "$attempt" -lt 100 ]; do
        if "$ADB" shell dumpsys power 2>/dev/null | grep -Eq 'mWakefulness=Awake|Display Power: state=ON'; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 0.05
    done
    return 1
}

is_powered() {
    "$ADB" shell dumpsys battery 2>/dev/null \
        | grep -Eq 'AC powered: true|USB powered: true|Wireless powered: true|Dock powered: true'
}

require_powered() {
    if [ "$REQUIRE_CHARGING" = "1" ] && ! is_powered; then
        echo "error: device is not externally powered during the charging wake test" >&2
        exit 1
    fi
}

echo "==> Binder service"
service_check

INITIAL_PID="$(process_pid)"
if [ -z "$INITIAL_PID" ]; then
    echo "error: charger HAL process is not running" >&2
    exit 1
fi
echo "pid=$INITIAL_PID"
INITIAL_CPU_TICKS="$(process_cpu_ticks "$INITIAL_PID")"
INITIAL_NICE="$(process_stat_field "$INITIAL_PID" 19)"
if [ -n "$INITIAL_NICE" ] && [ "$INITIAL_NICE" -gt 0 ]; then
    echo "error: charger HAL process starts at background nice level $INITIAL_NICE" >&2
    exit 1
fi
echo "process_nice=${INITIAL_NICE:-unknown} max_wake_ms=$MAX_WAKE_MS"
require_powered
if [ "$REQUIRE_CHARGING" = "1" ]; then
    echo "charging_test=required"
fi

echo "==> Wake locks before test"
"$ADB" shell dumpsys power 2>/dev/null | sed -n '/Wake Locks:/,/Suspend Blockers:/p' || true

METRICS_FILE="$(mktemp)"
trap 'rm -f "$METRICS_FILE"' EXIT

cycle=1
while [ "$cycle" -le "$CYCLES" ]; do
    "$ADB" shell input keyevent KEYCODE_SLEEP
    sleep 2

    sleep_state="$(process_stat_field "$INITIAL_PID" 3)"
    if [ "$sleep_state" = "D" ]; then
        echo "error: charger HAL is stuck in uninterruptible sleep during cycle $cycle" >&2
        exit 1
    fi

    start_ns="$(date +%s%N)"
    "$ADB" shell input keyevent KEYCODE_WAKEUP
    if ! wait_for_awake; then
        echo "error: device did not wake during cycle $cycle" >&2
        exit 1
    fi
    end_ns="$(date +%s%N)"
    elapsed_ms=$(((end_ns - start_ns) / 1000000))
    printf '%s\n' "$elapsed_ms" >> "$METRICS_FILE"

    if [ "$elapsed_ms" -gt "$MAX_WAKE_MS" ]; then
        echo "error: wake cycle $cycle took ${elapsed_ms}ms (limit ${MAX_WAKE_MS}ms)" >&2
        exit 1
    fi

    current_pid="$(process_pid)"
    if [ -z "$current_pid" ] || [ "$current_pid" != "$INITIAL_PID" ]; then
        echo "error: charger HAL restarted during cycle $cycle (pid=$current_pid)" >&2
        exit 1
    fi

    require_powered

    echo "cycle=$cycle wake_ms=$elapsed_ms pid=$current_pid sleep_state=${sleep_state:-unknown}"
    cycle=$((cycle + 1))
done

WAKE_SUMMARY="$(awk '
    NR == 1 { min = max = $1 }
    { sum += $1; if ($1 < min) min = $1; if ($1 > max) max = $1; values[NR] = $1 }
    END {
        for (i = 1; i <= NR; i++) {
            for (j = i + 1; j <= NR; j++) {
                if (values[j] < values[i]) {
                    temp = values[i]; values[i] = values[j]; values[j] = temp
                }
            }
        }
        p95_index = int((NR * 95 + 99) / 100)
        printf "min_ms=%d avg_ms=%.1f p95_ms=%d max_ms=%d", min, sum / NR, values[p95_index], max
    }
' "$METRICS_FILE")"
echo "==> Wake latency: $WAKE_SUMMARY"

echo "==> HAL-related wake locks after test"
WAKELOCK_OUTPUT="$("$ADB" shell dumpsys power 2>/dev/null | grep -iE 'oplus.*charger|charger.*hal' || true)"
if [ -n "$WAKELOCK_OUTPUT" ]; then
    echo "$WAKELOCK_OUTPUT"
    echo "warning: inspect the lines above for a held wake lock" >&2
else
    echo "none found in dumpsys power"
fi

echo "==> Kernel wakeup sources"
"$ADB" shell su 0 sh -c \
    "cat /sys/kernel/debug/wakeup_sources 2>/dev/null | grep -iE 'oplus.*charger|charger.*hal' || true" \
    2>/dev/null || echo "unavailable (root/debugfs required)"

echo "==> Suspend-control wake locks"
for suspend_service in suspend_control_internal suspend_control; do
    output="$($ADB shell dumpsys "$suspend_service" 2>/dev/null | grep -iE 'oplus.*charger|charger.*hal' || true)"
    if [ -n "$output" ]; then
        echo "$suspend_service:"
        echo "$output"
    fi
done

FINAL_CPU_TICKS="$(process_cpu_ticks "$INITIAL_PID")"
if [ -n "$INITIAL_CPU_TICKS" ] && [ -n "$FINAL_CPU_TICKS" ]; then
    echo "==> HAL CPU ticks during test: $((FINAL_CPU_TICKS - INITIAL_CPU_TICKS))"
fi

echo "==> Recent HAL errors"
"$ADB" logcat -d -T 5m 2>/dev/null \
    | grep -iE 'OplusChargerHAL|vendor.oplus.hardware.charger' \
    | grep -iE 'fatal|panic|anr|stuck|deadlock|failed|error' \
    | tail -50 || true

echo "==> Final Binder service"
service_check

echo "PASS: $CYCLES lock/wake cycles completed without HAL restart or wake timeout"
