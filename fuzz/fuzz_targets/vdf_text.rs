// SPDX-License-Identifier: GPL-2.0-or-later
//! Text KeyValues: parsing never panics, and anything that parses survives
//! a write and re-parse unchanged.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ossteam_vdf::text;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    if let Ok(doc) = text::parse_document(s) {
        let again = text::parse_document(&text::document_to_string(&doc)).expect("written text re-parses");
        assert_eq!(again, doc);
    }
});
