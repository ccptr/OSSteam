// SPDX-License-Identifier: GPL-2.0-or-later
//! `config` (SPEC §15.2; `docs/spec/ipc.md` §5.4).

use std::fmt::Write as _;

use ossteam_ipc::v1;
use serde_json::{Value, json};

use crate::{Cli, Failure, connect, exit};

pub fn list(cli: &Cli) -> Result<(), Failure> {
    let settings = connect(cli, true)?.get_settings()?;
    if cli.json {
        let all: Vec<Value> = settings.iter().map(setting_json).collect();
        println!("{}", json!({ "settings": all }));
    } else if !cli.quiet {
        print!("{}", list_text(&settings));
    }
    Ok(())
}

pub fn get(cli: &Cli, key: &str) -> Result<(), Failure> {
    let settings = connect(cli, true)?.get_settings()?;
    let Some(s) = settings.iter().find(|s| s.key == key.trim()) else {
        return Err(Failure::new(
            exit::NOT_FOUND,
            format!("there is no setting {key:?}; 'ossteam config list' shows them all"),
        ));
    };
    if cli.json {
        println!("{}", setting_json(s));
    } else if !cli.quiet {
        println!("{}", s.value);
    }
    Ok(())
}

/// Sets an override, or with `value` `None` removes it.
pub fn set(cli: &Cli, key: &str, value: Option<&str>) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    if key.trim() == TAKE_OVER && value.map(str::trim) == Some("true") {
        warn_take_over(&client)?;
    }
    let s = client.set_setting(key, value)?;
    if cli.json {
        println!("{}", setting_json(&s));
    } else if !cli.quiet {
        println!("{} = {} ({})", s.key, s.value, source_text(s.source));
    }
    Ok(())
}

/// The setting whose risks are printed before it is turned on.
const TAKE_OVER: &str = "discovery.take_over_official";

/// Prints the take-over's risks, as the daemon describes the setting, on
/// standard error, whatever `--json` and `--quiet` say
/// (`steamworks-abi.md` §1.1.6).
fn warn_take_over(client: &ossteam_ipc::Client) -> Result<(), Failure> {
    let settings = client.get_settings()?;
    if let Some(s) = settings.iter().find(|s| s.key == TAKE_OVER) {
        eprintln!("warning: turning on {TAKE_OVER}. {}", s.description);
    }
    Ok(())
}

fn list_text(settings: &[v1::Setting]) -> String {
    let mut out = String::new();
    let key_w = settings
        .iter()
        .map(|s| s.key.len())
        .chain(["KEY".len()])
        .max()
        .unwrap_or(0);
    let value_w = settings
        .iter()
        .map(|s| s.value.len())
        .chain(["VALUE".len()])
        .max()
        .unwrap_or(0);
    let _ = writeln!(out, "{:<key_w$}  {:<value_w$}  SOURCE", "KEY", "VALUE");
    for s in settings {
        let _ = writeln!(
            out,
            "{:<key_w$}  {:<value_w$}  {}",
            s.key,
            s.value,
            source_text(s.source)
        );
    }
    out
}

fn source_name(raw: i32) -> &'static str {
    match v1::SettingSource::try_from(raw).unwrap_or(v1::SettingSource::Unspecified) {
        v1::SettingSource::Default => "default",
        v1::SettingSource::File => "file",
        v1::SettingSource::Override => "override",
        v1::SettingSource::Unspecified => "unknown",
    }
}

fn source_text(raw: i32) -> &'static str {
    match v1::SettingSource::try_from(raw).unwrap_or(v1::SettingSource::Unspecified) {
        v1::SettingSource::File => "from ossteamd.toml",
        other => source_name(other as i32),
    }
}

fn kind_name(raw: i32) -> &'static str {
    match v1::SettingKind::try_from(raw).unwrap_or(v1::SettingKind::Unspecified) {
        v1::SettingKind::Bool => "bool",
        v1::SettingKind::Integer => "integer",
        v1::SettingKind::String => "string",
        v1::SettingKind::Unspecified => "unknown",
    }
}

pub fn setting_json(s: &v1::Setting) -> Value {
    json!({
        "key": s.key,
        "kind": kind_name(s.kind),
        "value": s.value,
        "source": source_name(s.source),
        "default": s.default_value,
        "file_value": (!s.file_value.is_empty()).then_some(&s.file_value),
        "description": s.description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_aligns_columns() {
        let s = |key: &str, value: &str, source: v1::SettingSource| v1::Setting {
            key: key.into(),
            value: value.into(),
            source: source as i32,
            ..Default::default()
        };
        let text = list_text(&[
            s("a.long_key", "false", v1::SettingSource::Default),
            s("b", "true", v1::SettingSource::File),
        ]);
        assert_eq!(
            text,
            "KEY         VALUE  SOURCE\n\
             a.long_key  false  default\n\
             b           true   from ossteamd.toml\n"
        );
    }
}
