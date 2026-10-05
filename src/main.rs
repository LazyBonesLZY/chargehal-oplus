#![allow(non_snake_case)]

use std::sync::Arc;

use vendor_oplus_charger::adapter::{Adapter, ChargerInfo};
use vendor_oplus_charger::SERVICE_NAME;

include!(concat!(env!("OUT_DIR"), "/charger.rs"));
use crate::vendor::oplus::hardware::charger::testKitFeatureTestResult::testKitFeatureTestResult;
use crate::vendor::oplus::hardware::charger::ICharger::{BnCharger, ICharger};

type BinderResult<T> = rsbinder::status::Result<T>;

fn init_logging() {
    let _ = tracing_log::LogTracer::init();

    #[cfg(target_os = "android")]
    {
        use tracing_logcat::{LogcatMakeWriter, LogcatTag};
        use tracing_subscriber::fmt::format::Format;

        match LogcatMakeWriter::new(LogcatTag::Fixed("OplusChargerHAL".into())) {
            Ok(writer) => {
                let _ = tracing_subscriber::fmt()
                    .event_format(Format::default().without_time())
                    .with_env_filter(
                        tracing_subscriber::EnvFilter::try_from_default_env()
                            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                    )
                    .with_writer(writer)
                    .with_ansi(false)
                    .try_init();
            }
            Err(_) => {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(
                        tracing_subscriber::EnvFilter::try_from_default_env()
                            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                    )
                    .with_ansi(false)
                    .try_init();
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
}

macro_rules! ok0_methods {
    ($($name:ident $(($($arg:ident: $ty:ty),*))?;)+) => {$(
        #[allow(unused_variables)]
        fn $name(&self $(, $($arg: $ty),*)?) -> BinderResult<i32> { Ok(0) }
    )+};
}

macro_rules! empty_string_methods {
    ($($name:ident $(($($arg:ident: $ty:ty),*))?;)+) => {$(
        #[allow(unused_variables)]
        fn $name(&self $(, $($arg: $ty),*)?) -> BinderResult<String> { Ok(String::new()) }
    )+};
}

struct ChargerBinderService {
    adapter: Arc<Adapter>,
}

impl ChargerBinderService {
    fn info(&self) -> ChargerInfo {
        self.adapter.info.lock().clone()
    }

    fn int_string(value: i32) -> String {
        value.to_string()
    }

    fn get_chg_config_value(&self, flag: i32, extra: &str) -> String {
        match flag {
            0 => Self::int_string(self.adapter.get_fast_charge_snapshot().fast_type),
            1..=3 => {
                let snapshot = self.adapter.get_fast_charge_snapshot();
                Self::int_string(if snapshot.should_show_power() {
                    self.adapter.get_adapter_power_w()
                } else {
                    0
                })
            }
            4 => {
                let snapshot = self.adapter.get_fast_charge_snapshot();
                Self::int_string(
                    if snapshot.should_show_power()
                        && (snapshot.is_pps_active() || snapshot.cp_online != 0)
                    {
                        self.adapter.get_adapter_power_w()
                    } else {
                        0
                    },
                )
            }
            5 => self.adapter.get_charge_limit_value(),
            6 => {
                let snapshot = self.adapter.get_fast_charge_snapshot();
                Self::int_string((snapshot.is_fast_charge() || snapshot.is_svooc_active()) as i32)
            }
            14 => self.adapter.get_charge_limit_state(),
            20 => Self::int_string(self.adapter.get_battery_cycle_count()),
            21 => Self::int_string(self.adapter.is_silicon_battery_now() as i32),
            23 => self.adapter.get_battery_gauge_type(),
            24 | 25 => self.adapter.get_bypass_charge_status(),
            30 => {
                if extra == "Update" {
                    self.adapter.charger_info_json.lock().clone()
                } else {
                    "0".into()
                }
            }
            31 => "-1".into(),
            33 => Self::int_string(self.adapter.get_battery_rm()),
            37 => self.adapter.reverse_chg_info.lock().clone(),
            40 => String::new(),
            // The official implementation reports "-1" when the flag has no
            // handler in its get table; "0" would encode "unsupported" as a
            // legitimate numeric value.
            _ => "-1".into(),
        }
    }

    fn set_bool_like(value: &str) -> bool {
        matches!(value, "1" | "true" | "TRUE" | "True" | "enable" | "enabled")
    }
}

impl rsbinder::Interface for ChargerBinderService {}

impl ICharger for ChargerBinderService {
    ok0_methods! {
        VolDividerIcWorkModeSet(data: &str);
        chgExchangeMesgInit;
        chgExchangeSohMesgInit;
        getAcType;
        getBattSubCurrent;
        getBccExpStatus;
        getBmsHeatingStatus;
        getChargerCoolDown;
        getChargerCriticalLog;
        getChargerIdVolt;
        getChargerLog;
        getCustomSelectChgMode;
        getParallelChgMosTestResult;
        getPsyBatteryNotify;
        getPsyBatteryPchg;
        getPsyBatteryPchgResetCount;
        getPsyOtgOnline;
        getPsyOtgSwitch;
        getPsyQGVbatDeviation;
        getPsyUsbStatus;
        getQgVbatDeviation;
        getSmartChgMode;
        getUsbPrimalType;
        getWiredOtgOnline;
        getWirelessChargePumpEn;
        getWirelessPenPresent;
        getWirelessPtmcId;
        getWirelessRXEnable;
        getWirelessRealType;
        getWirelessUserSleepMode;
        nightstandby(status: i32);
        setChargeEMMode(data: &str);
        setChargerControl(data: &str);
        setChargerCriticalLog(data: &str);
        setChargerCycle(data: &str);
        setChargerFactoryModeTest(data: &str);
        setChargerLog(data: &str);
        setChgStatusToBcc(status: i32);
        setCustomSelectChgMode(mode: i32, enable: bool);
        setFastchgFwUpdate(data: &str);
        setPsyMmiChgEn(data: &str);
        setPsyOtgSwitch(data: &str);
        setReserveSocDebug(data: &str);
        setShipMode(data: &str);
        setSmartChgMode(data: &str);
        setTbattPwrOff(data: &str);
        setUisohDebugInfo(data: &str);
        setUsbPrimalType(data: &str);
        setWirelessChargePumpEn(data: &str);
        setWirelessFtmMode(data: &str);
        setWirelessIconDelay(data: &str);
        setWirelessIdtAdcTest(data: &str);
        setWirelessPenSoc(data: &str);
        setWirelessRXEnable(data: &str);
        setWirelessTXEnable(data: &str);
        setWirelessUserSleepMode(data: &str);
        setWlsThirdPartitionInfo(data: &str);
        testKitGetFeatureNum;
        updateUiSohToPartion;
        setChgOlcConfig(data: &str);
        setSuperEnduranceStatus(data: &str);
        setSuperEnduranceCount(data: &str);
        setBobStatus(data: &str);
        setPsySlowChgEn(data: &str);
        getCpVbatDeviation;
        getChargingModeInGsmCall;
        setChargingModeInGsmCall(data: &str);
        setChgRusConfig(data: &str);
    }

    empty_string_methods! {
        getBattParamNoplug;
        getBccCsvData;
        getBmsHeatingRunningStatus;
        getChargerControl;
        getDevinfoFastchg;
        getPsyWirelessRX;
        getPsyWirelessRxVersion;
        getPsyWirelessTX;
        getPsyWirelessTxVersion;
        getReserveSocDebug;
        getWirelessDeviated;
        queryWlsPencilInfo;
        getChgOlcConfig;
    }

    fn getBattAuthenticate(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_authenticate())
    }

    fn getPsyBatteryHmac(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_authenticate())
    }

    fn notifyScreenStatus(&self, status: i32) -> BinderResult<i32> {
        self.adapter.notify_screen_status(status);
        Ok(0)
    }

    fn getBattShortIcOtpStatus(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_short_ic_otp_status())
    }

    fn getBattPPSChgIng(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_fast_charge_snapshot().is_pps_active() as i32)
    }

    fn getBattPPSChgPower(&self) -> BinderResult<i32> {
        let snapshot = self.adapter.get_fast_charge_snapshot();
        Ok(
            if snapshot.should_show_power() && snapshot.is_pps_active() {
                self.adapter.get_adapter_power_w()
            } else {
                0
            },
        )
    }

    fn getBattVoocChgIng(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_fast_charge_snapshot().is_svooc_active() as i32)
    }

    fn getBatteryVoltageNow(&self) -> BinderResult<i32> {
        Ok(self.info().battery_voltage_now)
    }

    fn getFastCharge(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_fast_charge_snapshot().is_fast_charge() as i32)
    }

    fn getPsyAcOnline(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_ac_online())
    }

    fn getPsyBatteryCC(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_cc())
    }

    fn getPsyBatteryCurrentNow(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_current_now())
    }

    fn getPsyInputCurrent(&self) -> BinderResult<i32> {
        Ok(self.info().usb_current_now)
    }

    fn getWirelessCurrentNow(&self) -> BinderResult<i32> {
        Ok(self.info().wireless_current_now)
    }

    fn getWirelessVoltageNow(&self) -> BinderResult<i32> {
        Ok(self.info().wireless_voltage_now)
    }

    fn getPsyBatteryFcc(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_fcc())
    }

    fn getPsyBatteryLevel(&self) -> BinderResult<i32> {
        Ok(self.info().battery_capacity)
    }

    fn getPsyBatteryRm(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_rm())
    }

    fn getPsyBatteryShortFeature(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_short_feature())
    }

    fn getPsyBatteryShortStatus(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_short_status())
    }

    fn getPsyBatteryStatus(&self) -> BinderResult<String> {
        Ok(self.adapter.get_battery_status())
    }

    fn getPsyBatteryTemp(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_temp())
    }

    fn getPsyChargeTech(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_fast_charge_snapshot().charge_tech)
    }

    fn getPsyFastChgType(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_fast_charge_snapshot().fast_type)
    }

    fn getPsyPcPortOnline(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_pc_port_online())
    }

    fn getPsyTypeOrientation(&self) -> BinderResult<i32> {
        Ok(self.info().cc_orientation)
    }

    fn getPsyUsbOnline(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_usb_online())
    }

    fn getQuickModeGain(&self) -> BinderResult<String> {
        Ok(self.adapter.quick_mode_gain.lock().clone())
    }

    fn getUIsohValue(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_battery_soh())
    }

    fn getUisohDebugParameterInfo(&self) -> BinderResult<String> {
        Ok(self.adapter.soh_debug_info.lock().clone())
    }

    fn healthd_update_ui_soc_decimal(&self) -> BinderResult<String> {
        Ok(self.adapter.get_decimal_soc())
    }

    fn getUsbInputCurrentNow(&self) -> BinderResult<i32> {
        Ok(self.info().usb_current_now)
    }

    fn getWirelessAdapterPower(&self) -> BinderResult<i32> {
        let snapshot = self.adapter.get_fast_charge_snapshot();
        Ok(
            if snapshot.should_show_power() && snapshot.wireless_online != 0 {
                self.adapter.get_adapter_power_w()
            } else {
                0
            },
        )
    }

    fn getWirelessCapacity(&self) -> BinderResult<i32> {
        Ok(self.info().battery_capacity)
    }

    fn getWirelessOnline(&self) -> BinderResult<i32> {
        Ok(self.adapter.get_wireless_online())
    }

    fn getWirelessTXEnable(&self) -> BinderResult<String> {
        // The official implementation reads an OPPO-only proc node that this HAL
        // never touches. Returning a synthesised "disable" claimed a reading we
        // do not have.
        Ok(String::new())
    }

    fn queryChargeInfo(&self) -> BinderResult<String> {
        Ok(self.adapter.charger_info_json.lock().clone())
    }

    fn setChargerCoolDown(&self, data: &str) -> BinderResult<i32> {
        *self.adapter.cooldown.lock() = data.to_string();
        Ok(0)
    }

    fn setSmartCoolDown(
        &self,
        coolDown: i32,
        normalCoolDown: i32,
        pkgName: &str,
    ) -> BinderResult<i32> {
        *self.adapter.cooldown.lock() = format!("{},{},{}", coolDown, normalCoolDown, pkgName);
        Ok(0)
    }

    fn setBatteryLogPush(&self, data: &str) -> BinderResult<i32> {
        self.adapter.battery_log_enabled.store(
            Self::set_bool_like(data),
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(0)
    }

    fn getPsyBatterySN(&self) -> BinderResult<String> {
        Ok(self.info().batt_sn)
    }

    fn getBattGaugeInfo(&self) -> BinderResult<String> {
        // The OPlus HAL returns the raw gauge node contents, empty when the node
        // is absent. It never substitutes a synthesised string, so neither do we
        // (see chargehal-vendor-refs/FORMAT-CONTRACT.md §3).
        Ok(self.info().gauge_info)
    }

    fn setChgConfig(&self, flag: i32, extra: &str, _callerName: i32) -> BinderResult<i32> {
        // The official implementation answers -1 for a flag with no handler in
        // its set table, so a caller can tell "applied" from "no such flag".
        // Answering 0 for everything made a dropped write look like a success.
        match flag {
            5 => self.adapter.set_charge_limit_value(extra),
            14 => self.adapter.set_charge_limit_state(extra),
            19 => *self.adapter.anti_expansion_dis.lock() = extra.to_string(),
            25 => self.adapter.set_bypass_charge_status(extra),
            36 => *self.adapter.reverse_chg_info.lock() = extra.to_string(),
            _ => return Ok(-1),
        }
        Ok(0)
    }

    fn getChgConfig(&self, flag: i32, extra: &str, _callerName: i32) -> BinderResult<String> {
        Ok(self.get_chg_config_value(flag, extra))
    }

    fn setUsbEyeDiagram(
        &self,
        model: i32,
        eyeDiagram: &str,
        _isDefaultEyeDiagram: bool,
    ) -> BinderResult<i32> {
        // Mirrors the official validation: the payload is a `#`-separated pair
        // whose first field is a known platform tag, and only model 0 is
        // accepted. Anything else is rejected instead of being stored and later
        // handed back as if it were a reading.
        let mut fields = eyeDiagram.split('#');
        let tag = fields.next().unwrap_or_default();
        let known_tag = matches!((tag.len(), tag), (3, "mtk") | (4, "qcom") | (8, "unknown"));
        if fields.next().is_none() || !known_tag {
            return Ok(-1);
        }
        if model != 0 {
            return Err(rsbinder::status::ExceptionCode::UnsupportedOperation.into());
        }
        *self.adapter.usb_eye_diagram.lock() = eyeDiagram.to_string();
        Ok(0)
    }

    fn getUsbCurrentEyeDiagram(&self, model: i32) -> BinderResult<String> {
        // The official HAL service throws exception code -7 (UnsupportedOperation)
        // when model != 0 (see chargehal-vendor-refs/FORMAT-CONTRACT.md §3).
        if model != 0 {
            return Err(rsbinder::status::ExceptionCode::UnsupportedOperation.into());
        }
        Ok(self.adapter.usb_eye_diagram.lock().clone())
    }

    fn testKitFeatureTest(&self, _index: i32) -> BinderResult<testKitFeatureTestResult> {
        Ok(testKitFeatureTestResult {
            r#str: "unsupported".into(),
            r#ret: 0,
        })
    }

    fn testKitGetFeatureList(&self) -> BinderResult<String> {
        Ok(String::new())
    }

    fn testKitGetFeatureName(&self, _index: i32) -> BinderResult<String> {
        Ok(String::new())
    }

    fn getInterfaceVersion(&self) -> BinderResult<i32> {
        Ok(vendor_oplus_charger::INTERFACE_VERSION)
    }

    fn getInterfaceHash(&self) -> BinderResult<String> {
        Ok(vendor_oplus_charger::INTERFACE_HASH.to_string())
    }
}

fn main() -> anyhow::Result<()> {
    init_logging();

    tracing::info!("Charger HAL adapter starting, service: {}", SERVICE_NAME);

    rsbinder::ProcessState::init_default()
        .map_err(|error| anyhow::anyhow!("failed to initialize Binder process state: {error}"))?;
    rsbinder::ProcessState::start_thread_pool();

    let adapter = Adapter::new();
    let binder_service = ChargerBinderService { adapter };
    let binder = BnCharger::new_binder(binder_service);
    rsbinder::hub::add_service(SERVICE_NAME, binder.as_binder())?;

    tracing::info!("Registered. Joining thread pool...");
    rsbinder::ProcessState::join_thread_pool()?;
    Ok(())
}
