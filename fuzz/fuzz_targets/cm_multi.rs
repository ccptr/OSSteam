// SPDX-License-Identifier: GPL-2.0-or-later
//! `Multi` unpacking (`docs/spec/cm.md` §3.4, §8): decoding and inflating
//! never panic, and never allocate past the inflate limit.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ossteam_net::multi;
use ossteam_proto::Message;
use ossteam_proto::steam::CMsgMulti;

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = CMsgMulti::decode(data) {
        if let Ok(parts) = multi::unpack(&m) {
            let total: usize = parts.iter().map(Vec::len).sum();
            assert!(total <= multi::MAX_INFLATED);
        }
    }
});
