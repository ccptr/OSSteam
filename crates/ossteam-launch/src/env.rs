// SPDX-License-Identifier: GPL-2.0-or-later
//! The variables a launch adds to the daemon's environment
//! (`launch-env.md` §1, LAUNCH-2).

use ossteam_types::AppId;

/// The variables to set, in a fixed order. They replace variables of the
/// same name; the rest of the daemon's environment is inherited.
pub fn launch_vars(app: AppId, account_name: &str) -> Vec<(&'static str, String)> {
    // An app launched from the library has a game id equal to its app id
    // (`launch.md` §6).
    let game_id = u64::from(app.0).to_string();
    vec![
        ("SteamAppId", app.0.to_string()),
        ("SteamGameId", game_id.clone()),
        ("SteamOverlayGameId", game_id),
        ("SteamClientLaunch", "1".into()),
        ("SteamEnv", "1".into()),
        ("SteamUser", account_name.into()),
        ("SteamAppUser", account_name.into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables() {
        let v = launch_vars(AppId(480), "tester");
        assert_eq!(
            v,
            [
                ("SteamAppId", "480".to_owned()),
                ("SteamGameId", "480".into()),
                ("SteamOverlayGameId", "480".into()),
                ("SteamClientLaunch", "1".into()),
                ("SteamEnv", "1".into()),
                ("SteamUser", "tester".into()),
                ("SteamAppUser", "tester".into()),
            ]
        );
    }
}
