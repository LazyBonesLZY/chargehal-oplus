// Lenovo battery HAL interface.
//
// Reconstructed from the official V5 AIDL library shipped on device
// (vendor.lenovo.hardware.battery-V5-ndk.so). The method order below is
// load-bearing: AIDL assigns transaction codes by declaration order, and the
// official library uses codes 1..53 with no gaps and no reserved slots.
// Do not reorder without re-checking against the device library.
//
// descriptor:      vendor.lenovo.hardware.battery.IBattery
// interface hash:  7ecc7f65d867ea475e72a3598c065dd7e5c303dc
// interface version: 5
//
// Evidence for every transaction code (entry address, `mov w1, #imm`, and the
// `bl AIBinder_transact` call site) lives in
// chargehal-vendor-refs/lenovo/INTERFACE.md §3.
//
// Getter/setter shape: every getter returns its value; every setter takes the
// new value first and returns a success flag. `*ForNum` methods carry a
// per-battery index, `Portx*` methods a per-port index — two independent index
// spaces, never interchangeable.

package vendor.lenovo.hardware.battery;

@VintfStability
interface IBattery {
    // ── transaction codes 1..9 ──
    boolean getBatteryMaintenanceEnabledV2();          // 1
    String getChargerType();                           // 2
    boolean isBatteryMaintenanceEnabled();             // 3
    boolean setBatteryChargeDisabled(boolean in);      // 4
    boolean setBatteryMaintenanceEnabled(boolean in);  // 5
    boolean setBatteryMaintenanceEnabledV2(boolean in); // 6
    boolean setUsbSupplyDisabled(boolean in);          // 7
    boolean setShipModeState(int in);                  // 8
    boolean setBatteryActivateDate(in String in);      // 9

    // ── transaction codes 10..19 ──
    String getBatteryActivateDate();                   // 10
    String getBatteryProduceDate();                    // 11
    boolean setBatteryProtectedLevel(int in);          // 12
    int getBatteryProtectedLevel();                    // 13
    boolean setMaxBatteryChargingLevel(int in);        // 14
    int getMaxBatteryChargingLevel();                  // 15
    boolean setBatteryRechargingPercent(int in);       // 16
    int getBatteryRechargingPercent();                 // 17
    int getBatterySOH();                               // 18
    int getBatteryCV();                                // 19

    // ── transaction codes 20..29 (port-indexed, then scalars) ──
    int getPortxVoltageNow(int port);                  // 20
    int getPortxCurrentNow(int port);                  // 21
    long getBatteryTimeToFullNow();                    // 22
    long getBatteryTimeToEmptyNow();                   // 23
    boolean getPortxGPIO(int port);                    // 24
    boolean setStylusQiCommand(int in);                // 25
    int getBatteryTemperature();                       // 26
    int getBatteryCycleCount();                        // 27
    int getBatteryVoltage();                           // 28
    int getBatteryCurrent();                           // 29

    // ── transaction codes 30..39 (per-battery variants) ──
    boolean setBatteryActivateDateForNum(int num, in String in); // 30
    String getBatteryActivateDateForNum(int num);      // 31
    String getBatteryProduceDateForNum(int num);       // 32
    int getBatterySOHForNum(int num);                  // 33
    int getBatteryCVForNum(int num);                   // 34
    int getBatteryCycleCountForNum(int num);           // 35
    int getBatteryVoltageForNum(int num);              // 36
    int getBatteryCurrentForNum(int num);              // 37
    int getBatteryTemperatureForNum(int num);          // 38
    String getBatteryManufacturerForNum(int num);      // 39

    // ── transaction codes 40..49 ──
    boolean setBypassLevel(int in);                    // 40
    int getBypassLevel();                              // 41
    boolean setVirtualBypassBatteryLevel(int in);      // 42
    int getVirtualBypassBatteryLevel();                // 43
    int getBatteryAbnormalStatus();                    // 44
    boolean setStylusBotCommand(in String in);         // 45
    boolean writeCommandToNode(in String node, in String cmd); // 46
    String readCommandFromNode(in String node);        // 47
    int getBatteryCapacity();                          // 48
    int getBatteryChargeCounter();                     // 49

    // ── transaction codes 50..53 ──
    int getBatteryChargeFull();                        // 50
    int getBatteryChargeFullDesign();                  // 51
    String getChargeAdapterPower();                    // 52
    String getChargeAdapterType();                     // 53

    // AIDL metadata. Declared explicitly because rsbinder-aidl only emits the
    // generated accessors when the interface names them; the official library
    // answers both at LAST_CALL_TRANSACTION / LAST_CALL_TRANSACTION - 1.
    int getInterfaceVersion();
    String getInterfaceHash();
}
