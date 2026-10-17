// SPDX-License-Identifier: GPL-2.0-or-later
//! CM message framing (`docs/spec/cm.md` §3.2, §8): parsing never panics,
//! and anything that parses survives an encode and re-parse unchanged.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ossteam_net::Packet;

fuzz_target!(|data: &[u8]| {
    if let Ok(p) = Packet::parse(data) {
        let again = Packet::parse(&p.encode()).expect("encoded packet re-parses");
        assert_eq!(again, p);
    }
});
