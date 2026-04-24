//! [`eg4_bridge::eg4::packet_decoder::PacketDecoder`] framing, checksum, and stream behaviour using
//! the same golden bytes as the parser tests.

use super::golden::{self, ALL_DECODE_OK};
use bytes::BytesMut;
use eg4_bridge::eg4::packet::Parser;
use eg4_bridge::eg4::packet_decoder::PacketDecoder;
use std::io::ErrorKind;
use tokio_util::codec::Decoder;

#[test]
fn decoder_emits_same_packet_as_parser_for_all_golden_frames() {
    for frame in ALL_DECODE_OK {
        let expected = Parser::parse(frame).unwrap();
        let mut dec = PacketDecoder::new();
        let mut buf = BytesMut::from_iter(frame.iter().copied());
        let got = dec.decode(&mut buf).expect("decode").expect("full frame");
        assert_eq!(got, expected);
        assert!(buf.is_empty(), "decoder should consume entire frame");
    }
}

#[test]
fn decoder_rejects_heartbeat_frame_that_parser_accepts() {
    let frame = golden::HEARTBEAT;
    assert!(Parser::parse(frame).is_ok());
    let mut dec = PacketDecoder::new();
    let mut buf = BytesMut::from_iter(frame.iter().copied());
    let err = dec.decode(&mut buf).expect_err("19-byte frame below decoder minimum");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(
        err.to_string().contains("too small"),
        "unexpected error: {err}"
    );
}

#[test]
fn decoder_waits_for_partial_frame_then_decodes() {
    let frame = golden::READ_PARAM_REPLY;
    let mut dec = PacketDecoder::new();
    let mut buf = BytesMut::from_iter(frame[..10].iter().copied());
    assert!(dec.decode(&mut buf).unwrap().is_none());
    assert_eq!(buf.len(), 10);
    buf.extend_from_slice(&frame[10..]);
    let got = dec.decode(&mut buf).unwrap().unwrap();
    assert_eq!(got, Parser::parse(frame).unwrap());
    assert!(buf.is_empty());
}

#[test]
fn decoder_two_frames_back_to_back() {
    let a = golden::READ_PARAM_REPLY;
    let b = golden::WRITE_PARAM_REPLY;
    let mut dec = PacketDecoder::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(a);
    buf.extend_from_slice(b);

    let p1 = dec.decode(&mut buf).unwrap().unwrap();
    let p2 = dec.decode(&mut buf).unwrap().unwrap();
    assert_eq!(p1, Parser::parse(a).unwrap());
    assert_eq!(p2, Parser::parse(b).unwrap());
    assert!(buf.is_empty());
}

#[test]
fn decoder_rejects_bad_magic() {
    let mut frame = golden::HEARTBEAT.to_vec();
    frame[0] = 0;
    let mut dec = PacketDecoder::new();
    let mut buf = BytesMut::from_iter(frame);
    let err = dec.decode(&mut buf).expect_err("invalid header");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn decoder_rejects_modbus_crc_mismatch_on_translated_data() {
    let mut frame = golden::READ_HOLD_REPLY.to_vec();
    let n = frame.len();
    frame[n - 1] ^= 0xff;
    let mut dec = PacketDecoder::new();
    let mut buf = BytesMut::from_iter(frame);
    let err = dec.decode(&mut buf).expect_err("checksum");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(
        err.to_string().contains("Checksum mismatch"),
        "unexpected error: {err}"
    );
}
