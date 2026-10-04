// Xiaomi MiCharge HAL interface.
//
// Reconstructed from the official V2 AIDL library shipped on device
// (vendor.xiaomi.hardware.micharge-V2-ndk.so). The method order below is
// load-bearing: AIDL assigns transaction codes by declaration order, and the
// official library uses codes 1..50 for the V1 surface and 51..56 for the V2
// additions. Do not reorder without re-checking against the device library.
//
// descriptor: vendor.xiaomi.hardware.micharge.IMiCharge
// interface hash: f8886a701012d522ea9977256f52b3c47596963d
//
// Every method except setWirelessChargingEnabled takes and returns String,
// boolean or int. The HAL returns raw sysfs node contents as strings.

package vendor.xiaomi.hardware.micharge;

@VintfStability
interface IMiCharge {
    // ── V1 surface: transaction codes 1..50 ──

    String getBatteryAuthentic();        // 1
    String getBatteryCapacity();         // 2
    String getBatteryChargeFull();       // 3
    String getBatteryChargeType();       // 4
    String getBatteryCycleCount();       // 5
    String getBatteryIbat();             // 6
    String getBatteryResistance();       // 7
    String getBatterySoh();              // 8
    String getBatteryTbat();             // 9
    String getBatteryThermaLevel();      // 10
    String getBatteryVbat();             // 11
    String getBtTransferStartState();    // 12
    String getCarChargingType();         // 13
    String getChargingPowerMax();        // 14
    String getCoolModeState();           // 15
    String getFastChargeModeStatus();    // 16
    String getInputSuspendState();       // 17
    String getMiChargePath(in String key);          // 18
    String getNightChargingState();      // 19
    String getPSValue();                 // 20
    String getPdApdoMax();               // 21
    String getPdAuthentication();        // 22
    String getQuickChargeType();         // 23
    String getSBState();                 // 24
    String getSocDecimal();              // 25
    String getSocDecimalRate();          // 26
    String getTxAdapt();                 // 27
    String getUsbCurrent();              // 28
    String getUsbVoltage();              // 29
    String getWirelessChargingStatus();  // 30
    String getWirelessFwStatus();        // 31
    String getWirelessReverseStatus();   // 32
    boolean isBatteryLifeFunctionSupported();       // 33
    boolean isDPConnected();             // 34
    boolean isFunctionSupported(in String key);     // 35
    boolean isUSB32();                   // 36
    boolean isWirelessChargingSupported();          // 37
    boolean isWiressFwUpdateSupported(); // 38
    int setBtState(in String value);                // 39
    int setBtTransferStartState(in String value);   // 40
    int setCoolModeState(in String value);          // 41
    int setInputSuspendState(in String value);      // 42
    int setMiChargePath(in String key, in String value);  // 43
    int setNightChargingState(in String value);     // 44
    int setRxCr(in String value);                   // 45
    int setSBState(in String value);                // 46
    int setSmCountReset(in String value);           // 47
    int setUpdateWirelessFw(in String value);       // 48
    int setWirelessChargingEnabled(boolean enable); // 49
    int setWlsTxSpeed(in String value);             // 50

    // ── V2 additions: transaction codes 51..56 ──

    String getTypeCCommonInfo(in String key);       // 51
    int setTypeCCommonInfo(in String key, in String value);  // 52
    String getChargeCommonInfo(in String key);      // 53
    int setChargeCommonInfo(in String key, in String value); // 54
    String getBatteryCommonInfo(in String key);     // 55
    int setBatteryCommonInfo(in String key, in String value); // 56

    // Interface metadata. The official library exposes these at the fixed AIDL
    // codes 0x00FFFFFF and 0x00FFFFFE; build.rs rewrites the generated
    // constants to those values.
    int getInterfaceVersion();
    String getInterfaceHash();
}
