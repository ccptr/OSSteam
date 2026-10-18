// SPDX-License-Identifier: GPL-2.0-or-later
//! Generates every vtable slot of `docs/spec/steamworks/vtables.txt` with
//! its exact signature for the target (`steamworks-abi.md` §2.2, §2.4).
//!
//! A slot whose interface, method and signature appear in
//! `src/implemented.txt` calls that Rust function. Every other slot is a
//! stub that records the call and returns the failure value of its
//! return type (ABI-3).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const TABLE: &str = "../../docs/spec/steamworks/vtables.txt";
const IMPLEMENTED: &str = "src/implemented.txt";

/// Struct types that can be returned by value, with the Rust type that
/// mirrors their layout (`src/abi.rs`).
const RETURNED_STRUCTS: &[(&str, &str)] = &[
    ("CSteamID", "crate::abi::CSteamID"),
    ("CGameID", "crate::abi::CSteamID"),
    ("SteamIPAddress_t", "crate::abi::SteamIpAddress"),
    ("InputAnalogActionData_t", "crate::abi::AnalogActionData"),
    (
        "ControllerAnalogActionData_t",
        "crate::abi::AnalogActionData",
    ),
    ("InputDigitalActionData_t", "crate::abi::DigitalActionData"),
    (
        "ControllerDigitalActionData_t",
        "crate::abi::DigitalActionData",
    ),
    ("InputMotionData_t", "crate::abi::MotionData"),
    ("ControllerMotionData_t", "crate::abi::MotionData"),
];

/// Struct types passed by value, with their size on 32-bit Windows, where
/// the callee pops its arguments and must know their size.
const PASSED_STRUCTS: &[(&str, u32)] = &[
    ("CSteamID", 8),
    ("CGameID", 8),
    ("SteamIPAddress_t", 20),
    ("SteamPartyBeaconLocation_t", 12),
    ("RemoteStorageUpdatePublishedFileRequest_t", 40),
];

struct Slot {
    method: String,
    params: Vec<String>,
    ret: String,
}

struct Version {
    name: String,
    interface: String,
    slots: Vec<Slot>,
}

struct Implemented {
    interface: String,
    method: String,
    params: Vec<String>,
    ret: String,
    path: String,
    line: usize,
    used: bool,
}

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let table = dir.join(TABLE);
    let implemented = dir.join(IMPLEMENTED);
    println!("cargo::rerun-if-changed={}", table.display());
    println!("cargo::rerun-if-changed={}", implemented.display());

    let mut versions = parse_table(&table);
    let mut impls = parse_implemented(&implemented);
    let os = std::env::var("CARGO_CFG_TARGET_OS").expect("set by cargo");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("set by cargo");
    let target = Target {
        windows: os == "windows",
        thiscall: os == "windows" && arch == "x86",
    };
    if !target.windows {
        for v in &mut versions {
            declaration_order(&mut v.slots);
        }
    }
    let code = generate(&versions, &mut impls, &target);
    for i in &impls {
        if !i.used {
            panic!(
                "{IMPLEMENTED}:{}: {}.{} with that signature is in no vtable",
                i.line, i.interface, i.method
            );
        }
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    std::fs::write(out.join("vtables.rs"), code).expect("write vtables.rs");
}

fn parse_table(path: &Path) -> Vec<Version> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let mut versions: Vec<Version> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(head) = line.strip_prefix('[') {
            let head = head.trim_end_matches(']');
            let (name, interface) = head
                .split_once(' ')
                .unwrap_or_else(|| panic!("{TABLE}:{}: malformed header", n + 1));
            versions.push(Version {
                name: name.to_owned(),
                interface: interface.to_owned(),
                slots: Vec::new(),
            });
            continue;
        }
        let v = versions
            .last_mut()
            .unwrap_or_else(|| panic!("{TABLE}:{}: slot before any header", n + 1));
        let (index, rest) = line
            .split_once(' ')
            .unwrap_or_else(|| panic!("{TABLE}:{}: malformed slot", n + 1));
        assert_eq!(
            index.parse::<usize>().ok(),
            Some(v.slots.len()),
            "{TABLE}:{}: slots out of order",
            n + 1
        );
        let open = rest.find('(').expect("a parameter list");
        let close = rest.rfind(") -> ").expect("a return type");
        let params = rest[open + 1..close]
            .split(", ")
            .filter(|p| !p.is_empty())
            .map(|p| {
                p.rsplit_once(": ")
                    .unwrap_or_else(|| panic!("{TABLE}:{}: malformed parameter {p}", n + 1))
                    .1
                    .to_owned()
            })
            .collect();
        v.slots.push(Slot {
            method: rest[..open].to_owned(),
            params,
            ret: rest[close + 5..].to_owned(),
        });
    }
    versions
}

/// The table lists overloads as MSVC lays them out: next to each other,
/// the last declared first. Proton names the n-th declared overload
/// `<Method>_<n>` from the second on. The Itanium C++ ABI (Linux, macOS)
/// keeps declaration order, so each run of overloads is reversed there
/// (`steamworks-abi.md` §2.4).
fn declaration_order(slots: &mut [Slot]) {
    fn base(method: &str) -> &str {
        match method.rsplit_once('_') {
            Some((b, n)) if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => b,
            _ => method,
        }
    }
    let mut start = 0;
    while start < slots.len() {
        let name = base(&slots[start].method).to_owned();
        let mut end = start + 1;
        while end < slots.len() && base(&slots[end].method) == name {
            end += 1;
        }
        slots[start..end].reverse();
        start = end;
    }
}

fn parse_implemented(path: &Path) -> Vec<Implemented> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || {
            panic!(
                "{IMPLEMENTED}:{}: expected `I.M(types) -> type = path`",
                n + 1
            )
        };
        let Some((sig, path)) = line.split_once(" = ") else {
            bad()
        };
        let Some((head, ret)) = sig.split_once(" -> ") else {
            bad()
        };
        let Some((name, params)) = head.split_once('(') else {
            bad()
        };
        let Some((interface, method)) = name.split_once('.') else {
            bad()
        };
        let params = params
            .trim_end_matches(')')
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        out.push(Implemented {
            interface: interface.to_owned(),
            method: method.to_owned(),
            params,
            ret: ret.trim().to_owned(),
            path: path.trim().to_owned(),
            line: n + 1,
            used: false,
        });
    }
    out
}

struct Target {
    windows: bool,
    thiscall: bool,
}

impl Target {
    fn abi(&self) -> &'static str {
        if self.thiscall { "thiscall" } else { "C" }
    }
}

/// Whether an implementation's type (`implemented.txt`) fits a table type.
fn fits(table: &str, ours: &str) -> bool {
    match ours {
        "bool" => table == "i8",
        "steamid" => table == "CSteamID",
        _ => table == ours,
    }
}

fn param_type(t: &str) -> &'static str {
    match t {
        "i8" => "i8",
        "u8" => "u8",
        "i16" => "i16",
        "u16" => "u16",
        "i32" => "i32",
        "u32" => "u32",
        "i64" => "i64",
        "u64" | "CSteamID" | "CGameID" => "u64",
        "f32" => "f32",
        "f64" => "f64",
        "ptr" => "*mut core::ffi::c_void",
        other => panic!("{other} cannot be an implemented parameter"),
    }
}

fn returned_struct(t: &str) -> Option<&'static str> {
    RETURNED_STRUCTS
        .iter()
        .find(|(n, _)| *n == t)
        .map(|(_, r)| *r)
}

fn return_type(t: &str) -> String {
    match t {
        "void" => "()".into(),
        "str" => "*const core::ffi::c_char".into(),
        "ptr" => "*mut core::ffi::c_void".into(),
        "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "i64" | "u64" | "f32" | "f64" => t.into(),
        other => returned_struct(other)
            .unwrap_or_else(|| panic!("no Rust type for returned {other}"))
            .into(),
    }
}

fn failure(t: &str) -> &'static str {
    match t {
        "void" => "()",
        "str" => "crate::abi::EMPTY.as_ptr()",
        "ptr" => "core::ptr::null_mut()",
        "f32" | "f64" => "0.0",
        "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "i64" | "u64" => "0",
        // SAFETY (in the generated code): the returned structs are plain
        // data for which all-zero is a valid value.
        _ => "unsafe { core::mem::zeroed() }",
    }
}

/// Stack bytes of the arguments after `this` on 32-bit Windows.
fn stack_words(slot: &Slot) -> u32 {
    let mut n = 0;
    if returned_struct(&slot.ret).is_some() {
        n += 4;
    }
    for p in &slot.params {
        n += match p.as_str() {
            "i64" | "u64" | "f64" => 8,
            "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "f32" | "ptr" => 4,
            s => {
                let size = PASSED_STRUCTS
                    .iter()
                    .find(|(n, _)| *n == s)
                    .unwrap_or_else(|| panic!("no size for {s}"))
                    .1;
                size.div_ceil(4) * 4
            }
        };
    }
    // The hidden return pointer is declared on its own.
    if returned_struct(&slot.ret).is_some() {
        n -= 4;
    }
    n / 4
}

fn generate(versions: &[Version], impls: &mut [Implemented], t: &Target) -> String {
    let mut out = String::new();
    out.push_str("// Generated by build.rs from docs/spec/steamworks/vtables.txt.\n\n");
    let mut interfaces: BTreeMap<&str, usize> = BTreeMap::new();
    for v in versions {
        let n = interfaces.len();
        interfaces.entry(&v.interface).or_insert(n);
    }
    let mut names: Vec<(&str, usize)> = interfaces.iter().map(|(k, v)| (*k, *v)).collect();
    names.sort_by_key(|(_, i)| *i);
    let _ = writeln!(out, "pub static INTERFACES: [&str; {}] = [", names.len());
    for (name, _) in &names {
        let _ = writeln!(out, "    {name:?},");
    }
    out.push_str("];\n\n");

    let total: usize = versions.iter().map(|v| v.slots.len()).sum();
    let _ = writeln!(out, "pub const SLOT_COUNT: usize = {total};\n");
    let _ = writeln!(out, "pub static SLOTS: [SlotInfo; {total}] = [");
    for (vi, v) in versions.iter().enumerate() {
        for s in &v.slots {
            let _ = writeln!(
                out,
                "    SlotInfo {{ version: {vi}, method: {:?} }},",
                s.method
            );
        }
    }
    out.push_str("];\n\n");

    let abi = t.abi();
    let mut id = 0usize;
    let mut tables = String::new();
    for (vi, v) in versions.iter().enumerate() {
        let _ = writeln!(tables, "static VT_{vi}: [Fp; {}] = [", v.slots.len());
        for s in &v.slots {
            let imp = impls.iter_mut().find(|i| {
                i.interface == v.interface
                    && i.method == s.method
                    && i.params.len() == s.params.len()
                    && i.params.iter().zip(&s.params).all(|(o, t)| fits(t, o))
                    && fits(&s.ret, &i.ret)
            });
            let sret = returned_struct(&s.ret);
            let mut params = String::from("this: *mut crate::object::Object");
            if t.windows
                && let Some(r) = sret
            {
                let _ = write!(params, ", ret: *mut {r}");
            }
            let ret_ty = if t.windows
                && let Some(r) = sret
            {
                format!("*mut {r}")
            } else {
                return_type(&s.ret)
            };
            let body = match imp {
                Some(imp) => {
                    imp.used = true;
                    let mut args = Vec::new();
                    for (i, (table, ours)) in s.params.iter().zip(&imp.params).enumerate() {
                        let _ = write!(params, ", a{i}: {}", param_type(table));
                        args.push(if ours == "bool" {
                            format!("a{i} != 0")
                        } else {
                            format!("a{i}")
                        });
                    }
                    let call = format!(
                        "crate::{}(unsafe {{ &*this }}, {})",
                        imp.path,
                        args.join(", ")
                    );
                    let wrapped = match imp.ret.as_str() {
                        "bool" => format!("i8::from({call})"),
                        "steamid" => format!("crate::abi::CSteamID({call})"),
                        _ => call,
                    };
                    let fail = failure(&s.ret);
                    if t.windows && sret.is_some() {
                        format!(
                            "let v = crate::guard::run({id}, || {wrapped}, || {fail});\n    \
                             if !ret.is_null() {{ unsafe {{ ret.write(v) }}; }}\n    ret"
                        )
                    } else {
                        format!("crate::guard::run({id}, || {wrapped}, || {fail})")
                    }
                }
                None => {
                    if t.thiscall {
                        for w in 0..stack_words(s) {
                            let _ = write!(params, ", _p{w}: u32");
                        }
                    }
                    let fail = failure(&s.ret);
                    if t.windows && sret.is_some() {
                        format!(
                            "crate::report::stub({id}, this);\n    let v = {fail};\n    \
                             if !ret.is_null() {{ unsafe {{ ret.write(v) }}; }}\n    ret"
                        )
                    } else {
                        format!("crate::report::stub({id}, this);\n    {fail}")
                    }
                }
            };
            let _ = writeln!(
                out,
                "#[allow(unused_variables, clippy::all)]\nunsafe extern \"{abi}\" fn s{id}({params}) -> {ret_ty} {{\n    {body}\n}}"
            );
            let _ = writeln!(tables, "    Fp(s{id} as *const core::ffi::c_void),");
            id += 1;
        }
        tables.push_str("];\n");
    }
    out.push('\n');
    out.push_str(&tables);
    out.push('\n');

    let _ = writeln!(
        out,
        "pub static VERSIONS: [VersionInfo; {}] = [",
        versions.len()
    );
    let mut first = 0;
    for (vi, v) in versions.iter().enumerate() {
        let _ = writeln!(
            out,
            "    VersionInfo {{ name: c{:?}, interface: {}, first_slot: {first}, vtable: &VT_{vi} }},",
            v.name,
            interfaces[v.interface.as_str()]
        );
        first += v.slots.len();
    }
    out.push_str("];\n");
    out
}
