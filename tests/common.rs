#![allow(dead_code)]

use eg4_bridge::prelude::*;
use eg4_bridge::{config, eg4, influx, database};
use std::str::FromStr;
use tokio::sync::broadcast::error::TryRecvError;
use mockito;
use serde_json::json;
use eg4_bridge::eg4::inverter::Serial;
use eg4_bridge::unixtime::UnixTime;
use eg4_bridge::prelude::Config;
use eg4_bridge::prelude::ConfigWrapper;

pub struct Factory();
impl Factory {
    pub fn config() -> Config {
        Config::new("config.yaml.example".to_owned()).unwrap()
    }

    pub fn config_wrapper() -> ConfigWrapper {
        ConfigWrapper::new("config.yaml.example".to_owned()).unwrap()
    }

    pub fn translated_data() -> eg4::packet::TranslatedData {
        eg4::packet::TranslatedData {
            datalog: Serial::from_str("2222222222").unwrap(),
            serial: Serial::from_str("5555555555").unwrap(),
            device_function: eg4::packet::DeviceFunction::ReadInput,
            register: 0,
            values: vec![0, 0],
        }
    }

    pub fn packet() -> eg4::packet::Packet {
        eg4::packet::Packet::TranslatedData(Factory::translated_data())
    }

    pub fn channel_data() -> eg4::inverter::ChannelData {
        eg4::inverter::ChannelData::Packet(Factory::packet())
    }

    pub fn influx_data() -> influx::ChannelData {
        influx::ChannelData::InputData(serde_json::json!({
            "time": UnixTime::now(),
            "datalog": Serial::from_str("1234567890").unwrap(),
            "register": 0,
            "value": 0,
        }))
    }

    pub fn database_data() -> database::ChannelData {
        database::ChannelData::InputData(serde_json::json!({
            "time": UnixTime::now(),
            "datalog": Serial::from_str("1234567890").unwrap(),
            "register": 0,
            "value": 0,
        }))
    }

    pub fn mqtt_data() -> mqtt::ChannelData {
        mqtt::ChannelData::Message(mqtt::Message {
            topic: "test/topic".to_string(),
            retain: false,
            payload: "test payload".to_string(),
        })
    }

    pub fn translated_data_with_values(values: Vec<u8>) -> eg4::packet::TranslatedData {
        eg4::packet::TranslatedData {
            datalog: Serial::from_str("2222222222").unwrap(),
            serial: Serial::from_str("5555555555").unwrap(),
            device_function: eg4::packet::DeviceFunction::ReadInput,
            register: 0,
            values,
        }
    }

    pub fn packet_with_values(values: Vec<u8>) -> eg4::packet::Packet {
        eg4::packet::Packet::TranslatedData(Factory::translated_data_with_values(values))
    }

    pub fn channel_data_with_values(values: Vec<u8>) -> eg4::inverter::ChannelData {
        eg4::inverter::ChannelData::Packet(Factory::packet_with_values(values))
    }

    pub fn influx_data_with_values(values: Vec<u8>) -> influx::ChannelData {
        influx::ChannelData::InputData(serde_json::json!({
            "time": UnixTime::now(),
            "datalog": Serial::from_str("1234567890").unwrap(),
            "register": 0,
            "value": values,
        }))
    }

    pub fn database_data_with_values(values: Vec<u8>) -> database::ChannelData {
        database::ChannelData::InputData(serde_json::json!({
            "time": UnixTime::now(),
            "datalog": Serial::from_str("1234567890").unwrap(),
            "register": 0,
            "value": values,
        }))
    }

    pub fn mqtt_data_with_values(values: Vec<u8>) -> mqtt::ChannelData {
        mqtt::ChannelData::Message(mqtt::Message {
            topic: "test/topic".to_string(),
            retain: false,
            payload: serde_json::to_string(&values).unwrap(),
        })
    }

    pub fn translated_data_with_register(register: u16) -> eg4::packet::TranslatedData {
        eg4::packet::TranslatedData {
            datalog: Serial::from_str("2222222222").unwrap(),
            serial: Serial::from_str("5555555555").unwrap(),
            device_function: eg4::packet::DeviceFunction::ReadInput,
            register,
            values: vec![0, 0],
        }
    }

    pub fn packet_with_register(register: u16) -> eg4::packet::Packet {
        eg4::packet::Packet::TranslatedData(Factory::translated_data_with_register(register))
    }

    pub fn channel_data_with_register(register: u16) -> eg4::inverter::ChannelData {
        eg4::inverter::ChannelData::Packet(Factory::packet_with_register(register))
    }

    pub fn influx_data_with_register(register: u16) -> influx::ChannelData {
        influx::ChannelData::InputData(serde_json::json!({
            "time": UnixTime::now(),
            "datalog": Serial::from_str("1234567890").unwrap(),
            "register": register,
            "value": 0,
        }))
    }

    pub fn database_data_with_register(register: u16) -> database::ChannelData {
        database::ChannelData::InputData(serde_json::json!({
            "time": UnixTime::now(),
            "datalog": Serial::from_str("1234567890").unwrap(),
            "register": register,
            "value": 0,
        }))
    }

    pub fn mqtt_data_with_register(register: u16) -> mqtt::ChannelData {
        mqtt::ChannelData::Message(mqtt::Message {
            topic: format!("test/topic/{}", register),
            retain: false,
            payload: "0".to_string(),
        })
    }

    pub fn translated_data_with_datalog(datalog: Serial) -> eg4::packet::TranslatedData {
        eg4::packet::TranslatedData {
            datalog,
            serial: Serial::from_str("5555555555").unwrap(),
            device_function: eg4::packet::DeviceFunction::ReadInput,
            register: 0,
            values: vec![0, 0],
        }
    }

    pub fn packet_with_datalog(datalog: Serial) -> eg4::packet::Packet {
        eg4::packet::Packet::TranslatedData(Factory::translated_data_with_datalog(datalog))
    }

    pub fn channel_data_with_datalog(datalog: Serial) -> eg4::inverter::ChannelData {
        eg4::inverter::ChannelData::Packet(Factory::packet_with_datalog(datalog))
    }

    pub fn influx_data_with_datalog(datalog: Serial) -> influx::ChannelData {
        influx::ChannelData::InputData(serde_json::json!({
            "time": UnixTime::now(),
            "datalog": datalog,
            "register": 0,
            "value": 0,
        }))
    }

    pub fn database_data_with_datalog(datalog: Serial) -> database::ChannelData {
        database::ChannelData::InputData(serde_json::json!({
            "time": UnixTime::now(),
            "datalog": datalog,
            "register": 0,
            "value": 0,
        }))
    }

    pub fn mqtt_data_with_datalog(datalog: Serial) -> mqtt::ChannelData {
        mqtt::ChannelData::Message(mqtt::Message {
            topic: format!("test/topic/{}", datalog),
            retain: false,
            payload: "0".to_string(),
        })
    }

    pub fn inverter() -> config::Inverter {
        config::Inverter {
            enabled: true,
            port: 8000,
            host: "localhost".to_owned(),
            datalog: Serial::from_str("2222222222").unwrap(),
            serial: Serial::from_str("5555555555").unwrap(),
            heartbeats: None,
            publish_holdings_on_connect: None,
            read_timeout: None,
        }
    }

    pub fn read_input_1() -> lxp::packet::ReadInput1 {
        lxp::packet::ReadInput1 {
            status: 16,
            v_pv_1: Some(0.0),
            v_pv_2: Some(0.0),
            v_pv_3: Some(0.0),
            v_bat: Some(49.1),
            soc: 55,
            soh: 0,
            internal_fault: 5,
            p_pv: 0,
            p_pv_1: 0,
            p_pv_2: 0,
            p_pv_3: 0,
            p_battery: -813,
            p_charge: 0,
            p_discharge: 813,
            v_ac_r: 246.3,
            v_ac_s: 409.6,
            v_ac_t: 0.0,
            f_ac: 50.02,
            p_inv: 732,
            p_rec: 0,
            pf: 1.0,
            v_eps_r: 246.3,
            v_eps_s: 256.0,
            v_eps_t: 2875.2,
            f_eps: 50.02,
            p_eps: 0,
            s_eps: 0,
            p_grid: -10,
            p_to_grid: 10,
            p_to_user: 0,
            e_pv_day: 0.0,
            e_pv_day_1: 0.0,
            e_pv_day_2: 0.0,
            e_pv_day_3: 0.0,
            e_inv_day: 5.9,
            e_rec_day: 2.2,
            e_chg_day: 2.2,
            e_dischg_day: 7.5,
            e_eps_day: 0.0,
            e_to_grid_day: 0.2,
            e_to_user_day: 3.2,
            v_bus_1: 373.2,
            v_bus_2: 293.5,
            time: UnixTime::now(),
            datalog: Serial::from_str("1234567890").unwrap(),
        }
    }

    pub fn read_input_2() -> lxp::packet::ReadInput2 {
        lxp::packet::ReadInput2 {
            e_pv_all: 4215.8,
            e_pv_all_1: 4215.8,
            e_pv_all_2: 0.0,
            e_pv_all_3: 0.0,
            e_inv_all: 3249.1,
            e_rec_all: 3919.5,
            e_chg_all: 4392.6,
            e_dischg_all: 4092.7,
            e_eps_all: 0.0,
            e_to_grid_all: 979.6,
            e_to_user_all: 5889.8,
            fault_code: 5,
            warning_code: 3,
            t_inner: 49,
            t_rad_1: 36,
            t_rad_2: 37,
            t_bat: 0,
            runtime: 67589346,
            time: UnixTime::now(),
            datalog: Serial::from_str("1234567890").unwrap(),
        }
    }

    pub fn read_input_3() -> lxp::packet::ReadInput3 {
        lxp::packet::ReadInput3 {
            max_chg_curr: 150.0,
            max_dischg_curr: 150.0,
            charge_volt_ref: 53.2,
            dischg_cut_volt: 40.0,
            bat_status_0: 0,
            bat_status_1: 0,
            bat_status_2: 0,
            bat_status_3: 0,
            bat_status_4: 0,
            bat_status_5: 192,
            bat_status_6: 0,
            bat_status_7: 0,
            bat_status_8: 0,
            bat_status_9: 0,
            bat_status_inv: 3,
            bat_count: 6,
            bat_capacity: 0,
            bat_current: 0.0,
            bms_event_1: 1,
            bms_event_2: 2,
            max_cell_voltage: 0.0,
            min_cell_voltage: 0.0,
            max_cell_temp: 0.0,
            min_cell_temp: 0.0,
            bms_fw_update_state: 2,
            cycle_count: 200,
            vbat_inv: 5.4,
            time: UnixTime::now(),
            datalog: Serial::from_str("1234567890").unwrap(),
        }
    }

    pub fn read_input_all() -> lxp::packet::ReadInputAll {
        lxp::packet::ReadInputAll {
            status: 16,
            v_pv_1: Some(0.0),
            v_pv_2: Some(0.0),
            v_pv_3: Some(0.0),
            v_bat: Some(49.1),
            soc: 55,
            soh: 0,
            internal_fault: 0,
            p_pv: 0,
            p_pv_1: 0,
            p_pv_2: 0,
            p_pv_3: 0,
            p_battery: 0,
            p_charge: 0,
            p_discharge: 0,
            v_ac_r: 0.0,
            v_ac_s: 0.0,
            v_ac_t: 0.0,
            f_ac: 0.0,
            p_inv: 0,
            p_rec: 0,
            pf: 0.0,
            v_eps_r: 0.0,
            v_eps_s: 0.0,
            v_eps_t: 0.0,
            f_eps: 0.0,
            p_eps: 0,
            s_eps: 0,
            p_grid: 0,
            p_to_grid: 0,
            p_to_user: 0,
            e_pv_day: 0.0,
            e_pv_day_1: 0.0,
            e_pv_day_2: 0.0,
            e_pv_day_3: 0.0,
            e_inv_day: 0.0,
            e_rec_day: 0.0,
            e_chg_day: 0.0,
            e_dischg_day: 0.0,
            e_eps_day: 0.0,
            e_to_grid_day: 0.0,
            e_to_user_day: 0.0,
            v_bus_1: 0.0,
            v_bus_2: 0.0,
            e_pv_all: 0.0,
            e_pv_all_1: 0.0,
            e_pv_all_2: 0.0,
            e_pv_all_3: 0.0,
            e_inv_all: 0.0,
            e_rec_all: 0.0,
            e_chg_all: 0.0,
            e_dischg_all: 0.0,
            e_eps_all: 0.0,
            e_to_grid_all: 0.0,
            e_to_user_all: 0.0,
            fault_code: 0,
            warning_code: 0,
            t_inner: 0,
            t_rad_1: 0,
            t_rad_2: 0,
            t_bat: 0,
            runtime: 0,
            max_chg_curr: 0.0,
            max_dischg_curr: 0.0,
            charge_volt_ref: 0.0,
            dischg_cut_volt: 0.0,
            bat_status_0: 0,
            bat_status_1: 0,
            bat_status_2: 0,
            bat_status_3: 0,
            bat_status_4: 0,
            bat_status_5: 0,
            bat_status_6: 0,
            bat_status_7: 0,
            bat_status_8: 0,
            bat_status_9: 0,
            bat_status_inv: 0,
            bat_count: 0,
            bat_capacity: 0,
            bat_current: 0.0,
            bms_event_1: 0,
            bms_event_2: 0,
            max_cell_voltage: 0.0,
            min_cell_voltage: 0.0,
            max_cell_temp: 0.0,
            min_cell_temp: 0.0,
            bms_fw_update_state: 0,
            cycle_count: 0,
            vbat_inv: 0.0,
            v_gen: 0.0,
            f_gen: 0.0,
            p_gen: 0,
            e_gen_day: 0.0,
            e_gen_all: 0.0,
            v_eps_l1: 0.0,
            v_eps_l2: 0.0,
            p_eps_l1: 0,
            p_eps_l2: 0,
            s_eps_l1: 0,
            s_eps_l2: 0,
            e_eps_l1_day: 0.0,
            e_eps_l2_day: 0.0,
            e_eps_l1_all: 0.0,
            e_eps_l2_all: 0.0,
            datalog: Serial::from_str("2222222222").unwrap(),
            time: UnixTime::now(),
        }
    }
}

pub fn common_setup() {
    let _ = env_logger::try_init();
}

pub fn unwrap_inverter_channeldata_packet(i: lxp::inverter::ChannelData) -> lxp::packet::Packet {
    if let lxp::inverter::ChannelData::Packet(i) = i {
        return i;
    }
    panic!()
}

pub fn unwrap_influx_channeldata_input_data(i: influx::ChannelData) -> serde_json::Value {
    if let influx::ChannelData::InputData(i) = i {
        return i;
    }
    panic!()
}

pub fn unwrap_database_channeldata_read_input_all(
    i: database::ChannelData,
) -> lxp::packet::ReadInputAll {
    if let database::ChannelData::ReadInputAll(i) = i {
        return *i;
    }
    panic!()
}

pub fn type_of<T>(_: &T) -> &str {
    std::any::type_name::<T>()
}
