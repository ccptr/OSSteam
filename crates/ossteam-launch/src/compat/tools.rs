// SPDX-License-Identifier: GPL-2.0-or-later
//! The tool list in app 891390 and which tool an app gets
//! (`compat-tools.md` §1 and §2).

use ossteam_types::AppId;
use ossteam_vdf::{Object, Value};

/// One entry of `extended/compat_tools`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tool {
    /// The entry's key, as written.
    pub name: String,
    pub app: AppId,
    pub depot: u32,
    /// `require_tool_appid` in the list; the tool's manifest has the
    /// final say (§3).
    pub requires: Option<AppId>,
    pub display_name: String,
    pub from_os: Vec<String>,
    pub to_os: Vec<String>,
    pub aliases: Vec<String>,
    /// `unlisted` is `1`.
    pub unlisted: bool,
    pub legacy: bool,
}

impl Tool {
    /// Whether the tool runs programs of `os` (`windows`, `linux`).
    pub fn runs(&self, os: &str) -> bool {
        self.from_os.iter().any(|o| o.eq_ignore_ascii_case(os))
    }

    /// The platform whose depots and launch options an app run by this
    /// tool uses (§4): the first of `from_oslist`.
    pub fn platform(&self) -> Option<&str> {
        self.from_os.first().map(String::as_str)
    }
}

/// An alias the user can choose (`extended/selectable_aliases`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectableAlias {
    pub alias: String,
    pub display_name: String,
}

/// One entry of `extended/app_compat_configs` (§5.3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppConfig {
    /// For `STEAM_COMPAT_CONFIG`; empty when absent.
    pub config: String,
    /// `NAME=VALUE`; empty when absent.
    pub environment: String,
}

/// The tool list (§1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolList {
    pub tools: Vec<Tool>,
    pub selectable: Vec<SelectableAlias>,
    /// `extended/app_mappings`: the app and the tool's name, for entries
    /// whose `platform` is `linux`.
    pub mappings: Vec<(AppId, String)>,
    pub configs: Vec<(AppId, AppConfig)>,
}

/// Where a mapping came from (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    User,
    /// App 891390's `app_mappings`.
    Valve,
    /// The app's own `config/app_mappings`.
    App,
    /// The app has a Linux version.
    Native,
    /// `compat.default_tool`.
    Default,
}

/// The mapping that won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping<'a> {
    pub tool: &'a Tool,
    pub source: Source,
    pub priority: u32,
}

/// What [`ToolList::resolve`] needs to know about an app.
#[derive(Debug, Clone, Copy)]
pub struct MappingInput<'a> {
    pub app: AppId,
    /// The app's info (the contents of `appinfo`), when known.
    pub appinfo: Option<&'a Object>,
    /// The user's `compat.tool.<app id>`; empty for none.
    pub user: &'a str,
    /// `compat.default_tool`; empty for none.
    pub default_tool: &'a str,
}

/// The priorities from the official client's log (§2).
const USER: u32 = 250;
const VALVE: u32 = 100;
const APP: u32 = 90;
const NATIVE: u32 = 75;
const DEFAULT: u32 = 75;

impl ToolList {
    /// The list in app 891390's info. Entries that are not objects, and
    /// tools without an app id, are skipped.
    pub fn parse(appinfo: &Object) -> Self {
        let mut list = Self::default();
        let Some(ext) = appinfo.get_object("extended") else {
            return list;
        };
        for (name, v) in objects(ext.get_object("compat_tools")) {
            let Some(app) = number(v, "appid").map(AppId) else {
                continue;
            };
            list.tools.push(Tool {
                name: name.to_owned(),
                app,
                depot: number(v, "depotid").unwrap_or(0),
                requires: number(v, "require_tool_appid")
                    .filter(|&a| a != 0)
                    .map(AppId),
                display_name: text(v, "display_name"),
                from_os: split(&text(v, "from_oslist")),
                to_os: split(&text(v, "to_oslist")),
                aliases: split(&text(v, "aliases")),
                unlisted: number(v, "unlisted") == Some(1),
                legacy: number(v, "legacy").is_some_and(|n| n != 0),
            });
        }
        for (alias, v) in objects(ext.get_object("selectable_aliases")) {
            list.selectable.push(SelectableAlias {
                alias: alias.to_owned(),
                display_name: text(v, "display_name"),
            });
        }
        for (_, v) in objects(ext.get_object("app_mappings")) {
            if let Some(app) = number(v, "appid")
                && text(v, "platform").eq_ignore_ascii_case("linux")
            {
                list.mappings.push((AppId(app), text(v, "tool")));
            }
        }
        for (_, v) in objects(ext.get_object("app_compat_configs")) {
            if let Some(app) = number(v, "appid") {
                list.configs.push((
                    AppId(app),
                    AppConfig {
                        config: text(v, "config"),
                        environment: text(v, "environment"),
                    },
                ));
            }
        }
        list
    }

    /// The tool `name` names: a key, or else an alias, without regard to
    /// case (§1).
    pub fn find(&self, name: &str) -> Option<&Tool> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        self.tools
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .or_else(|| {
                self.tools
                    .iter()
                    .find(|t| t.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)))
            })
    }

    pub fn by_app(&self, app: AppId) -> Option<&Tool> {
        self.tools.iter().find(|t| t.app == app)
    }

    pub fn config(&self, app: AppId) -> Option<&AppConfig> {
        self.configs.iter().find(|(a, _)| *a == app).map(|(_, c)| c)
    }

    /// Whether `name` is a tool or a selectable alias that the user may
    /// choose (§8).
    pub fn is_choosable(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The tools and selectable aliases offered for an app with launch
    /// options for `platforms` (§8): listed tools that run one of them,
    /// in list order, then the selectable aliases whose tool does. Each
    /// is the name to choose and the tool it stands for.
    pub fn choices(&self, platforms: &[&str]) -> Vec<(&str, &str, &Tool)> {
        let runs = |t: &Tool| platforms.iter().any(|p| t.runs(p));
        let mut out: Vec<(&str, &str, &Tool)> = self
            .tools
            .iter()
            .filter(|t| !t.unlisted && runs(t))
            .map(|t| (t.name.as_str(), t.display_name.as_str(), t))
            .collect();
        for s in &self.selectable {
            if let Some(t) = self.find(&s.alias).filter(|t| runs(t)) {
                out.push((s.alias.as_str(), s.display_name.as_str(), t));
            }
        }
        out
    }

    /// The mapping with the highest priority, or `None` when the app has
    /// none and runs natively (§2). A mapping only replaces one of a
    /// lower priority, so among equals the first of the table wins.
    pub fn resolve(&self, input: MappingInput<'_>) -> Option<Mapping<'_>> {
        // A tool is a program for its `to_oslist` and runs as one.
        if self.by_app(input.app).is_some() {
            return None;
        }
        let mut best: Option<Mapping<'_>> = None;
        let mut offer = |name: &str, source, priority| {
            let Some(tool) = self.find(name) else {
                return;
            };
            if best.as_ref().is_none_or(|b| priority > b.priority) {
                best = Some(Mapping {
                    tool,
                    source,
                    priority,
                });
            }
        };
        offer(input.user, Source::User, USER);
        for (app, tool) in &self.mappings {
            if *app == input.app {
                offer(tool, Source::Valve, VALVE);
            }
        }
        let info = input.appinfo;
        if let Some(own) = info.and_then(|i| i.get_path(&["config", "app_mappings"])) {
            for (_, v) in objects(own.as_object()) {
                if text(v, "platform").eq_ignore_ascii_case("linux") {
                    let priority = number(v, "priority").unwrap_or(APP);
                    offer(&text(v, "tool"), Source::App, priority);
                }
            }
        }
        if has_linux_version(info) {
            offer("native", Source::Native, NATIVE);
        } else {
            offer(input.default_tool, Source::Default, DEFAULT);
        }
        best
    }
}

/// Whether `common/oslist` names `linux`.
pub fn has_linux_version(appinfo: Option<&Object>) -> bool {
    appinfo
        .and_then(|i| i.get_path(&["common", "oslist"]))
        .and_then(Value::to_text_scalar)
        .is_some_and(|l| split(&l).iter().any(|o| o.eq_ignore_ascii_case("linux")))
}

fn objects(o: Option<&Object>) -> impl Iterator<Item = (&str, &Object)> {
    o.into_iter()
        .flat_map(Object::iter)
        .filter_map(|(k, v)| Some((k, v.as_object()?)))
}

fn text(o: &Object, key: &str) -> String {
    o.get(key)
        .and_then(Value::to_text_scalar)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

fn number(o: &Object, key: &str) -> Option<u32> {
    o.get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()))
        .and_then(|n| u32::try_from(n).ok())
}

fn split(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A slice of app 891390's info as the official client cached it on
    /// 2026-10-18, in text form.
    pub(crate) const MANIFESTS: &str = r#"
"appinfo"
{
    "appid" "891390"
    "common" { "name" "SteamPlay 2.0 Manifests" "type" "Config" "oslist" "linux" }
    "extended"
    {
        "selectable_aliases" { "proton-stable" { "display_name" "Proton Stable" } }
        "compat_tools"
        {
            "proton_experimental"
            {
                "appid" "1493710" "depotid" "1493711" "require_tool_appid" "4183110"
                "display_name" "Proton Experimental" "from_oslist" "windows"
                "to_oslist" "linux" "aliases" "proton-experimental"
            }
            "proton_11"
            {
                "appid" "4628710" "depotid" "4628711" "require_tool_appid" "4183110"
                "display_name" "Proton 11.0-2" "from_oslist" "windows" "to_oslist" "linux"
                "aliases" "proton-9,proton-stable,proton-11.0-beta,proton-11.0-2RC"
            }
            "proton_5"
            {
                "appid" "1245040" "depotid" "1245041" "display_name" "Proton 5.0-10"
                "from_oslist" "windows" "to_oslist" "linux" "aliases" "proton-5.0-10"
                "legacy" "1"
            }
            "steamlinuxruntime_4"
            {
                "appid" "4183110" "depotid" "4183111" "display_name" "Steam Linux Runtime 4.0"
                "from_oslist" "linux" "to_oslist" "linux" "unlisted" "1"
                "aliases" "steamrt4,steamrt4-any"
            }
            "steamlinuxruntime_sniper"
            {
                "appid" "1628350" "depotid" "1628351" "display_name" "Steam Linux Runtime 3.0"
                "from_oslist" "linux" "to_oslist" "linux" "unlisted" "1" "aliases" "sniper"
            }
            "steamlinuxruntime"
            {
                "appid" "1070560" "depotid" "1070561" "display_name" "Steam Linux Runtime 1.0"
                "from_oslist" "linux" "to_oslist" "linux" "unlisted" "0"
                "aliases" "native,scout,SteamLinuxRuntime_scout"
            }
        }
        "app_mappings"
        {
            "730" { "appid" "730" "platform" "linux" "tool" "SteamLinuxRuntime_sniper" "config" "none" "comment" "cs2" }
            "39210" { "appid" "39210" "platform" "linux" "tool" "proton-11.0-beta" "config" "none" }
            "227000" { "appid" "227000" "platform" "linux" "tool" "" "config" "none" }
            "1" { "appid" "1" "platform" "windows" "tool" "proton_5" }
        }
        "app_compat_configs"
        {
            "7670" { "appid" "7670" "config" "noesync,nofsync" "comment" "BioShock" }
            "368340" { "appid" "368340" "environment" "DXVK_HDR=0" }
        }
    }
}
"#;

    pub(crate) fn list() -> ToolList {
        ToolList::parse(
            ossteam_vdf::text::parse(MANIFESTS)
                .unwrap()
                .get_object("appinfo")
                .unwrap(),
        )
    }

    fn info(text: &str) -> Object {
        ossteam_vdf::text::parse(text)
            .unwrap()
            .get_object("appinfo")
            .unwrap()
            .clone()
    }

    fn input<'a>(app: u32, appinfo: Option<&'a Object>) -> MappingInput<'a> {
        MappingInput {
            app: AppId(app),
            appinfo,
            user: "",
            default_tool: "proton_experimental",
        }
    }

    #[test]
    fn parses_the_list() {
        let l = list();
        assert_eq!(l.tools.len(), 6);
        let p = &l.tools[0];
        assert_eq!(p.name, "proton_experimental");
        assert_eq!(p.app, AppId(1493710));
        assert_eq!(p.depot, 1493711);
        assert_eq!(p.requires, Some(AppId(4183110)));
        assert_eq!(p.from_os, ["windows"]);
        assert!(!p.unlisted && !p.legacy);
        assert!(l.find("proton_5").unwrap().legacy);
        assert!(l.find("sniper").unwrap().unlisted);
        assert!(!l.find("native").unwrap().unlisted);
        assert_eq!(
            l.mappings,
            [
                (AppId(730), "SteamLinuxRuntime_sniper".to_owned()),
                (AppId(39210), "proton-11.0-beta".into()),
                (AppId(227000), String::new()),
            ]
        );
        assert_eq!(l.config(AppId(7670)).unwrap().config, "noesync,nofsync");
        assert_eq!(l.config(AppId(368340)).unwrap().environment, "DXVK_HDR=0");
    }

    #[test]
    fn names_ignore_case_and_keys_win() {
        let l = list();
        assert_eq!(
            l.find("SteamLinuxRuntime_sniper").unwrap().app,
            AppId(1628350)
        );
        assert_eq!(l.find("native").unwrap().app, AppId(1070560));
        assert_eq!(l.find("Proton-Stable").unwrap().app, AppId(4628710));
        assert!(l.find("").is_none());
        assert!(l.find("proton_4").is_none());
    }

    #[test]
    fn valve_mapping_beats_the_apps_own_and_native() {
        // Counter-Strike 2: mapped by 891390 at 100, by itself at 90, and
        // native at 75.
        let cs2 = info(
            r#""appinfo" { "common" { "oslist" "windows,linux" }
               "config" { "app_mappings" { "1" { "platform" "linux"
                 "tool" "SteamLinuxRuntime_sniper" "comment" "sniper SLR" } } } }"#,
        );
        let l = list();
        let m = l.resolve(input(730, Some(&cs2))).unwrap();
        assert_eq!(m.tool.name, "steamlinuxruntime_sniper");
        assert_eq!((m.source, m.priority), (Source::Valve, 100));
    }

    #[test]
    fn an_apps_own_priority_can_win() {
        // Team Fortress 2 maps itself at 101.
        let tf2 = info(
            r#""appinfo" { "common" { "oslist" "windows,macos,linux" }
               "config" { "app_mappings" { "1" { "platform" "linux"
                 "tool" "SteamLinuxRuntime_sniper" "priority" "101" } } } }"#,
        );
        let l = list();
        let m = l.resolve(input(440, Some(&tf2))).unwrap();
        assert_eq!((m.source, m.priority), (Source::App, 101));
        // Without a priority it is 90, above native.
        let dota = info(
            r#""appinfo" { "common" { "oslist" "windows,linux" }
               "config" { "app_mappings" { "1" { "platform" "linux"
                 "tool" "SteamLinuxRuntime_sniper" } } } }"#,
        );
        let m = l.resolve(input(570, Some(&dota))).unwrap();
        assert_eq!((m.source, m.priority), (Source::App, 90));
    }

    #[test]
    fn linux_apps_get_native_and_others_the_default() {
        let l = list();
        let spacewar = info(r#""appinfo" { "common" { "oslist" "windows,linux" } }"#);
        let m = l.resolve(input(480, Some(&spacewar))).unwrap();
        assert_eq!(m.tool.name, "steamlinuxruntime");
        assert_eq!(m.source, Source::Native);

        let windows = info(r#""appinfo" { "common" { "oslist" "windows" } }"#);
        let m = l.resolve(input(12345, Some(&windows))).unwrap();
        assert_eq!(m.tool.name, "proton_experimental");
        assert_eq!(m.source, Source::Default);

        let off = MappingInput {
            default_tool: "",
            ..input(12345, Some(&windows))
        };
        assert!(l.resolve(off).is_none());
        // Without the app's info it is not known to have a Linux version.
        assert_eq!(
            l.resolve(input(12345, None)).unwrap().source,
            Source::Default
        );
    }

    #[test]
    fn the_user_wins_and_unknown_or_empty_names_are_ignored() {
        let l = list();
        let windows = info(r#""appinfo" { "common" { "oslist" "windows" } }"#);
        let m = l
            .resolve(MappingInput {
                user: "proton-stable",
                ..input(39210, Some(&windows))
            })
            .unwrap();
        assert_eq!(
            (m.tool.name.as_str(), m.source),
            ("proton_11", Source::User)
        );

        // An unknown user choice falls back to Valve's mapping.
        let m = l
            .resolve(MappingInput {
                user: "proton_4",
                ..input(39210, Some(&windows))
            })
            .unwrap();
        assert_eq!(
            (m.tool.name.as_str(), m.source),
            ("proton_11", Source::Valve)
        );

        // An empty Valve mapping is no mapping.
        let m = l.resolve(input(227000, Some(&windows))).unwrap();
        assert_eq!(m.source, Source::Default);

        // Tools get no tool, whatever their info says.
        assert!(
            l.resolve(MappingInput {
                user: "proton-stable",
                ..input(1628350, Some(&windows))
            })
            .is_none()
        );
    }

    #[test]
    fn choices_follow_the_apps_platforms() {
        let l = list();
        let names =
            |p: &[&str]| -> Vec<String> { l.choices(p).iter().map(|c| c.0.to_owned()).collect() };
        assert_eq!(
            names(&["windows"]),
            [
                "proton_experimental",
                "proton_11",
                "proton_5",
                "proton-stable"
            ]
        );
        assert_eq!(names(&["linux"]), ["steamlinuxruntime"]);
    }
}
