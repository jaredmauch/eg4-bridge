//! Golden-byte regression tests for [`eg4_bridge::eg4::packet::Parser`] and
//! [`eg4_bridge::eg4::packet_decoder::PacketDecoder`].
//!
//! On-wire vectors live in [`golden`]. Historical note: `TcpFrameFactory::create_frame` produces a
//! shorter envelope than [`Parser::parse`] consumes, so we only lock in **decode** behaviour here.

mod decoder;
mod golden;

use eg4_bridge::eg4::inverter::Serial;
use eg4_bridge::eg4::packet::{
    DeviceFunction, Heartbeat, Packet, Parser, ReadParam, TranslatedData, WriteParam,
};
use golden::*;
use std::str::FromStr;

/// Ensures every entry in [`golden::ALL`] stays parseable when golden vectors are edited.
#[test]
fn parser_accepts_every_golden_vector() {
    for frame in golden::ALL {
        Parser::parse(frame).unwrap();
    }
}

fn datalog() -> Serial {
    Serial::from_str("2222222222").unwrap()
}

fn serial() -> Serial {
    Serial::from_str("5555555555").unwrap()
}

#[test]
fn parse_heartbeat() {
    assert_eq!(
        Parser::parse(HEARTBEAT).unwrap(),
        Packet::Heartbeat(Heartbeat { datalog: datalog() })
    );
}

#[test]
fn parse_read_hold_reply() {
    assert_eq!(
        Parser::parse(READ_HOLD_REPLY).unwrap(),
        Packet::TranslatedData(TranslatedData {
            datalog: datalog(),
            device_function: DeviceFunction::ReadHold,
            inverter: serial(),
            register: 12,
            values: vec![22, 6, 20, 5, 16, 57],
        })
    );
}

#[test]
fn parse_read_inputs_reply() {
    assert_eq!(
        Parser::parse(READ_INPUTS_REPLY).unwrap(),
        Packet::TranslatedData(TranslatedData {
            datalog: datalog(),
            device_function: DeviceFunction::ReadInput,
            inverter: serial(),
            register: 0,
            values: vec![
                32, 0, 0, 0, 0, 0, 0, 0, 250, 1, 77, 0, 0, 53, 0, 0, 0, 0, 0, 0, 128, 13, 0, 0, 114,
                9, 0, 16, 132, 0, 142, 19, 0, 0, 198, 13, 202, 5, 232, 3, 114, 9, 0, 10, 80, 112,
                142, 19, 0, 0, 0, 0, 0, 0, 36, 15, 0, 0, 0, 0, 0, 0, 91, 0, 83, 0, 87, 0, 114, 0, 0,
                0, 1, 0, 102, 0, 174, 14, 183, 12
            ]
        })
    );
}

#[test]
fn parse_read_inputs_all_protocol5_reply() {
    assert_eq!(
        Parser::parse(READ_INPUTS_ALL_PROTOCOL5_REPLY).unwrap(),
        Packet::TranslatedData(TranslatedData {
            datalog: datalog(),
            device_function: DeviceFunction::ReadInput,
            inverter: serial(),
            register: 0,
            values: vec![
                16, 0, 0, 0, 0, 0, 121, 15, 247, 1, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 74,
                9, 0, 0, 0, 0, 141, 19, 0, 0, 0, 0, 120, 0, 0, 0, 73, 9, 212, 1, 193, 124, 140, 19,
                0, 0, 0, 0, 0, 0, 8, 2, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 8, 0, 1, 0, 0, 0, 0, 0, 55, 0,
                118, 15, 232, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 232, 1, 0, 0, 177,
                1, 0, 0, 61, 0, 0, 0, 41, 0, 0, 0, 0, 0, 0, 0, 217, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                24, 0, 25, 0, 39, 0, 24, 0, 0, 0, 75, 52, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 34, 0, 0, 0, 136, 19, 23, 2, 0, 0, 0, 0, 1, 8, 0, 16, 16, 220, 255,
                42, 48, 18, 255, 171, 14, 131, 240, 96, 157, 16, 2, 0, 1, 0, 50, 0, 76, 255, 0, 0,
                245, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 16, 246, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4,
                1, 0, 0, 50, 48, 52, 51, 48, 50, 50, 52, 48, 49, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0
            ]
        })
    );
}

#[test]
fn parse_write_single_reply() {
    assert_eq!(
        Parser::parse(WRITE_SINGLE_REPLY).unwrap(),
        Packet::TranslatedData(TranslatedData {
            datalog: datalog(),
            device_function: DeviceFunction::WriteSingle,
            inverter: serial(),
            register: 66,
            values: vec![100, 0]
        })
    );
}

#[test]
fn parse_write_multi_reply() {
    assert_eq!(
        Parser::parse(WRITE_MULTI_REPLY).unwrap(),
        Packet::TranslatedData(TranslatedData {
            datalog: datalog(),
            device_function: DeviceFunction::WriteMulti,
            inverter: serial(),
            register: 12,
            values: vec![3, 0]
        })
    );
}

#[test]
fn parse_read_param_reply() {
    assert_eq!(
        Parser::parse(READ_PARAM_REPLY).unwrap(),
        Packet::ReadParam(ReadParam {
            datalog: datalog(),
            register: 0,
            values: vec![44, 1]
        })
    );
}

#[test]
fn parse_write_param_reply() {
    assert_eq!(
        Parser::parse(WRITE_PARAM_REPLY).unwrap(),
        Packet::WriteParam(WriteParam {
            datalog: datalog(),
            register: 7,
            values: vec![0, 3]
        })
    );
}
