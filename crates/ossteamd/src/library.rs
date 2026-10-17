// SPDX-License-Identifier: GPL-2.0-or-later
//! The Library methods (`ipc.md` §5.3): `ListApps` and `GetApp`, answered
//! from the PICS worker's last snapshot and the PICS cache.

use std::collections::{BTreeSet, HashMap};

use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{self, ErrorCode};
use ossteam_pics::cache::Kind;
use ossteam_pics::{AppInfo, Cache, LibraryApp};
use ossteam_types::AppId;

use crate::pics::Snapshot;

pub fn list_apps(snapshot: &Snapshot, req: &v1::ListAppsRequest) -> v1::ListAppsResponse {
    let filter = req.filter.trim().to_lowercase();
    let matches = |a: &LibraryApp| {
        filter.is_empty() || a.name.to_lowercase().contains(&filter) || a.id.to_string() == filter
    };
    // Nothing is installed until the install engine exists (M1b), which
    // is the true state.
    let installed = |_: &LibraryApp| false;
    let keep = |a: &LibraryApp| matches(a) && (!req.installed_only || installed(a));

    let apps = snapshot
        .library
        .apps
        .iter()
        .filter_map(|a| {
            let dlc: Vec<&LibraryApp> = a.dlc.iter().filter(|d| keep(d)).collect();
            if keep(a) {
                // A matching app keeps its DLC unless only installed ones
                // were asked for.
                let all: Vec<&LibraryApp> = if req.installed_only {
                    dlc
                } else {
                    a.dlc.iter().collect()
                };
                Some(summary(a, &all))
            } else if !dlc.is_empty() {
                Some(summary(a, &dlc))
            } else {
                None
            }
        })
        .collect();
    v1::ListAppsResponse {
        apps,
        complete: snapshot.complete,
        packages_without_info: snapshot
            .library
            .packages_without_info
            .iter()
            .map(|p| p.0)
            .collect(),
    }
}

pub fn get_app(
    snapshot: &Snapshot,
    cache: &Cache,
    app_id: u32,
) -> Result<v1::GetAppResponse, RemoteError> {
    let Some(app) = snapshot.library.find(AppId(app_id)) else {
        let mut message = format!("app {app_id} is not in the library");
        if !snapshot.complete {
            message.push_str(", which is still loading");
        }
        return Err(RemoteError::new(ErrorCode::NotFound, message));
    };
    let mut r = v1::GetAppResponse {
        app: Some(summary(app, &app.dlc.iter().collect::<Vec<_>>())),
        ..Default::default()
    };
    if !app.has_info {
        return Ok(r);
    }
    let Some(info) = cache.load(Kind::App, app_id) else {
        // Deleted since the snapshot was made; the next update fetches it.
        if let Some(a) = &mut r.app {
            a.has_info = false;
        }
        return Ok(r);
    };
    let a = AppInfo(&info.data);
    r.change_number = info.change_number;
    r.missing_token = info.missing_token;
    r.oslist = a.oslist();
    r.install_dir = a.install_dir().to_owned();
    r.depots = a
        .depots()
        .into_iter()
        .map(|d| v1::Depot {
            depot_id: d.id.0,
            name: d.name,
            oslist: d.oslist,
            osarch: d.osarch,
            language: d.language,
            dlc_app_id: d.dlc_app_id.map_or(0, |a| a.0),
            from_app_id: d.from_app.map_or(0, |a| a.0),
            shared_install: d.shared_install,
            max_size: d.max_size.unwrap_or(0),
        })
        .collect();
    r.branches = a
        .branches()
        .into_iter()
        .map(|b| v1::Branch {
            name: b.name,
            build_id: b.build_id.unwrap_or(0),
            description: b.description,
            password_required: b.password_required,
            time_updated: b.time_updated.unwrap_or(0),
        })
        .collect();
    r.launch_options = a
        .launch_options()
        .into_iter()
        .map(|l| v1::LaunchOption {
            index: l.index,
            executable: l.executable,
            arguments: l.arguments,
            description: l.description,
            r#type: l.kind,
            oslist: l.oslist,
            osarch: l.osarch,
            betakey: l.betakey,
        })
        .collect();
    Ok(r)
}

/// The apps that differ between two libraries, including those added or
/// removed. A DLC's entry is compared on its own, not through its parent.
pub fn changed_apps(old: Option<&Snapshot>, new: Option<&Snapshot>) -> BTreeSet<AppId> {
    let flat = |s: Option<&Snapshot>| -> HashMap<AppId, LibraryApp> {
        s.map(|s| {
            s.library
                .iter()
                .map(|a| {
                    let own = LibraryApp {
                        dlc: Vec::new(),
                        ..a.clone()
                    };
                    (a.id, own)
                })
                .collect()
        })
        .unwrap_or_default()
    };
    let (old, new) = (flat(old), flat(new));
    let added_or_changed = new.iter().filter(|(id, a)| old.get(id) != Some(a));
    let removed = old.iter().filter(|(id, _)| !new.contains_key(id));
    added_or_changed.chain(removed).map(|(&id, _)| id).collect()
}

fn summary(a: &LibraryApp, dlc: &[&LibraryApp]) -> v1::AppSummary {
    v1::AppSummary {
        app_id: a.id.0,
        name: a.name.clone(),
        r#type: a.app_type.clone(),
        parent: a.parent.map_or(0, |p| p.0),
        has_info: a.has_info,
        installed: false,
        dlc: dlc.iter().map(|d| summary(d, &[])).collect(),
    }
}

#[cfg(test)]
mod tests {
    use ossteam_pics::Library;
    use ossteam_types::SteamId;

    use super::*;

    fn app(id: u32, name: &str, dlc: Vec<LibraryApp>) -> LibraryApp {
        LibraryApp {
            id: AppId(id),
            name: name.into(),
            app_type: if dlc.is_empty() { "DLC" } else { "Game" }.into(),
            parent: None,
            has_info: true,
            dlc,
        }
    }

    fn snapshot(apps: Vec<LibraryApp>) -> Snapshot {
        Snapshot {
            steam_id: SteamId(1),
            library: Library {
                apps,
                packages_without_info: Vec::new(),
            },
            complete: true,
        }
    }

    fn ids(r: &v1::ListAppsResponse) -> Vec<(u32, Vec<u32>)> {
        r.apps
            .iter()
            .map(|a| (a.app_id, a.dlc.iter().map(|d| d.app_id).collect()))
            .collect()
    }

    #[test]
    fn filters_by_name_or_id_and_keeps_matching_dlc() {
        let s = snapshot(vec![
            app(10, "Half-Life", vec![app(11, "Soundtrack", vec![])]),
            app(
                20,
                "Portal",
                vec![app(21, "Half-Life Hats", vec![]), app(22, "Other", vec![])],
            ),
        ]);
        let list = |filter: &str| {
            ids(&list_apps(
                &s,
                &v1::ListAppsRequest {
                    filter: filter.into(),
                    installed_only: false,
                },
            ))
        };
        assert_eq!(list(""), [(10, vec![11]), (20, vec![21, 22])]);
        assert_eq!(list("half-life"), [(10, vec![11]), (20, vec![21])]);
        assert_eq!(list("22"), [(20, vec![22])]);
        assert!(list("nothing").is_empty());
    }

    #[test]
    fn nothing_is_installed_yet() {
        let s = snapshot(vec![app(10, "Game", vec![app(11, "Pack", vec![])])]);
        let r = list_apps(
            &s,
            &v1::ListAppsRequest {
                filter: String::new(),
                installed_only: true,
            },
        );
        assert!(r.apps.is_empty());
        assert!(r.complete);
    }

    #[test]
    fn get_app_needs_the_app_in_the_library() {
        let s = snapshot(vec![app(10, "Game", vec![])]);
        let cache = Cache::new(std::env::temp_dir().join("ossteamd-no-such-cache"));
        let e = get_app(&s, &cache, 99).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        // Listed but not cached: the summary only.
        let r = get_app(&s, &cache, 10).unwrap();
        assert!(!r.app.unwrap().has_info);
    }

    #[test]
    fn changes_cover_added_removed_and_renamed_apps() {
        let a = snapshot(vec![
            app(10, "Game", vec![app(11, "Pack", vec![])]),
            app(20, "Other", vec![]),
        ]);
        let b = snapshot(vec![
            app(10, "Game", vec![app(11, "Pack 2", vec![])]),
            app(30, "New", vec![]),
        ]);
        let ids: Vec<u32> = changed_apps(Some(&a), Some(&b))
            .into_iter()
            .map(|a| a.0)
            .collect();
        assert_eq!(ids, [11, 20, 30]);
        assert_eq!(changed_apps(None, Some(&a)).len(), 3);
        assert!(changed_apps(Some(&a), Some(&a)).is_empty());
    }
}
