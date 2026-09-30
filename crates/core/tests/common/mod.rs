//! Reports read from a real X11 over the 2.4 GHz dongle (factory settings, profile 1).
#![allow(dead_code)]

pub const DPI: &str = "04380100003f2020 1225384b75810101 0000000000010000 02 \
                       ff0000 00ff00 0000ff ffff00 00ffff ff00ff ff4000 ffffff 02 0f69 00000000";
pub const LIGHTING: &str = "050f0100 03a8 00ff00 01 04 01af 0000";
pub const POLLING: &str = "06090101fe00000000";
pub const PROFILES: &str = "0c0a01fe05fa00000000";
pub const BUTTONS: &str = "083b01 020000 030000 040000 000000 000000 0d0000 060000 050000 \
                           000000 000000 000000 000000 000000 000000 000000 000000 \
                           090000 0a0000 0034";

/// Decode hex, ignoring whitespace.
pub fn bytes(hex: &str) -> Vec<u8> {
    let digits: Vec<u8> = hex.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits.chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
}
