# 接手报告：OPlus Charger HAL 适配层

> 交接时间：2026-10-04
> 交接范围：充电数据源从「硬编码 sysfs 节点」改造为「桥接小米 vendor HAL 优先，节点兜底」
> 当前状态：代码完成、构建通过、**未经真机验证**

---

## 1. 项目是什么

一个 ROM 集成件（不是 APK）。它对外提供 OPlus/ColorOS 期望的 AIDL 服务：

```
vendor.oplus.hardware.charger.ICharger/default   （VINTF 版本 6，接口版本 11）
```

跑在小米设备上，把本机的充电数据喂给 ColorOS 的充电框架层。装在 `/odm/bin/hw`，由 init 以 root 拉起。

历史背景：这个项目最初直接读内核 sysfs 节点（`/sys/class/qcom-battery/*`）来拼数据。那套节点只在旧机型上存在，新机型的电源节点整体迁到了 `/sys/class/xm_power/*`，于是所有读数静默归零——这就是本次改造要解决的问题。

---

## 2. 本次改造做了什么

数据源抽象成一层后端，运行时自动选择：

| 后端 | 文件 | 角色 |
|---|---|---|
| 小米 MiCharge HAL | `src/backend/micharge.rs` | **优先**。通过 binder 调 `vendor.xiaomi.hardware.micharge.IMiCharge/default` |
| 内核节点 | `src/backend/sysfs.rs` | **兜底**。原来的节点读取逻辑整体搬迁 |

选择逻辑在 `src/backend/mod.rs::select()`：探测小米 HAL，成功就用它，失败回退节点读取。HAL 探测用 `#[cfg(target_os = "android")]` 门控——host 上（单元测试）没有 servicemanager，不能触发 binder 初始化。

改动清单：

| 文件 | 变化 |
|---|---|
| `src/backend/mod.rs` | 新增，trait 定义 + 探测选择（73 行） |
| `src/backend/micharge.rs` | 新增，HAL 桥接后端（511 行） |
| `src/backend/sysfs.rs` | 新增，节点兜底后端（1451 行，含 19 个随迁单测） |
| `aidl/vendor/xiaomi/hardware/micharge/IMiCharge.aidl` | 新增，小米接口定义（86 行） |
| `build.rs` | 改为生成两份绑定（OPlus + 小米），各自断言 transaction code |
| `src/lib.rs` | 加 `pub mod backend;` 和 `include!(micharge.rs)` |
| `src/adapter.rs` | 2692 → 1594 行，删掉搬迁残留，改为调 backend |
| `src/main.rs` | 两处返回格式修正 |
| `tools/validate-static.sh` | 按新结构重写，新增桥接契约断言 |
| `README.md` | 补充后端架构说明 |

总计 +341 / −1314 行。

---

## 3. 关键设计决策与依据

**为什么桥接优先**：新旧机型的节点体系完全不同（`qcom-battery` vs `xm_power`），而小米 HAL 内部已经吸收了这个差异。桥接让 HAL 去处理节点布局，我们只搬数据。

**为什么 222 旧机不桥接 HIDL**：核对了 222 的 `micharge@1.0-impl.so` 实际读写的节点，与兜底后端读的是同一批（`/sys/class/qcom-battery/*` + `/sys/class/power_supply/*`）。桥接只会把相同数据多绕一层 binder，换来的是架构一致性而非数据正确性。而 HIDL 和 AIDL 是两套传输层（`libhidlbase` vs `libbinder_ndk`），同一二进制无法同时链接，成本不低。所以只做 AIDL 桥接。

**为什么 build.rs 要 patch 生成的代码**：AIDL 规定 `getInterfaceVersion` / `getInterfaceHash` 的 transaction code 是 `0x00FFFFFF` / `0x00FFFFFE`，但 rsbinder-aidl 0.10.0 不生成这两个固定值。build.rs 改写生成文件并 `assert!` 补丁命中，上游格式变化时会显式构建失败而不是静默出错。

**为什么 `queryChargeInfo` 被重写**：官方 HAL 返回的是**换行分隔的 53 键 `key=value` 列表**，ColorOS 客户端按 key 名解析。项目原来返回的是自造的 `;` 分隔串，客户端根本读不出来。这是本次发现的最严重不兼容，键名与顺序取自官方服务（见 `FORMAT-CONTRACT.md` §3.1）。

**一个被纠正的方向性误判**：官方二进制里那批 `$$bcc@@%d`、`$$res@@%d,%d` 格式串**不是**任何 ICharger 方法的返回契约，而是内部 BCC 日志上报的编码（有调用链证据）。如果按它去改返回格式会走错方向。

**为什么不写死私有节点的换算系数**：小米 HAL 对所有 getter 都只是「读节点第一行、剥换行、原样返回」，二进制里没有任何换算代码。只有标准 `power_supply` 节点（capacity/voltage_now/current_now/temp/charge_full/cycle_count）有内核 ABI 背书的刻度，`xm_power/*` 和 `qcom-battery/*` 私有节点的单位无从确认，所以一个假设的系数都没写进代码。

---

## 4. 当前状态（已验证）

```
cargo fmt --all -- --check          干净
cargo test --all-targets            38 passed / 0 failed
cargo clippy --all-targets -- -D warnings   零告警
./tools/validate-static.sh          PASS
ANDROID_NDK_HOME=<ndk29> ./build.sh release 成功（aarch64，1008K）
./tools/validate-static.sh dist/...  PASS
```

二进制里确认包含桥接代码：`vendor.xiaomi.hardware.micharge.IMiCharge/default`、DeathRecipient 重连日志、回退日志；依赖仅 `libdl` / `libc`（rsbinder 是纯 Rust binder 实现，不需要 `libbinder`）。

**未验证的部分**：全部真机行为。没有设备可接，桥接能否连上、读数是否正确、ColorOS 是否满意返回格式，都没有实测过。

---

## 5. 已知限制与未完成项

1. **无真机验证**——最重要的一条。代码只保证编译通过和逻辑自洽。
2. **222 旧机走兜底路径**，不是桥接（决策见 §3）。
3. **13 个方法在小米设备上必然返回空串**。官方规范要求它们读 `/sys/class/oplus_chg/*`、`/proc/charger/*`、`/proc/wireless/*` 这类 OPPO 私有节点，小米内核没有这些节点。改不改都是空，所以保留了现状。
4. **私有节点单位未确认**。需要插真机读一次实际值才能定。
5. **部分返回格式尚未对齐官方**，清单在 `FORMAT-CONTRACT.md` §7：
   - P1：13 个「恒空」方法（受限于 §5.3，收益低）
   - P2：`healthd_update_ui_soc_decimal`（项目有合成小数电量逻辑，与官方直读节点不同，属产品决策）、`getWirelessTXEnable`（硬编码 `"disable"`）、`getQuickModeGain`（分隔符应为 `+`）、`getUsbCurrentEyeDiagram`（`model != 0` 时应返回异常码 −7）
   - `getChgConfig` 的逐 flag 格式未枚举（官方是运行期函数表分发）
6. **`getBatteryResistance` / `getBatteryThermaLevel` 语义在两侧不同**：171 读的是 pack 识别电阻 / 热控限流档位，不是电芯内阻 / 温度。桥接层按方法名直连会拿到错误量纲，`micharge.rs` 里对这两个 getter 刻意不调用（没有对应字段）。
7. **证据样本在 `/tmp`**（`/tmp/oplus-charger/`、`/tmp/vendor-dump/`），会被系统清理，建议归档。

---

## 6. 下一步建议

按优先级：

1. **真机验证 171**。`REQUIRE_CHARGING=1 MAX_WAKE_MS=1000 ./tools/validate-device.sh 10`，确认桥接连得上、服务不重启、唤醒延迟达标。
2. **回填单位**。拿实测值确认 `xm_power/*` 私有节点的刻度，更新 `MICHARGE-MAPPING.md` §7 与 `micharge.rs` 里标着 unconfirmed 的注释。
3. **补格式**。按 `FORMAT-CONTRACT.md` §7 的 P2 清单逐条对齐，重点是 `getUsbCurrentEyeDiagram` 的异常码语义（返回空串可能让客户端误判）。
4. **（可选）HIDL 桥接**。若将来有架构统一需求，两条路径：纯 Rust 手工实现 HIDL 客户端（需从 `libhidlbase` 还原 `hidl_string` 编码与 `_hidl_cb` 回调机制 + 50 个方法的 transaction code），或写 C++ 薄层链接原厂接口库（需先重建 `.hal` 跑 `hidl-gen`）。

---

## 7. 验证命令

```bash
# 静态检查（需要 ripgrep）
./tools/validate-static.sh

# 完整验证链
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings

# 交叉编译
export ANDROID_NDK_HOME="$HOME/Android/Sdk/ndk/29.0.14206865"
./build.sh release
./tools/validate-static.sh dist/vendor.oplus.hardware.charger-V6-service

# 真机（需先安装到 /odm/bin/hw）
REQUIRE_CHARGING=1 MAX_WAKE_MS=1000 ./tools/validate-device.sh 10
```

---

## 8. 参考资料与证据

核查报告（每条结论都附反汇编或字符串证据）。注意这些文件在**项目外的持久目录** `/home/lazybones/chargehal-vendor-refs/`，不在本仓库内——若要随仓库一起分发，需要先复制进来：

| 文件 | 内容 |
|---|---|
| `chargehal-vendor-refs/INTERFACE.md` | 两台设备的小米 HAL 契约、56 个方法签名、transaction code、底层节点体系差异 |
| `chargehal-vendor-refs/MICHARGE-MAPPING.md` | 小米 56 个方法的节点映射与单位判据、171/222 差异、键集合、未确认清单 |
| `chargehal-vendor-refs/FORMAT-CONTRACT.md` | 官方 OPlus 132 方法的返回格式契约、25 个 String 方法的对照表、修正建议 |

二进制样本：

| 位置 | 内容 |
|---|---|
| `chargehal-vendor-refs/171/`、`222/` | 小米 micharge HAL 的 service 与接口库（从设备只读提取） |
| `/tmp/oplus-charger/` | 官方 OPlus charger V11 包（service、V11/V9 ndk 接口库、HIDL 1.0 库、VINTF 声明、rc）——**易失，建议归档** |
| `/tmp/vendor-dump/` | 小米样本副本——**易失** |

---

## 9. 容易踩的坑

- **`validate-static.sh` 是文本断言，很脆**。它按函数名和字面量匹配（比如要求 `notify_screen_status` 函数体内不得出现任何 I/O 符号、三个定时常量的字面值精确匹配）。重命名函数或移动代码位置都会让它变红，改代码时要同步改它。
- **`src/lib.rs` 必须用 `pub mod backend;`**。私有模块链会让一批 pub 项触发 dead_code，clippy `-D warnings` 直接失败。
- **`charger.rs` 和 `micharge.rs` 不能同时 include 到同一个 crate root**，两者都会生成 `mod vendor`。目前 OPlus 绑定在 bin crate、小米绑定在 lib crate，是刻意分开的。
- **`IMiCharge.aidl` 的方法声明顺序就是 transaction code**。顺序由设备上的官方接口库反汇编得到（`FIRST_CALL_TRANSACTION + 0..55`），已与设备核对一致，**不要重排**。
- **节点读取失败时保留原值，不清零**。这是全项目的既有语义（`update_int_from_paths` 系列），桥接后端也遵守（`parse_int` 失败返回 `None` → `apply_*` 跳过）。有单测覆盖，别改成清零。
- **`refresh` 的 `should_cancel` 检查点不能删**。屏幕状态切换时要中断扫描并重新排期，这是项目的省电契约，兜底后端里 11 个检查点全部保留。
