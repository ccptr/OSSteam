// SPDX-License-Identifier: GPL-2.0-or-later
//! Compiles the IPC schema with protox (pure Rust, so no `protoc` is needed
//! on any platform) and generates Rust types with prost.

fn main() {
    let protos = ["proto/ossteam/ipc/v1/ipc.proto"];
    for p in &protos {
        println!("cargo:rerun-if-changed={p}");
    }
    let fds = protox::compile(protos, ["proto"]).expect("compile IPC protos");
    prost_build::Config::new()
        .compile_fds(fds)
        .expect("generate IPC types");
}
