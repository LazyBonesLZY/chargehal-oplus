# OPlus Charger HAL Adapter

Rust/NDK implementation of `vendor.oplus.hardware.charger.ICharger/default`
(V11, 132 methods) for Xiaomi devices running ColorOS. Prefers the Xiaomi
`IMiCharge` vendor HAL when reachable, falls back to kernel-node reads, and
publishes the OPlus contract to `/odm/bin/hw`.

> [!WARNING]
> Root service that writes charging nodes. Test only on a recoverable device
> with a full backup. Bad integration can break charging, boot, or battery
> reporting.

## Compatibility

- Android API 30–37 (`rsbinder` also tolerates 29); `aarch64-linux-android` only.
- VINTF AIDL 11, Binder metadata version 11, service binary
  `vendor.oplus.hardware.charger-V11-service`. Keep the three on 11 together.

## Backends (selection order)

1. **MiCharge HAL** (`src/backend/micharge.rs`) — live AIDL proxy. The vendor
   HAL absorbs per-model kernel layout, which is why it wins.
2. **HIDL-generation nodes** (`src/backend/hidl.rs`) — direct reads of that
   generation's `power_supply` / `qcom-battery` map, branch-selected by
   `ro.board.platform`. Explicitly not an HIDL RPC client (device-only C++
   proxies, and our process already owns its single binder domain); the vendor
   implementation returns those node contents verbatim anyway.
3. **Kernel nodes** (`src/backend/sysfs.rs`) — generic fallback, always available.

A dead HAL degrades to the fallback instead of stalling the poll worker.

## Units

The HAL does no conversion — it returns node text verbatim — so every unit
decision is ours, graded by evidence:

- **A** — standard `power_supply` ABI, used as-is (%, µV, µA, 0.1 °C, µAh, count).
- **C** — vendor-private nodes (`xm_power/*`, `qcom-battery/*`): copied verbatim,
  never scaled. A guessed factor looks plausible and is worse than no conversion.

No B grade exists (the HAL contains no arithmetic). `adapter_power_w` is the one
exception: private node through a W/mW/µW heuristic, needs a device reading.

## Editing rules

- `IMiCharge.aidl` declaration order **is** the transaction code (1..56).
  Never reorder.
- Never call `getMiChargePath` with `set_cycle_power` / `unset_cycle_power` —
  the latter makes the vendor HAL kill itself (`raise(SIGINT)+raise(SIGALRM)`).
- `setMiChargePath` returns 0 for unknown keys without writing. Not a general
  write path.
- Charge limiting uses `setInputSuspendState`; `setCoolModeState` is a stub.
- Failed reads keep the previous value (`update_int_from_paths` and friends).
  Never zero a field on a failed read.
- Same leaf name ≠ same quantity across generations: of 29 shared node names
  only 6 are equivalent; `soh`, `soc_decimal`, `power_max`, `resistance` differ.
- `tools/validate-static.sh` asserts on source text. Update it with the change.

## Build

Linux, Rust stable, `aarch64-linux-android`, NDK r29, `ripgrep`.

```bash
rustup target add aarch64-linux-android
export ANDROID_NDK_HOME="$HOME/Android/Sdk/ndk/29.0.14206865"
./build.sh release
```

Binaries land in `target/` and `dist/` (both git-ignored).

## ROM integration

1. Add the source tree to the device build tree.
2. Run `./build.sh release` before Soong.
3. Include `Android.bp`, `charger-hal-service.rc`, `charger-hal-service.xml`.
4. Build `vendor.oplus.hardware.charger-V11-service` (installs to `/odm/bin/hw`).
5. Add target SELinux rules; verify every writable sysfs node; check VINTF
   merging, init logs, and Binder registration before flashing.

No generic SELinux policy here; live binary replacement is not a supported
install method.

## Validation

```bash
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
./tools/validate-static.sh
```

On-device lock/wake test (not on a primary device):

```bash
REQUIRE_CHARGING=1 MAX_WAKE_MS=1000 ./tools/validate-device.sh 10
```

## Limitations

- **Never verified on a device.** Builds, tests, and static checks pass; the
  first device run is the real test.
- OPPO private nodes (`/sys/class/oplus_chg/*`, `/proc/charger/*`,
  `/proc/wireless/*`) do not exist on Xiaomi kernels — those calls return "".
- Decimal SOC is collected but not served: the HAL's node is not the one the
  contract names and its scale is unconfirmed.
- Data-port protection on the AIDL path relies on `pc_port_online` alone
  (`usb_type`'s domain is unverified); the HIDL path uses its own `has_dp`.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
