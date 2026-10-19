// SPDX-License-Identifier: GPL-2.0-or-later
//! A library of 10,000 apps from the fake CM for the GUI's scrolling
//! budget (SPEC §20, `gui.md` §7). Not a test: it serves a logged-in
//! daemon until a client stops it, so it is ignored unless asked for:
//!
//! ```text
//! cargo test -p ossteamd --release -- --ignored --nocapture gui_bench
//! ```
//!
//! It prints how to run the GUI against that daemon. Every app has a
//! capsule, already in the daemon's image cache, so the GUI loads and
//! draws images as it would for a real library without fetching any.

use std::path::Path;

use ossteam_ipc::v1::Topic;
use ossteam_types::Secret;

use super::*;
use crate::testing::m1b::section;

const APPS: u32 = 10_000;
const PER_PACKAGE: u32 = 100;
const FIRST_APP: u32 = 100_000;
/// `common/store_asset_mtime` of every app.
const MTIME: u64 = 1;
const CAPSULE: &str = "library_600x900.png";
/// Different images, so that the GUI does not draw one texture throughout.
const VARIANTS: u32 = 16;

fn app_id(i: u32) -> u32 {
    FIRST_APP + i * 10
}

fn steam() -> FakeCm {
    let mut cm = FakeCm::new();
    let mut account = Account::new("bench", 4242);
    for p in 0..APPS / PER_PACKAGE {
        let apps: Vec<u32> = (p * PER_PACKAGE..(p + 1) * PER_PACKAGE)
            .map(app_id)
            .collect();
        cm = cm.package(1000 + p, &apps);
        account = account.license(1000 + p, 0);
    }
    for i in 0..APPS {
        cm = cm.app(app_id(i), &format!("Bench Game {i:05}"), "Game", None);
    }
    let common = section(&format!(
        r#""store_asset_mtime" "{MTIME}"
           "library_assets_full" {{ "library_capsule" {{ "image" {{ "english" "{CAPSULE}" }} }} }}"#
    ));
    for i in 0..APPS {
        cm.app_section(app_id(i), "common", common.clone());
    }
    cm.account(account)
}

/// A 600 × 900 capsule in shades of one hue.
fn capsule(variant: u32) -> Vec<u8> {
    let (w, h) = (600u32, 900u32);
    let hue = [
        (variant * 37 % 256) as u8,
        (variant * 91 % 256) as u8,
        (variant * 151 % 256) as u8,
    ];
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let shade = ((x + y) * 255 / (w + h)) as u8;
            pixels.extend(hue.iter().map(|&c| c / 2 + shade / 2));
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&pixels))
        .expect("encode the capsule");
    out
}

/// Puts every app's capsule where the daemon caches it (`pics.md` §9).
fn seed_images(root: &Root) {
    let images = root.path().join("images");
    std::fs::create_dir_all(&images).expect("create the image directory");
    let variants: Vec<_> = (0..VARIANTS)
        .map(|v| {
            let path = images.join(format!("{v}.png"));
            std::fs::write(&path, capsule(v)).expect("write a capsule");
            path
        })
        .collect();
    let assets = root.path().join("cache").join("assets");
    for i in 0..APPS {
        let dir = assets.join(app_id(i).to_string()).join(MTIME.to_string());
        std::fs::create_dir_all(&dir).expect("create the app's image directory");
        link(&variants[(i % VARIANTS) as usize], &dir.join(CAPSULE));
    }
}

fn link(from: &Path, to: &Path) {
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to).expect("copy a capsule");
    }
}

#[test]
#[ignore = "serves a daemon for the GUI until stopped"]
fn gui_bench_library_of_10k_apps() {
    let root = Root::new();
    seed_images(&root);
    let cm = steam();
    let d = TestDaemon::start(&root, &cm);
    let c = d.subscribed(&[Topic::Session]);
    c.login_with_credentials("bench", &Secret::new("bench-password".into()), true)
        .expect("the login starts");
    let ended = wait_event(&c, "the login to end", |k| match k {
        event::Kind::LoginEnded(e) => Some(e.error.clone()),
        _ => None,
    });
    assert!(ended.is_none(), "the login failed: {ended:?}");
    wait_until("the whole library", || {
        let r = c.list_apps("", false).expect("list apps");
        r.complete && r.apps.len() == APPS as usize
    });
    drop(c);

    let runtime = root.runtime_dir();
    let rt = runtime.display();
    let var = if cfg!(target_os = "macos") {
        "TMPDIR"
    } else {
        "XDG_RUNTIME_DIR"
    };
    // Wayland's socket is found through XDG_RUNTIME_DIR unless the display
    // is a path.
    let wayland = match (
        std::env::var("WAYLAND_DISPLAY"),
        std::env::var("XDG_RUNTIME_DIR"),
    ) {
        (Ok(d), Ok(dir)) if !d.starts_with('/') => format!(" WAYLAND_DISPLAY={dir}/{d}"),
        _ => String::new(),
    };
    println!("A library of {APPS} apps is ready. Run the GUI against it with");
    println!("  {var}={rt}{wayland} OSSTEAM_GUI_BENCH=library-scroll ossteam-gui --no-autostart");
    println!("and stop this daemon with");
    println!("  {var}={rt} ossteam daemon stop");
    d.wait();
}
