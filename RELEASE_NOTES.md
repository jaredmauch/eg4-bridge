## 2026-04-24

- Added integration tests for `eg4::packet_decoder::PacketDecoder` using captured on-wire frames from the repository.
- Refactored packet TCP frame tests into `tests/test_packet_tcp_frames/` with shared golden bytes for parser and decoder coverage.
- Documented the known heartbeat framing mismatch where `Parser` accepts a 19-byte frame but `PacketDecoder` enforces a 20-byte minimum.
