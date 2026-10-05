# OPlus Charger HAL Adapter

Rust/NDK implementation of the native `ICharger` HAL for Xiaomi devices
running ColorOS. It bridges the Xiaomi vendor charging HAL when that HAL is
reachable and falls back to reading standard `power_supply` and Xiaomi
`qcom-battery` sysfs nodes when it is not, then exposes:

```text
vendor.oplus.hardware.charger.ICharger/default
```

This is a ROM integration component, not an APK.

> [!WARNING]
> This service runs as root and writes device-specific charging nodes. Test it
> only on a recoverable device with a complete backup. Incorrect integration
> can break charging, boot, or battery reporting.

## Compatibility

- Android API 30 through 37. `rsbinder` also supports API 29.
- `aarch64-linux-android` / `arm64-v8a` only.
- Native link target: Android API 26.
- Tested toolchain: Rust stable and Android NDK r29.
- VINTF AIDL version: 6.
- Binder stable-interface metadata: version 11.

The API range describes Binder protocol compatibility. Hardware behavior still
depends on the target kernel, ROM, SELinux policy, and available sysfs nodes.

## Features

- Battery, USB, PD, and wireless charging state.
- Vendor-HAL bridging first: data comes from `vendor.xiaomi.hardware.micharge`
  when that HAL is reachable, with direct kernel-node reads as the fallback.
- Background cache refreshed by power-supply uevents and timed polling.
- Fast-charge classification with USB data-port protection.
- Charge limiting through the vendor HAL, with a node-based fallback.
- Compatibility stubs for unsupported ColorOS calls.

## Charging backends

Three backends implement the same `ChargeBackend` trait, and the adapter picks one
at startup, in this order:

1. **Xiaomi MiCharge HAL** ([`src/backend/micharge.rs`](src/backend/micharge.rs)) —
   preferred. Talks to `vendor.xiaomi.hardware.micharge.IMiCharge/default` over
   live AIDL and moves the HAL's string results into the adapter snapshot. The
   vendor HAL already absorbs the per-model kernel layout, which is why it wins:
   node names, units and permissions differ between kernel generations.
2. **Xiaomi HIDL-generation nodes** ([`src/backend/hidl.rs`](src/backend/hidl.rs)) —
   used when the AIDL service is unreachable but the device declares the Xiaomi
   HIDL 1.0 generation
   (`/vendor/etc/vintf/manifest/vendor.xiaomi.hardware.micharge@1.0.xml`). It
   reads that generation's own node map (`power_supply` / `qcom-battery`,
   branch-selected by `ro.board.platform`) directly. This is deliberately **not**
   an HIDL RPC client: the transport needs device-only C++ proxy classes and a
   second binder domain this crate's AIDL-only stack cannot open, and the vendor
   implementation returns those same node contents verbatim, so the node map is
   the honest data path here.
3. **Kernel nodes** ([`src/backend/sysfs.rs`](src/backend/sysfs.rs)) — fallback.
   Reads `power_supply` and `qcom-battery` nodes directly. Always available, so a
   device without either vendor generation still reports sane charging data.

If the vendor HAL dies mid-session the bridge reconnects on the next scan; while
it is unreachable every call is delegated to the node reader, so a dead HAL
degrades to the fallback rather than stalling the poll worker.

### Unit handling

The vendor HAL performs no conversion — it returns node contents verbatim — so
every unit decision is ours. Node evidence is graded and the grade decides how a
value may be used:

- **A grade** — the node is standard Linux `power_supply` ABI, so the unit is
  fixed by that ABI and the value is used as-is (`capacity` %, `voltage_now` µV,
  `current_now` µA, `temp` 0.1 °C, `charge_full`/`charge_counter` µAh,
  `cycle_count` count).
- **C grade** — the node is vendor-private (`xm_power/*`, `qcom-battery/*`) and
  has no documented scale. These values are copied through **verbatim, never
  scaled**. A guessed factor is worse than an unconverted number, because it
  looks plausible.

There is no B grade: the HAL contains no arithmetic at all, so nothing can be
reverse-derived from it. `adapter_power_w` is the one known exception — it reads
a private node through a W/mW/µW threshold heuristic and needs a device reading
to confirm.

## Implementation notes

Details that will bite anyone editing the bridge:

- **`IMiCharge.aidl` declaration order is the transaction code.** It was
  recovered by disassembling the on-device interface library and matches codes
  1..56. Reordering the file silently breaks every call.
- **`getMiChargePath` must not be called with the keys `set_cycle_power` or
  `unset_cycle_power`.** The latter makes the vendor HAL kill itself
  (`raise(SIGINT)` + `raise(SIGALRM)`). The interface is currently not used at
  all, which is why this is latent rather than active.
- **`setMiChargePath` is not a general write path.** An unknown key makes the HAL
  return 0 without writing anything, so a caller would read success.
- **`setCoolModeState` is a stub** — it shares its address with the getter and
  only logs. Charge limiting uses `setInputSuspendState`, the only working
  restrict switch on this generation.
- **The HAL returns whole node contents**, not the first line: multi-line values
  keep their inner newlines, so a numeric parse on such a node fails and the
  previous value is retained.
- **Node reads never zero a field.** A failed or missing read keeps the previous
  value (`update_int_from_paths` and friends). Preserve that when adding reads.
- **Same leaf name does not mean the same quantity across kernel generations.**
  Of 29 shared node names only 6 are safe to treat as equivalent; `soh`,
  `soc_decimal`, `power_max` and `resistance` all differ in meaning or scale.
- **`tools/validate-static.sh` asserts on source text.** Renaming a function or
  moving code makes it fail; update the script alongside the change.
- **`src/lib.rs` needs `pub mod backend;`** — a private module chain turns a batch
  of public items into dead code and fails `clippy -D warnings`.

## Build

Requirements: Linux, Rust stable, `aarch64-linux-android`, Android NDK r29, and
`ripgrep` for static validation.

```bash
rustup target add aarch64-linux-android
export ANDROID_NDK_HOME="$HOME/Android/Sdk/ndk/29.0.14206865"
./build.sh release
```

For a debug build:

```bash
./build.sh debug
```

The release binary and Soong package are generated under `target/` and
`dist/`. These directories are ignored by Git.

## ROM Integration

1. Add the source tree to the ROM/device build tree.
2. Run `./build.sh release` before invoking Soong.
3. Include `Android.bp`, `charger-hal-service.rc`, and `charger-hal-service.xml`.
4. Build the `vendor.oplus.hardware.charger-V6-service` module. It installs to `/odm/bin/hw`.
5. Add target-specific SELinux rules and verify all writable sysfs nodes.
6. Check VINTF merging, init logs, and Binder registration before flashing.

The project does not provide generic SELinux policy and does not support live
binary replacement as a complete installation method.

## Validation

```bash
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
./tools/validate-static.sh
```

After installation, run the device lock/wake test from a connected ADB device:

```bash
REQUIRE_CHARGING=1 MAX_WAKE_MS=1000 ./tools/validate-device.sh 10
```

This test checks service restarts, uninterruptible sleep, wake locks, and wake
latency. Do not run it on a primary device.

## Limitations

- This is a device-specific Xiaomi/ColorOS compatibility layer, not a generic Android HAL.
- **Nothing here has been verified on a device.** The build, the unit tests and
  the static checks pass; neither the AIDL bridge nor the HIDL-generation node
  map has been observed against a real HAL. Treat the first device run as the
  real test.
- The HIDL-generation backend is a direct-node reader keyed to that
  generation's mapping, not an HIDL RPC client. It cannot see anything the
  vendor HAL process itself would refuse to serve, and its `qcom-battery` /
  `power_supply` branch choice follows the constructor rule recovered from the
  binary.
- The bridge targets the AIDL V2 generation of the Xiaomi HAL. A device that only
  ships the HIDL 1.0 generation uses the HIDL-generation node backend, which
  reads that generation's node map directly instead of speaking HIDL RPC.
- Several `ICharger` methods are specified to return the raw contents of OPPO
  private nodes (`/sys/class/oplus_chg/*`, `/proc/charger/*`, `/proc/wireless/*`).
  Those nodes do not exist on Xiaomi kernels, so such calls return an empty
  string. `queryChargeInfo` and `getPsyBatteryStatus` follow the official wire
  format; the remaining gaps are documented in the source.
- **Decimal SOC is collected but not served.** The vendor HAL's decimal node is
  not the one the official contract names, and its scale (×100 vs ×1000) is
  unconfirmed, so it is surfaced only through the SOH debug interface until a
  device reading settles it.
- **USB data-port protection is incomplete on the bridge path.** The node that
  could carry `usb_type` has an unverified value domain, and reading it wrong
  would classify every charger as a data port; only `pc_port_online` is used.
- Charging node names, units, permissions, and control behavior vary by kernel.
- Some OPlus methods are stubs because the target Xiaomi kernel lacks the corresponding hardware.
- Authentication and short-circuit health values include target-specific compatibility behavior.
- Keep VINTF version 6 and Binder metadata version 11 together; changing only one can break integration.

## License

Project source is licensed under Apache-2.0. See [`LICENSE`](LICENSE).
