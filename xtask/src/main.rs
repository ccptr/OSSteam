// SPDX-License-Identifier: GPL-2.0-or-later
//! Repository tooling, run as `cargo xtask <command>`.
//!
//! - `check-layering`: enforces the crate dependency rules (SPEC WS-1 to
//!   WS-3, LEAF-2).
//! - `check-spdx`: enforces license headers and crate licenses (LIC-1).
//! - `update-protos <rev> [<file>...]`: re-vendors the Steam protobuf
//!   definitions at an upstream commit, applies local patches and rewrites
//!   `PROVENANCE.md` (LIC-5 to LIC-7).
//! - `import-steamworks <lsteamclient dir>`: rewrites the Steamworks
//!   vtable table (`docs/spec/steamworks/vtables.txt`) from Proton's
//!   interface wrappers.
//! - `build-steamclient [--release] [--target <triple>]`: builds the
//!   Steamworks client library for a 64-bit target and its 32-bit
//!   counterpart, laid out as the daemon expects (`lib32/`).
//! - `shaders [--check]`: compiles `ossteam-ui`'s Slang shaders to the
//!   committed SPIR-V, or checks that it matches (ADR 0004).

mod shaders;
mod steamclient;
mod steamworks;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use cargo_metadata::{DependencyKind, MetadataCommand};

const SPDX_LINE: &str = "// SPDX-License-Identifier: GPL-2.0-or-later";
const LICENSE: &str = "GPL-2.0-or-later";

/// Which workspace crates each workspace crate may depend on directly
/// (normal and build dependencies). Every allowed edge points down a layer,
/// so checking direct edges also bounds the transitive closure. A crate
/// missing from this table fails the check, so new crates get placed
/// deliberately.
///
/// `None` means any workspace crate except the leaf clients and the shim.
const ALLOWED: &[(&str, Option<&[&str]>)] = &[
    ("ossteam-types", Some(&[])),
    ("ossteam-vdf", Some(&["ossteam-types"])),
    ("ossteam-proto", Some(&["ossteam-types"])),
    (
        "ossteam-net",
        Some(&["ossteam-types", "ossteam-vdf", "ossteam-proto"]),
    ),
    (
        "ossteam-webapi",
        Some(&["ossteam-types", "ossteam-vdf", "ossteam-proto"]),
    ),
    ("ossteam-auth", Some(STEAM_SERVICES)),
    ("ossteam-pics", Some(STEAM_SERVICES)),
    ("ossteam-content", Some(STEAM_SERVICES)),
    ("ossteam-stats", Some(STEAM_SERVICES)),
    ("ossteam-launch", Some(STEAM_SERVICES)),
    ("ossteam-gameipc", Some(&["ossteam-types"])),
    ("ossteam-ipc", Some(&["ossteam-types"])),
    // WS-2: the shim sees only the game IPC protocol and plain types.
    (
        "ossteam-steamclient",
        Some(&["ossteam-gameipc", "ossteam-types"]),
    ),
    ("ossteamd", None),
    // LEAF-2: leaf clients reach the daemon through ossteam-ipc only;
    // the GUI also draws with ossteam-ui (WS-1).
    ("ossteam-cli", Some(&["ossteam-ipc", "ossteam-types"])),
    (
        "ossteam-gui",
        Some(&["ossteam-ipc", "ossteam-types", "ossteam-ui"]),
    ),
    // WS-1: the GUI toolkit depends only on our D-Bus client, which
    // depends on no other workspace crate.
    ("ossteam-ui", Some(&["ossteam-dbus"])),
    ("ossteam-dbus", Some(&[])),
    ("xtask", Some(&[])),
];

const STEAM_SERVICES: &[&str] = &[
    "ossteam-types",
    "ossteam-vdf",
    "ossteam-proto",
    "ossteam-net",
    "ossteam-webapi",
];

const NEVER_DEPENDED_ON: &[&str] = &["ossteam-cli", "ossteam-gui", "ossteam-steamclient", "xtask"];

/// Directories whose files are vendored verbatim and keep their upstream
/// notices (LIC-6).
const VENDORED: &[&str] = &["crates/ossteam-proto/protos"];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let result = match args.next().as_deref() {
        Some("check-layering") => check_layering(),
        Some("check-spdx") => check_spdx(),
        Some("update-protos") => match args.next() {
            Some(rev) => update_protos(&rev, args.collect()),
            None => Err(anyhow::anyhow!(
                "usage: cargo xtask update-protos <rev> [<file>...]"
            )),
        },
        Some("import-steamworks") => match args.next() {
            Some(dir) => steamworks::import(Path::new(&dir)),
            None => Err(anyhow::anyhow!(
                "usage: cargo xtask import-steamworks <Proton lsteamclient directory>"
            )),
        },
        Some("build-steamclient") => steamclient::build(args.collect()),
        Some("shaders") => shaders::run(args.collect()),
        Some(other) => Err(anyhow::anyhow!("unknown command {other:?}")),
        None => Err(anyhow::anyhow!(
            "usage: cargo xtask <check-layering|check-spdx|update-protos|import-steamworks|build-steamclient|shaders>"
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_owned()
}

fn check_layering() -> anyhow::Result<()> {
    let meta = MetadataCommand::new()
        .manifest_path(workspace_root().join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("running cargo metadata")?;
    let members: BTreeMap<&str, _> = meta
        .workspace_packages()
        .into_iter()
        .map(|p| (p.name.as_str(), p))
        .collect();
    let allowed: BTreeMap<&str, Option<&[&str]>> = ALLOWED.iter().copied().collect();

    let mut problems = Vec::new();
    for (name, pkg) in &members {
        let Some(rule) = allowed.get(name) else {
            problems.push(format!(
                "{name} is not in xtask's layering table; add it to ALLOWED"
            ));
            continue;
        };
        for dep in &pkg.dependencies {
            if dep.kind == DependencyKind::Development || !members.contains_key(dep.name.as_str()) {
                continue;
            }
            let ok = match rule {
                Some(list) => list.contains(&dep.name.as_str()),
                None => !NEVER_DEPENDED_ON.contains(&dep.name.as_str()),
            };
            if !ok {
                problems.push(format!("{name} must not depend on {}", dep.name));
            }
        }
    }
    report("layering", problems)
}

fn check_spdx() -> anyhow::Result<()> {
    let root = workspace_root();
    let mut problems = Vec::new();

    let meta = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("running cargo metadata")?;
    for pkg in meta.workspace_packages() {
        if pkg.license.as_deref() != Some(LICENSE) {
            problems.push(format!(
                "{}: license must be {LICENSE}, is {:?}",
                pkg.name, pkg.license
            ));
        }
    }

    if !root.join("COPYING").is_file() {
        problems.push("COPYING (the GPLv2 text) is missing".into());
    }

    let mut files = Vec::new();
    for dir in ["crates", "xtask", "fuzz"] {
        collect_sources(&root, &root.join(dir), &mut files)?;
    }
    for path in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if text.lines().next() != Some(SPDX_LINE) {
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            problems.push(format!(
                "{}: first line must be `{SPDX_LINE}`",
                rel.display()
            ));
        }
    }
    report("SPDX", problems)
}

fn collect_sources(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let path = entry?.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if VENDORED.iter().any(|v| rel.starts_with(v)) || rel.ends_with("target") {
            continue;
        }
        if path.is_dir() {
            collect_sources(root, &path, out)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "proto")
        ) {
            out.push(path);
        }
    }
    Ok(())
}

const PROTO_UPSTREAM: &str = "https://github.com/SteamDatabase/Protobufs";
const PROTO_UPSTREAM_DIR: &str = "steam";
const PROTO_CRATE: &str = "crates/ossteam-proto";

/// Replaces `protos/` with the upstream files at `rev`, then applies
/// `patches/*.patch` in name order (LIC-7).
///
/// The file list is whatever `protos/` already holds plus `extra`, which
/// are paths relative to upstream's `steam/` directory. To drop a file,
/// delete it and run this again.
fn update_protos(rev: &str, extra: Vec<String>) -> anyhow::Result<()> {
    if rev.len() != 40 || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("rev must be a full 40-digit commit hash, got {rev:?}");
    }
    let root = workspace_root();
    let crate_dir = root.join(PROTO_CRATE);
    let protos = crate_dir.join("protos");

    let mut files = Vec::new();
    if protos.is_dir() {
        collect_protos(&protos, &protos, &mut files)?;
    }
    for f in extra {
        let f = f.trim_start_matches("./").to_owned();
        if f.contains("..") || !f.ends_with(".proto") {
            bail!("{f:?} is not a relative .proto path");
        }
        files.push(f);
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        bail!("no files to vendor; name some after the rev");
    }

    let agent = http_agent();
    let mut fetched = Vec::new();
    for f in &files {
        let url = format!(
            "https://raw.githubusercontent.com/SteamDatabase/Protobufs/{rev}/{PROTO_UPSTREAM_DIR}/{f}"
        );
        let body = agent
            .get(&url)
            .call()
            .with_context(|| format!("fetching {url}"))?
            .body_mut()
            .read_to_vec()
            .with_context(|| format!("reading {url}"))?;
        fetched.push((f.clone(), body));
    }

    if protos.exists() {
        std::fs::remove_dir_all(&protos).context("clearing protos/")?;
    }
    for (f, body) in &fetched {
        let path = protos.join(f);
        std::fs::create_dir_all(path.parent().expect("file has a parent"))?;
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    }

    let mut patches = Vec::new();
    if let Ok(dir) = std::fs::read_dir(crate_dir.join("patches")) {
        for entry in dir {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "patch") {
                patches.push(path);
            }
        }
    }
    patches.sort();
    for patch in &patches {
        let status = std::process::Command::new("git")
            .arg("apply")
            .arg("-p1")
            .arg(format!("--directory={PROTO_CRATE}/protos"))
            .arg(patch)
            .current_dir(&root)
            .status()
            .context("running git apply")?;
        if !status.success() {
            bail!("{} does not apply", patch.display());
        }
    }

    let mut md = String::new();
    md.push_str("# Provenance of `protos/`\n\n");
    md.push_str("Generated by `cargo xtask update-protos`; do not edit by hand.\n\n");
    md.push_str("The files under `protos/` are unmodified copies of the community-maintained\n");
    md.push_str("Steam protobuf definitions (SPEC LIC-5, LIC-6), plus the patches listed\n");
    md.push_str("below (LIC-7). Upstream releases them into the public domain (The\n");
    md.push_str("Unlicense); they keep whatever notices they carry upstream.\n\n");
    md.push_str(&format!("- Upstream: <{PROTO_UPSTREAM}>\n"));
    md.push_str(&format!("- Commit: `{rev}`\n"));
    md.push_str(&format!("- Directory: `{PROTO_UPSTREAM_DIR}/`\n\n"));
    md.push_str("## Files\n\n");
    for f in &files {
        md.push_str(&format!("- `{f}`\n"));
    }
    md.push_str("\n## Patches\n\n");
    if patches.is_empty() {
        md.push_str("None.\n");
    }
    for p in &patches {
        let name = p.file_name().expect("patch has a name").to_string_lossy();
        md.push_str(&format!("- `patches/{name}`\n"));
    }
    std::fs::write(crate_dir.join("PROVENANCE.md"), md).context("writing PROVENANCE.md")?;

    println!(
        "vendored {} file(s) at {rev}, applied {} patch(es)",
        files.len(),
        patches.len()
    );
    Ok(())
}

/// The same TLS policy as the daemon (SEC-3): rustls with ring and the
/// platform verifier.
fn http_agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::Rustls)
                .unversioned_rustls_crypto_provider(std::sync::Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .new_agent()
}

fn collect_protos(base: &Path, dir: &Path, out: &mut Vec<String>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_protos(base, &path, out)?;
        } else if path.extension().is_some_and(|e| e == "proto") {
            let rel = path.strip_prefix(base).expect("under base");
            let parts: Vec<_> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect();
            out.push(parts.join("/"));
        }
    }
    Ok(())
}

fn report(what: &str, problems: Vec<String>) -> anyhow::Result<()> {
    if problems.is_empty() {
        println!("{what} check passed");
        return Ok(());
    }
    for p in &problems {
        eprintln!("  {p}");
    }
    bail!("{what} check found {} problem(s)", problems.len())
}
