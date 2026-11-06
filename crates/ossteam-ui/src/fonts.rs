// SPDX-License-Identifier: LGPL-2.1-or-later
//! Finding the fonts without reading them all first (ADR 0004 step 3,
//! spike item 1).
//!
//! Reading the header of every font file the system has took 60 ms warm
//! and up to 2 s from a cold disk cache (2,703 faces), which alone broke
//! §20's 500 ms first frame. So the font thread first lists the font
//! directories (names only), loads just the files whose names look like
//! the platform's UI family, and hands over a text system with those;
//! then it reads the rest for fallback. Text that needs a face the UI
//! family lacks waits for the rest (`text.rs`).
//!
//! The UI family, most preferred first:
//!
//! - **Linux and the BSDs:** fontconfig's `sans-serif` alias, read from
//!   its configuration as fontconfig orders it: every `<prefer>` list in
//!   file order, then `<accept>`, then `<default>`.
//! - **Windows:** Segoe UI, then Tahoma (older Windows, Wine).
//! - **macOS:** SF (`SFNS.ttf`), then Helvetica Neue, then Helvetica.
//!
//! If no file of any of them is found, everything is read before the
//! first frame, as before.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cosmic_text::fontdb;

/// Where the fonts are and which family the UI uses.
pub(crate) struct Discovery {
    /// The UI family's name as its faces give it, if found.
    pub family: Option<String>,
    /// A database with the UI family's faces, or with every face when
    /// the family was not found.
    pub db: fontdb::Database,
    /// Font files not in `db` yet.
    pub rest: Vec<PathBuf>,
    /// The directories that were looked in, for error messages.
    pub dirs: Vec<PathBuf>,
}

/// Lists the font directories and loads the UI family's faces.
pub(crate) fn discover() -> Discovery {
    let (families, dirs) = platform();
    let files = font_files(&dirs);
    for family in &families {
        let wanted = normalise(family);
        if wanted.is_empty() {
            continue;
        }
        let (candidates, others): (Vec<_>, Vec<_>) =
            files.iter().cloned().partition(|f| file_may_be(f, &wanted));
        if candidates.is_empty() {
            continue;
        }
        let mut db = fontdb::Database::new();
        for file in &candidates {
            // A file that cannot be read is skipped, as fontdb does.
            let _ = db.load_font_file(file);
        }
        let name = db.faces().find_map(|face| {
            face.families
                .iter()
                .find(|(name, _)| normalise(name) == wanted)
                .map(|(name, _)| name.clone())
        });
        if let Some(name) = name {
            set_generic_families(&mut db, &name);
            return Discovery {
                family: Some(name),
                db,
                rest: others,
                dirs,
            };
        }
    }
    let mut db = fontdb::Database::new();
    for file in &files {
        let _ = db.load_font_file(file);
    }
    // No preferred family: fontdb's own defaults for the generic
    // families stand, and the first face shaping finds does the rest.
    Discovery {
        family: None,
        db,
        rest: Vec::new(),
        dirs,
    }
}

/// Reads the given files into a database of their own.
pub(crate) fn load_files(files: &[PathBuf]) -> fontdb::Database {
    let mut db = fontdb::Database::new();
    for file in files {
        let _ = db.load_font_file(file);
    }
    db
}

fn set_generic_families(db: &mut fontdb::Database, family: &str) {
    db.set_sans_serif_family(family);
}

/// Lower-case letters and digits only: "Segoe UI" and "segoeui", ".SF NS"
/// and "SFNS" compare equal.
fn normalise(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Whether a font file's name suggests it holds `family` (normalised):
/// `NotoSans-Bold.ttf`, `DejaVuSans.ttf`, `segoeuib.ttf`,
/// `Cantarell-VF.otf`, `Inter[wght].ttf`, but not `NotoSansArabic-Regular.ttf`
/// or `DejaVuSansMono.ttf`. Loading the file confirms it.
fn file_may_be(path: &Path, family: &str) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    // Walk the stem, matching the family's letters and skipping anything
    // else, then look at what follows.
    let mut wanted = family.chars().peekable();
    let mut rest = "";
    for (i, c) in stem.char_indices() {
        let Some(&next) = wanted.peek() else {
            rest = &stem[i..];
            break;
        };
        if !c.is_ascii_alphanumeric() {
            continue;
        }
        if c.to_ascii_lowercase() != next {
            return false;
        }
        wanted.next();
    }
    if wanted.peek().is_some() {
        return false;
    }
    let rest = rest.to_ascii_lowercase();
    rest.is_empty()
        || !rest.starts_with(|c: char| c.is_ascii_alphanumeric())
        // Windows: segoeuib, segoeuiz, segoeuisl; tahomabd.
        || rest.len() <= 2
        || STYLES.contains(&rest.as_str())
}

/// Style names a file name may add to its family's without a separator.
const STYLES: &[&str] = &[
    "regular",
    "italic",
    "oblique",
    "bold",
    "bolditalic",
    "light",
    "lightitalic",
    "medium",
    "semibold",
    "black",
    "thin",
    "variable",
];

/// Every font file under `dirs`, by name only: nothing is read.
fn font_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen_dirs = HashSet::new();
    let mut seen_files = HashSet::new();
    let mut files = Vec::new();
    for dir in dirs {
        walk(dir, &mut seen_dirs, &mut seen_files, &mut files);
    }
    files
}

fn walk(
    dir: &Path,
    seen_dirs: &mut HashSet<PathBuf>,
    seen_files: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) {
    // fontconfig's directories can overlap or link to each other.
    let Ok(canonical) = std::fs::canonicalize(dir) else {
        return;
    };
    if !seen_dirs.insert(canonical) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let mut path = entry.path();
        let (is_dir, is_file) = if kind.is_symlink() {
            // Follow it, and count the file under its target's name.
            let Ok(target) = std::fs::canonicalize(&path) else {
                continue;
            };
            let Ok(meta) = std::fs::metadata(&target) else {
                continue;
            };
            if meta.is_file() {
                path = target;
            }
            (meta.is_dir(), meta.is_file())
        } else {
            (kind.is_dir(), kind.is_file())
        };
        if is_dir {
            walk(&path, seen_dirs, seen_files, files);
        } else if is_file && is_font_file(&path) && seen_files.insert(path.clone()) {
            files.push(path);
        }
    }
}

fn is_font_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "ttf" | "ttc" | "otf" | "otc"
        )
    })
}

/// The UI families, most preferred first, and the font directories.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn platform() -> (Vec<String>, Vec<PathBuf>) {
    let config = fontconfig::load();
    let mut dirs = fontconfig::dirs(&config);
    if dirs.is_empty() {
        // No configuration: fontdb's list.
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            dirs.push(home.join(".fonts"));
            dirs.push(home.join(".local/share/fonts"));
        }
    }
    (fontconfig::sans_serif(&config), dirs)
}

#[cfg(windows)]
fn platform() -> (Vec<String>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    match std::env::var_os("SYSTEMROOT") {
        Some(root) => dirs.push(PathBuf::from(root).join("Fonts")),
        None => dirs.push(PathBuf::from(r"C:\Windows\Fonts")),
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let profile = PathBuf::from(profile);
        dirs.push(profile.join(r"AppData\Local\Microsoft\Windows\Fonts"));
        dirs.push(profile.join(r"AppData\Roaming\Microsoft\Windows\Fonts"));
    }
    (vec!["Segoe UI".into(), "Tahoma".into()], dirs)
}

#[cfg(target_os = "macos")]
fn platform() -> (Vec<String>, Vec<PathBuf>) {
    let mut dirs: Vec<PathBuf> = vec!["/System/Library/Fonts".into(), "/Library/Fonts".into()];
    // Downloadable fonts, as fontdb finds them.
    if let Ok(assets) = std::fs::read_dir("/System/Library/AssetsV2") {
        for entry in assets.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("com_apple_MobileAsset_Font")
            {
                dirs.push(entry.path());
            }
        }
    }
    dirs.push("/Network/Library/Fonts".into());
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Fonts"));
    }
    (
        vec!["SF NS".into(), "Helvetica Neue".into(), "Helvetica".into()],
        dirs,
    )
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(unix, not(any(target_os = "ios", target_os = "android")))
)))]
fn platform() -> (Vec<String>, Vec<PathBuf>) {
    (Vec::new(), Vec::new())
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
mod fontconfig {
    use std::path::{Path, PathBuf};

    use fontconfig_parser::FontConfig;

    /// The configuration, read the way fontdb reads it: `FONTCONFIG_FILE`,
    /// else the user's file and the system's.
    pub(super) fn load() -> FontConfig {
        let mut config = FontConfig::default();
        if let Some(file) = std::env::var_os("FONTCONFIG_FILE") {
            let _ = config.merge_config(Path::new(&file));
            return config;
        }
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        let read_local = match config_home {
            Some(dir) => config
                .merge_config(&dir.join("fontconfig/fonts.conf"))
                .is_err(),
            None => true,
        };
        if read_local {
            let _ = config.merge_config(Path::new("/etc/fonts/local.conf"));
        }
        let _ = config.merge_config(Path::new("/etc/fonts/fonts.conf"));
        config
    }

    pub(super) fn dirs(config: &FontConfig) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        config
            .dirs
            .iter()
            .filter_map(|dir| match dir.path.strip_prefix("~") {
                Ok(rest) => home.as_ref().map(|h| h.join(rest)),
                Err(_) => Some(dir.path.clone()),
            })
            .collect()
    }

    /// What `sans-serif` stands for, in fontconfig's order: an alias's
    /// `<prefer>` goes before the generic name, `<accept>` after it and
    /// `<default>` at the end, each list in the order the files are read.
    pub(super) fn sans_serif(config: &FontConfig) -> Vec<String> {
        let aliases = config.aliases.iter().filter(|a| {
            matches!(
                a.alias.to_ascii_lowercase().as_str(),
                "sans-serif" | "sans serif"
            )
        });
        let mut prefer = Vec::new();
        let mut accept = Vec::new();
        let mut default = Vec::new();
        for alias in aliases {
            prefer.extend(alias.prefer.iter().cloned());
            accept.extend(alias.accept.iter().cloned());
            default.extend(alias.default.iter().cloned());
        }
        let mut seen = std::collections::HashSet::new();
        prefer
            .into_iter()
            .chain(accept)
            .chain(default)
            .filter(|f| seen.insert(f.clone()))
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use fontconfig_parser::FontConfig;

        #[test]
        fn sans_serif_in_fontconfig_order() {
            let dir = std::env::temp_dir().join(format!("ossteam-ui-fc-{}", std::process::id()));
            std::fs::create_dir_all(dir.join("conf.d")).unwrap();
            let alias = |prefer: &str, accept: &str| {
                format!(
                    "<?xml version=\"1.0\"?><fontconfig><alias><family>sans-serif</family>\
                     <prefer>{prefer}</prefer><accept>{accept}</accept></alias></fontconfig>"
                )
            };
            std::fs::write(
                dir.join("conf.d/10-a.conf"),
                alias(
                    "<family>Noto Sans</family><family>DejaVu Sans</family>",
                    "<family>Tahoma</family>",
                ),
            )
            .unwrap();
            std::fs::write(
                dir.join("conf.d/20-b.conf"),
                alias("<family>Cantarell</family><family>Noto Sans</family>", ""),
            )
            .unwrap();
            std::fs::write(
                dir.join("conf.d/30-c.conf"),
                "<?xml version=\"1.0\"?><fontconfig><alias><family>Noto Sans</family>\
                 <default><family>sans-serif</family></default></alias><dir>/opt/fonts</dir>\
                 <dir>~/fonts</dir></fontconfig>",
            )
            .unwrap();
            std::fs::write(
                dir.join("fonts.conf"),
                format!(
                    "<?xml version=\"1.0\"?><fontconfig><include>{}</include></fontconfig>",
                    dir.join("conf.d").display()
                ),
            )
            .unwrap();
            let mut config = FontConfig::default();
            config.merge_config(&dir.join("fonts.conf")).unwrap();
            assert_eq!(
                super::sans_serif(&config),
                ["Noto Sans", "DejaVu Sans", "Cantarell", "Tahoma"]
            );
            let dirs = super::dirs(&config);
            assert_eq!(dirs[0], std::path::Path::new("/opt/fonts"));
            assert!(dirs[1].ends_with("fonts") && dirs[1].is_absolute());
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{file_may_be, normalise};

    #[test]
    fn file_names_that_may_hold_a_family() {
        let may = |file: &str, family: &str| file_may_be(Path::new(file), &normalise(family));
        assert!(may("/f/NotoSans-Regular.ttf", "Noto Sans"));
        assert!(may("/f/NotoSans-CondensedBold.ttf", "Noto Sans"));
        assert!(may("/f/NotoSans[wdth,wght].ttf", "Noto Sans"));
        assert!(!may("/f/NotoSansArabic-Regular.ttf", "Noto Sans"));
        assert!(!may("/f/NotoSansMono-Regular.ttf", "Noto Sans"));
        assert!(!may("/f/NotoSerif-Regular.ttf", "Noto Sans"));
        assert!(may("/f/DejaVuSans.ttf", "DejaVu Sans"));
        assert!(may("/f/DejaVuSans-Bold.ttf", "DejaVu Sans"));
        assert!(!may("/f/DejaVuSansMono.ttf", "DejaVu Sans"));
        assert!(may("C:/Windows/Fonts/segoeui.ttf", "Segoe UI"));
        assert!(may("C:/Windows/Fonts/segoeuib.ttf", "Segoe UI"));
        assert!(may("C:/Windows/Fonts/segoeuisl.ttf", "Segoe UI"));
        assert!(!may("C:/Windows/Fonts/segoeuiemoji.ttf", "Segoe UI"));
        assert!(may("C:/Windows/Fonts/tahomabd.ttf", "Tahoma"));
        assert!(may("/System/Library/Fonts/SFNS.ttf", "SF NS"));
        assert!(may("/System/Library/Fonts/SFNSItalic.ttf", "SF NS"));
        assert!(!may("/System/Library/Fonts/SFNSMono.ttf", "SF NS"));
        assert!(may("/f/Cantarell-VF.otf", "Cantarell"));
        assert!(!may("/f/Noto.ttf", "Noto Sans"));
    }
}
