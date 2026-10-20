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
//! capsule, a hero and a logo, already in the daemon's image cache, so the
//! GUI loads and draws images as it would for a real library without
//! fetching any.

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
const HERO: &str = "library_hero.png";
const LOGO: &str = "logo.png";
/// The same at double size, which the GUI asks for when its scale factor
/// is above 1.
const CAPSULE_2X: &str = "library_600x900_2x.png";
const HERO_2X: &str = "library_hero_2x.png";
const LOGO_2X: &str = "logo_2x.png";
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
    let common = section(&assets());
    for i in 0..APPS {
        cm.app_section(app_id(i), "common", common.clone());
    }
    cm.account(account)
}

/// The keys of an app's `common` section that name its images.
pub(super) fn assets() -> String {
    format!(
        r#""store_asset_mtime" "{MTIME}"
           "library_assets_full"
           {{
               "library_capsule" {{
                   "image" {{ "english" "{CAPSULE}" }}
                   "image2x" {{ "english" "{CAPSULE_2X}" }}
               }}
               "library_hero" {{
                   "image" {{ "english" "{HERO}" }}
                   "image2x" {{ "english" "{HERO_2X}" }}
               }}
               "library_logo" {{
                   "image" {{ "english" "{LOGO}" }}
                   "image2x" {{ "english" "{LOGO_2X}" }}
               }}
           }}"#
    )
}

/// A `w` × `h` image in shades of one hue, the official size of each kind
/// (a 600 × 900 capsule, a 1920 × 620 hero, a 640 × 360 logo).
fn picture(variant: u32, w: u32, h: u32) -> Vec<u8> {
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
        .expect("encode an image");
    out
}

/// Puts the images of `apps` where the daemon caches them (`pics.md` §9).
pub(super) fn seed_images(root: &Root, apps: impl IntoIterator<Item = u32>) {
    let images = root.path().join("images");
    std::fs::create_dir_all(&images).expect("create the image directory");
    let kinds = [
        (CAPSULE, 600, 900),
        (HERO, 1920, 620),
        (LOGO, 640, 360),
        (CAPSULE_2X, 1200, 1800),
        (HERO_2X, 3840, 1240),
        (LOGO_2X, 1280, 720),
    ];
    let variants: Vec<Vec<_>> = (0..VARIANTS)
        .map(|v| {
            kinds
                .iter()
                .map(|&(name, w, h)| {
                    let path = images.join(format!("{v}-{name}"));
                    std::fs::write(&path, picture(v, w, h)).expect("write an image");
                    (name, path)
                })
                .collect()
        })
        .collect();
    let assets = root.path().join("cache").join("assets");
    for (i, app) in apps.into_iter().enumerate() {
        let dir = assets.join(app.to_string()).join(MTIME.to_string());
        std::fs::create_dir_all(&dir).expect("create the app's image directory");
        for (name, path) in &variants[i % VARIANTS as usize] {
            link(path, &dir.join(name));
        }
    }
}

fn link(from: &Path, to: &Path) {
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to).expect("copy an image");
    }
}

#[test]
#[ignore = "serves a daemon for the GUI until stopped"]
fn gui_bench_library_of_10k_apps() {
    let root = Root::new();
    seed_images(&root, (0..APPS).map(app_id));
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

    serve_gui(
        d,
        &root,
        &format!("A library of {APPS} apps"),
        "OSSTEAM_GUI_BENCH=library-scroll ",
    );
}

/// Prints how to run the GUI against `d`, and serves it until a client
/// stops it.
pub(super) fn serve_gui(d: TestDaemon, root: &Root, what: &str, gui_env: &str) {
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
    println!("{what} is ready. Run the GUI against it with");
    println!("  {var}={rt}{wayland} {gui_env}ossteam-gui --no-autostart");
    println!("and stop this daemon with");
    println!("  {var}={rt} ossteam daemon stop");
    d.wait();
}
