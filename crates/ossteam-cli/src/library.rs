// SPDX-License-Identifier: GPL-2.0-or-later
//! `library` and `info` (SPEC §15.2; `docs/spec/ipc.md` §5.3).

use std::fmt::Write as _;

use ossteam_ipc::Client;
use ossteam_ipc::v1::{self, AppSummary};
use serde_json::{Value, json};

use crate::{Cli, Failure, connect, exit};

pub fn list(cli: &Cli, installed: bool, filter: &str) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let r = client.list_apps(filter, installed)?;
    if cli.json {
        println!(
            "{}",
            json!({
                "complete": r.complete,
                "apps": r.apps.iter().map(app_json).collect::<Vec<_>>(),
                "packages_without_info": r.packages_without_info,
            })
        );
        return Ok(());
    }
    if !cli.quiet {
        print!("{}", library_text(&r.apps));
    }
    library_notes(&r);
    Ok(())
}

fn library_text(apps: &[AppSummary]) -> String {
    let mut out = String::new();
    if apps.is_empty() {
        return out;
    }
    let width = apps
        .iter()
        .flat_map(|a| std::iter::once(a).chain(&a.dlc))
        .map(|a| a.app_id.to_string().len())
        .max()
        .unwrap_or(0)
        .max("APP ID".len());
    let _ = writeln!(out, "{:<width$}  NAME", "APP ID");
    for a in apps {
        let _ = writeln!(out, "{:<width$}  {}", a.app_id, label(a, false));
        for d in &a.dlc {
            let _ = writeln!(out, "{:<width$}    {}", d.app_id, label(d, true));
        }
    }
    out
}

/// An app's name and type for listings. Apps without info show their id.
/// A DLC that is not `nested` under its game says which game it is for.
fn label(a: &AppSummary, nested: bool) -> String {
    if !a.has_info {
        return format!("(app {}: info not available)", a.app_id);
    }
    let mut s = if a.name.is_empty() {
        format!("(app {})", a.app_id)
    } else {
        a.name.clone()
    };
    if !a.r#type.is_empty() && !a.r#type.eq_ignore_ascii_case("game") {
        let _ = write!(s, " [{}]", a.r#type);
    }
    if a.parent != 0 && !nested {
        let _ = write!(s, " (for app {})", a.parent);
    }
    s
}

/// What the listing cannot show, on stderr so the list stays clean.
fn library_notes(r: &v1::ListAppsResponse) {
    if !r.complete {
        eprintln!("ossteam: the library is still loading; this list may be incomplete");
    }
    if !r.packages_without_info.is_empty() {
        eprintln!(
            "ossteam: the info of {} licensed package(s) is not available, so their apps are not listed",
            r.packages_without_info.len()
        );
    }
}

pub fn info(cli: &Cli, app: &str) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let id = resolve(&client, app)?;
    let r = client.get_app(id)?;
    if cli.json {
        println!("{}", info_json(&r));
    } else if !cli.quiet {
        print!("{}", info_text(&r));
    }
    Ok(())
}

/// An app id, or the exact name of an app in the library, ignoring case
/// (CLI-1).
fn resolve(client: &Client, app: &str) -> Result<u32, Failure> {
    if let Ok(id) = app.trim().parse() {
        return Ok(id);
    }
    let r = client.list_apps("", false)?;
    let matches: Vec<&AppSummary> = r
        .apps
        .iter()
        .flat_map(|a| std::iter::once(a).chain(&a.dlc))
        .filter(|a| a.name.eq_ignore_ascii_case(app.trim()))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.app_id),
        [] => {
            let mut message = format!("no app in the library is named {app:?}");
            if !r.complete {
                message.push_str(" (the library is still loading)");
            }
            Err(Failure::new(exit::NOT_FOUND, message))
        }
        many => {
            let list: Vec<String> = many
                .iter()
                .map(|a| format!("  {}  {}", a.app_id, label(a, false)))
                .collect();
            Err(Failure::new(
                exit::OTHER,
                format!(
                    "{app:?} names several apps; use an app id:\n{}",
                    list.join("\n")
                ),
            ))
        }
    }
}

fn info_text(r: &v1::GetAppResponse) -> String {
    let mut out = String::new();
    let Some(a) = &r.app else {
        return out;
    };
    let name = if a.name.is_empty() {
        "(no name)"
    } else {
        &a.name
    };
    let _ = writeln!(out, "{name} ({})", a.app_id);
    let mut field = |k: &str, v: &str| {
        if !v.is_empty() {
            let _ = writeln!(out, "  {k:<14}{v}");
        }
    };
    if !a.has_info {
        field("info", "not available");
    }
    field("type", &a.r#type);
    if a.parent != 0 {
        field("DLC for", &a.parent.to_string());
    }
    field("platforms", &r.oslist.join(", "));
    field("install dir", &r.install_dir);
    field("installed", if a.installed { "yes" } else { "no" });
    if r.change_number != 0 {
        field("change number", &r.change_number.to_string());
    }
    if r.missing_token {
        field("note", "Steam sent limited info: no access token");
    }
    if !a.dlc.is_empty() {
        let _ = writeln!(out, "DLC");
        for d in &a.dlc {
            let _ = writeln!(out, "  {:<10}{}", d.app_id, label(d, true));
        }
    }
    if !r.depots.is_empty() {
        let _ = writeln!(out, "depots");
        for d in &r.depots {
            let mut line = format!("  {:<10}{}", d.depot_id, d.name);
            let mut tags = Vec::new();
            if !d.oslist.is_empty() {
                tags.push(d.oslist.join(","));
            }
            if !d.osarch.is_empty() {
                tags.push(format!("{}-bit", d.osarch));
            }
            if !d.language.is_empty() {
                tags.push(d.language.clone());
            }
            if d.dlc_app_id != 0 {
                tags.push(format!("DLC {}", d.dlc_app_id));
            }
            if d.from_app_id != 0 {
                tags.push(format!("from app {}", d.from_app_id));
            }
            if d.max_size != 0 {
                tags.push(format!("up to {}", size(d.max_size)));
            }
            if !tags.is_empty() {
                let _ = write!(line, "  ({})", tags.join("; "));
            }
            let _ = writeln!(out, "{}", line.trim_end());
        }
    }
    if !r.branches.is_empty() {
        let _ = writeln!(out, "branches");
        for b in &r.branches {
            let mut line = format!("  {:<14}", b.name);
            if b.build_id != 0 {
                let _ = write!(line, "build {:<10}", b.build_id);
            }
            if b.time_updated != 0 {
                let _ = write!(line, "updated {}  ", date(b.time_updated));
            }
            if b.password_required {
                line.push_str("password  ");
            }
            if !b.description.is_empty() {
                let _ = write!(line, "{:?}", b.description);
            }
            let _ = writeln!(out, "{}", line.trim_end());
        }
    }
    if !r.launch_options.is_empty() {
        let _ = writeln!(out, "launch options");
        for l in &r.launch_options {
            let mut line = format!("  {:<4}{}", l.index, l.executable);
            if !l.arguments.is_empty() {
                let _ = write!(line, " {}", l.arguments);
            }
            let mut tags = Vec::new();
            if !l.description.is_empty() {
                tags.push(l.description.clone());
            }
            if !l.r#type.is_empty() {
                tags.push(l.r#type.clone());
            }
            if !l.oslist.is_empty() {
                tags.push(l.oslist.join(","));
            }
            if !l.osarch.is_empty() {
                tags.push(format!("{}-bit", l.osarch));
            }
            if !l.betakey.is_empty() {
                tags.push(format!("branch {}", l.betakey));
            }
            if !tags.is_empty() {
                let _ = write!(line, "  ({})", tags.join("; "));
            }
            let _ = writeln!(out, "{line}");
        }
    }
    out
}

fn app_json(a: &AppSummary) -> Value {
    json!({
        "app_id": a.app_id,
        "name": a.name,
        "type": a.r#type,
        "parent": (a.parent != 0).then_some(a.parent),
        "has_info": a.has_info,
        "installed": a.installed,
        "dlc": a.dlc.iter().map(app_json).collect::<Vec<_>>(),
    })
}

fn info_json(r: &v1::GetAppResponse) -> Value {
    json!({
        "app": r.app.as_ref().map(app_json),
        "change_number": r.change_number,
        "missing_token": r.missing_token,
        "oslist": r.oslist,
        "install_dir": r.install_dir,
        "depots": r.depots.iter().map(|d| json!({
            "depot_id": d.depot_id,
            "name": d.name,
            "oslist": d.oslist,
            "osarch": d.osarch,
            "language": d.language,
            "dlc_app_id": (d.dlc_app_id != 0).then_some(d.dlc_app_id),
            "from_app_id": (d.from_app_id != 0).then_some(d.from_app_id),
            "shared_install": d.shared_install,
            "max_size": (d.max_size != 0).then_some(d.max_size),
        })).collect::<Vec<_>>(),
        "branches": r.branches.iter().map(|b| json!({
            "name": b.name,
            "build_id": (b.build_id != 0).then_some(b.build_id),
            "description": b.description,
            "password_required": b.password_required,
            "time_updated": (b.time_updated != 0).then_some(b.time_updated),
        })).collect::<Vec<_>>(),
        "launch_options": r.launch_options.iter().map(|l| json!({
            "index": l.index,
            "executable": l.executable,
            "arguments": l.arguments,
            "description": l.description,
            "type": l.r#type,
            "oslist": l.oslist,
            "osarch": l.osarch,
            "betakey": l.betakey,
        })).collect::<Vec<_>>(),
    })
}

/// A byte count in binary units.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// A Unix time as a UTC calendar date, `YYYY-MM-DD`.
fn date(unix: u64) -> String {
    // Days to civil date, after Howard Hinnant's `civil_from_days`.
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: u32, name: &str, kind: &str) -> AppSummary {
        AppSummary {
            app_id: id,
            name: name.into(),
            r#type: kind.into(),
            has_info: true,
            ..Default::default()
        }
    }

    #[test]
    fn listing_nests_dlc_and_marks_what_is_missing() {
        let mut game = app(480, "Spacewar", "Game");
        game.dlc.push(AppSummary {
            parent: 480,
            ..app(110_902, "Spacewar Pack", "DLC")
        });
        let orphan = AppSummary {
            parent: 999,
            ..app(500, "Orphan", "DLC")
        };
        let unknown = AppSummary {
            app_id: 7,
            ..Default::default()
        };
        let text = library_text(&[game, orphan, unknown]);
        assert_eq!(
            text,
            "APP ID  NAME\n\
             480     Spacewar\n\
             110902    Spacewar Pack [DLC]\n\
             500     Orphan [DLC] (for app 999)\n\
             7       (app 7: info not available)\n"
        );
        assert!(library_text(&[]).is_empty());
    }

    #[test]
    fn sizes_and_dates() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(1_048_576), "1.0 MiB");
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_700_000_000), "2023-11-14");
        assert_eq!(date(951_782_400), "2000-02-29");
    }

    #[test]
    fn info_lists_sections_that_have_content() {
        let r = v1::GetAppResponse {
            app: Some(app(480, "Spacewar", "Game")),
            change_number: 9,
            oslist: vec!["windows".into(), "linux".into()],
            branches: vec![v1::Branch {
                name: "public".into(),
                build_id: 1234,
                time_updated: 1_700_000_000,
                ..Default::default()
            }],
            ..Default::default()
        };
        let text = info_text(&r);
        assert!(text.starts_with("Spacewar (480)\n"));
        assert!(text.contains("  platforms     windows, linux\n"));
        assert!(text.contains("  installed     no\n"));
        assert!(text.contains("  public        build 1234      updated 2023-11-14\n"));
        assert!(!text.contains("depots"));
    }
}
