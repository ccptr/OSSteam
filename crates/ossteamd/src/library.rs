// SPDX-License-Identifier: GPL-2.0-or-later
//! The Library methods (`ipc.md` §5.3): `ListApps` and `GetApp`, answered
//! from the PICS worker's last snapshot and the PICS cache, and the lookup
//! behind `GetAsset`.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{self, ErrorCode};
use ossteam_pics::cache::Kind;
use ossteam_pics::{AppInfo, AssetKind, Cache, LibraryApp};
use ossteam_types::AppId;

use crate::assets::Asset;
use crate::pics::Snapshot;

/// The language images are chosen in, until there is a language setting.
const ASSET_LANGUAGE: &str = "english";

/// Whether an app or DLC is installed: `None` when it is not, otherwise
/// whether an update is known for it.
pub type InstallState<'a> = &'a dyn Fn(&LibraryApp) -> Option<bool>;

pub fn list_apps(
    snapshot: &Snapshot,
    req: &v1::ListAppsRequest,
    install_state: InstallState,
) -> v1::ListAppsResponse {
    let filter = req.filter.trim().to_lowercase();
    let matches = |a: &LibraryApp| {
        filter.is_empty() || a.name.to_lowercase().contains(&filter) || a.id.to_string() == filter
    };
    let installed = |a: &LibraryApp| install_state(a).is_some();
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
                Some(summary(a, &all, install_state))
            } else if !dlc.is_empty() {
                Some(summary(a, &dlc, install_state))
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

/// Where one of a library app's images comes from and is cached
/// (`pics.md` §9).
pub fn asset(
    snapshot: &Snapshot,
    cache: &Cache,
    assets_dir: &Path,
    app_id: u32,
    kind: AssetKind,
    high_dpi: bool,
) -> Result<Asset, RemoteError> {
    let not_found = |m: String| RemoteError::new(ErrorCode::NotFound, m);
    let Some(app) = snapshot.library.find(AppId(app_id)) else {
        return Err(not_found(format!("app {app_id} is not in the library")));
    };
    let info = app
        .has_info
        .then(|| cache.load(Kind::App, app_id))
        .flatten()
        .ok_or_else(|| not_found(format!("the info of app {app_id} is not available")))?;
    let a = AppInfo(&info.data);
    let rel = a
        .asset_path(kind, high_dpi, ASSET_LANGUAGE)
        .ok_or_else(|| not_found(format!("app {app_id} has no {} image", kind.name())))?;
    Ok(Asset::new(
        assets_dir,
        AppId(app_id),
        a.store_asset_mtime(),
        rel,
    ))
}

pub fn get_app(
    snapshot: &Snapshot,
    cache: &Cache,
    app_id: u32,
    install_state: InstallState,
) -> Result<v1::GetAppResponse, RemoteError> {
    let Some(app) = snapshot.library.find(AppId(app_id)) else {
        let mut message = format!("app {app_id} is not in the library");
        if !snapshot.complete {
            message.push_str(", which is still loading");
        }
        return Err(RemoteError::new(ErrorCode::NotFound, message));
    };
    let mut r = v1::GetAppResponse {
        app: Some(summary(
            app,
            &app.dlc.iter().collect::<Vec<_>>(),
            install_state,
        )),
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
    // Launch options are read by ossteam-launch (`launch.md` §1).
    r.launch_options = crate::core::app_launch_options(a.0, "");
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

fn summary(a: &LibraryApp, dlc: &[&LibraryApp], install_state: InstallState) -> v1::AppSummary {
    let state = install_state(a);
    v1::AppSummary {
        app_id: a.id.0,
        name: a.name.clone(),
        r#type: a.app_type.clone(),
        parent: a.parent.map_or(0, |p| p.0),
        has_info: a.has_info,
        installed: state.is_some(),
        dlc: dlc.iter().map(|d| summary(d, &[], install_state)).collect(),
        update_available: state == Some(true),
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
            low_violence: Default::default(),
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
                &|_| None,
            ))
        };
        assert_eq!(list(""), [(10, vec![11]), (20, vec![21, 22])]);
        assert_eq!(list("half-life"), [(10, vec![11]), (20, vec![21])]);
        assert_eq!(list("22"), [(20, vec![22])]);
        assert!(list("nothing").is_empty());
    }

    #[test]
    fn installed_only_keeps_installed_apps_and_dlc() {
        let s = snapshot(vec![
            app(10, "Game", vec![app(11, "Pack", vec![])]),
            app(20, "Other", vec![]),
        ]);
        let only = |installed: &[u32]| {
            let r = list_apps(
                &s,
                &v1::ListAppsRequest {
                    filter: String::new(),
                    installed_only: true,
                },
                &|a| installed.contains(&a.id.0).then_some(false),
            );
            assert!(r.complete);
            ids(&r)
        };
        assert!(only(&[]).is_empty());
        assert_eq!(only(&[10]), [(10, vec![])]);
        // An installed DLC brings its parent along.
        assert_eq!(only(&[11]), [(10, vec![11])]);
    }

    #[test]
    fn get_app_needs_the_app_in_the_library() {
        let s = snapshot(vec![app(10, "Game", vec![])]);
        let cache = Cache::new(std::env::temp_dir().join("ossteamd-no-such-cache"));
        let e = get_app(&s, &cache, 99, &|_| None).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        // Listed but not cached: the summary only.
        let r = get_app(&s, &cache, 10, &|_| None).unwrap();
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
