// SPDX-License-Identifier: GPL-2.0-or-later
//! Compiles the vendored Steam protos (`protos/`, LIC-6) with protox and
//! generates Rust types with prost.
//!
//! Besides the message types, every `service` gets a module of zero-sized
//! method markers that implement `crate::ServiceMethod`, so callers name a
//! unified service method by type and get its request and response types
//! from the schema instead of from a hand-written table.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new("protos");
    println!("cargo:rerun-if-changed=protos");
    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort();
    // descriptor.proto is vendored only so the custom options resolve; its
    // types are not part of this crate's API.
    files.retain(|f| !f.starts_with("google/"));

    let fds = protox::compile(&files, [root]).expect("compile vendored Steam protos");
    prost_build::Config::new()
        .service_generator(Box::new(MethodMarkers))
        .compile_fds(fds)
        .expect("generate Steam types");
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("list protos/") {
        let path: PathBuf = entry.expect("read protos/ entry").path();
        if path.is_dir() {
            collect(base, &path, out);
        } else if path.extension().is_some_and(|e| e == "proto") {
            let rel = path.strip_prefix(base).expect("under protos/");
            let parts: Vec<_> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(parts.join("/"));
        }
    }
}

struct MethodMarkers;

impl prost_build::ServiceGenerator for MethodMarkers {
    fn generate(&mut self, service: prost_build::Service, buf: &mut String) {
        let module = snake(&service.name);
        writeln!(
            buf,
            "/// Methods of the `{}` unified service.",
            service.proto_name
        )
        .unwrap();
        writeln!(buf, "pub mod {module} {{").unwrap();
        for m in &service.methods {
            writeln!(buf, "    /// `{}.{}`", service.proto_name, m.proto_name).unwrap();
            writeln!(buf, "    #[derive(Debug, Clone, Copy)]").unwrap();
            writeln!(buf, "    pub struct {};", m.name.to_upper_camel()).unwrap();
            writeln!(
                buf,
                "    impl crate::ServiceMethod for {} {{\n        \
                 const SERVICE: &'static str = {:?};\n        \
                 const METHOD: &'static str = {:?};\n        \
                 type Request = super::{};\n        \
                 type Response = super::{};\n    }}",
                m.name.to_upper_camel(),
                service.proto_name,
                m.proto_name,
                m.input_type,
                m.output_type,
            )
            .unwrap();
        }
        writeln!(buf, "}}").unwrap();
    }
}

trait UpperCamel {
    fn to_upper_camel(&self) -> String;
}

impl UpperCamel for String {
    /// prost gives method names in snake case; markers are types.
    fn to_upper_camel(&self) -> String {
        self.split('_')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let mut c = p.chars();
                let first = c.next().expect("non-empty").to_ascii_uppercase();
                std::iter::once(first).chain(c).collect::<String>()
            })
            .collect()
    }
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
