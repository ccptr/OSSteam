// SPDX-License-Identifier: GPL-2.0-or-later
//! Binary KeyValues: parsing never panics, and anything that parses survives
//! a write and re-parse unchanged.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ossteam_vdf::binary::{self, Keys};

fuzz_target!(|data: &[u8]| {
    if let Ok((obj, _)) = binary::parse(data, Keys::Inline) {
        let again = binary::parse_all(&binary::to_vec(&obj), Keys::Inline).expect("written binary re-parses");
        // NaN floats compare unequal to themselves; compare the bytes instead.
        assert_eq!(binary::to_vec(&again), binary::to_vec(&obj));
    }
});
