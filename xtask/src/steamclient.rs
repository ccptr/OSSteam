// SPDX-License-Identifier: GPL-2.0-or-later
//! `build-steamclient`: builds the Steamworks client library for a 64-bit
//! target and its 32-bit counterpart, and lays them out the way the
//! daemon looks for them (`steamworks-abi.md` §1.4): the 64-bit library
//! next to the daemon's executable, the 32-bit one in `lib32/` below it.
//! On macOS there is no 32-bit library; the library holds both
//! architectures instead, joined with `lipo`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use cargo_metadata::MetadataCommand;

const PACKAGE: &str = "ossteam-steamclient";

/// The 32-bit target that goes with each 64-bit one. A 64-bit target
/// missing here gets no `lib32/`.
const PAIRS: &[(&str, &str)] = &[
    ("x86_64-unknown-linux-gnu", "i686-unknown-linux-gnu"),
    ("x86_64-pc-windows-msvc", "i686-pc-windows-msvc"),
    ("x86_64-pc-windows-gnu", "i686-pc-windows-gnu"),
];

/// The macOS targets a universal library is made of.
const UNIVERSAL: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

pub fn build(args: Vec<String>) -> anyhow::Result<()> {
    let mut release = false;
    let mut target = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--release" => release = true,
            "--target" => match args.next() {
                Some(t) => target = Some(t),
                None => bail!("--target needs a target triple"),
            },
            other => bail!("unknown argument {other:?}"),
        }
    }
    let root = crate::workspace_root();
    let target_dir: PathBuf = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("running cargo metadata")?
        .target_directory
        .into();
    let profile = if release { "release" } else { "debug" };

    // Without --target, cargo builds for the host into target/<profile>,
    // where the daemon is too.
    let host = host_triple()?;
    let triple = target.clone().unwrap_or_else(|| host.clone());
    let out = match &target {
        Some(t) => target_dir.join(t).join(profile),
        None => target_dir.join(profile),
    };
    cargo_build(&root, target.as_deref(), release)?;
    let file = library_file(&triple);
    if !out.join(&file).is_file() {
        bail!("cargo built no {} in {}", file, out.display());
    }
    println!("64-bit: {}", out.join(&file).display());

    if UNIVERSAL.contains(&triple.as_str()) {
        let other = UNIVERSAL
            .iter()
            .find(|t| **t != triple)
            .expect("two targets");
        cargo_build(&root, Some(other), release)?;
        let thin = out.join(&file);
        let fat = out.join(format!("{file}.universal"));
        let status = Command::new("lipo")
            .arg("-create")
            .arg(&thin)
            .arg(target_dir.join(other).join(profile).join(&file))
            .arg("-output")
            .arg(&fat)
            .status()
            .context("running lipo")?;
        if !status.success() {
            bail!("lipo could not join the {triple} and {other} libraries");
        }
        std::fs::rename(&fat, &thin)
            .with_context(|| format!("renaming {} to {}", fat.display(), thin.display()))?;
        println!("universal with {other}: {}", thin.display());
        return Ok(());
    }

    let Some(&(_, lib32)) = PAIRS.iter().find(|(t, _)| *t == triple) else {
        println!("{triple} has no 32-bit counterpart; no lib32/");
        return Ok(());
    };
    cargo_build(&root, Some(lib32), release)?;
    let built = target_dir.join(lib32).join(profile).join(&file);
    let dir = out.join("lib32");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::copy(&built, dir.join(&file))
        .with_context(|| format!("copying {} into {}", built.display(), dir.display()))?;
    println!("32-bit: {}", dir.join(&file).display());
    Ok(())
}

fn cargo_build(root: &Path, target: Option<&str>, release: bool) -> anyhow::Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["build", "--package", PACKAGE, "--lib"])
        .current_dir(root);
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
    if release {
        cmd.arg("--release");
    }
    let status = cmd.status().context("running cargo build")?;
    if !status.success() {
        bail!(
            "building {PACKAGE} for {} failed",
            target.unwrap_or("the host")
        );
    }
    Ok(())
}

fn host_triple() -> anyhow::Result<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = Command::new(rustc)
        .arg("-vV")
        .output()
        .context("running rustc -vV")?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_owned)
        .context("rustc -vV names no host")
}

/// The cdylib's file name on `triple`.
fn library_file(triple: &str) -> String {
    let name = PACKAGE.replace('-', "_");
    if triple.contains("windows") {
        format!("{name}.dll")
    } else if triple.contains("apple") {
        format!("lib{name}.dylib")
    } else {
        format!("lib{name}.so")
    }
}
