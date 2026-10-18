// SPDX-License-Identifier: GPL-2.0-or-later
//! The callback structures the shim queues (`steamworks-abi.md` §2.5,
//! §3.5), laid out with the packing of §2.2.

/// `SteamServersConnected_t`.
pub const SERVERS_CONNECTED: i32 = 101;
/// `SteamServersDisconnected_t`.
pub const SERVERS_DISCONNECTED: i32 = 103;
/// `SteamAPICallCompleted_t`.
pub const API_CALL_COMPLETED: i32 = 703;

/// `k_EResultNoConnection`.
pub const E_RESULT_NO_CONNECTION: i32 = 3;

/// `SteamServersConnected_t` has no fields; as a C++ struct it is one
/// byte.
pub fn servers_connected() -> Vec<u8> {
    vec![0]
}

/// `SteamServersDisconnected_t { EResult m_eResult; }`.
pub fn servers_disconnected(result: i32) -> Vec<u8> {
    result.to_le_bytes().to_vec()
}

/// `SteamAPICallCompleted_t { SteamAPICall_t m_hAsyncCall; int
/// m_iCallback; uint32 m_cubParam; }`: 16 bytes under either packing.
pub fn api_call_completed(call: u64, callback: i32, size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&call.to_le_bytes());
    out.extend_from_slice(&callback.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out
}

/// `UserStatsReceived_t` (`isteamuserstats.md` §3.2).
pub const USER_STATS_RECEIVED: i32 = 1101;
/// `UserStatsStored_t`.
pub const USER_STATS_STORED: i32 = 1102;
/// `UserAchievementStored_t`.
pub const USER_ACHIEVEMENT_STORED: i32 = 1103;
/// `UserAchievementIconFetched_t`.
pub const USER_ACHIEVEMENT_ICON_FETCHED: i32 = 1109;
/// `GlobalAchievementPercentagesReady_t`.
pub const GLOBAL_ACHIEVEMENT_PERCENTAGES_READY: i32 = 1110;

/// The callback packing (§2.2): 8 on Windows, 4 elsewhere. Pads `out` to
/// a multiple of `align`, capped at the packing.
fn pad(out: &mut Vec<u8>, align: usize) {
    let pack = if cfg!(windows) { 8 } else { 4 };
    let a = align.min(pack);
    out.resize(out.len().div_ceil(a) * a, 0);
}

/// `{ uint64 m_nGameID; EResult m_eResult; }`: `UserStatsStored_t`,
/// `GlobalAchievementPercentagesReady_t`, and `UserStatsReceived_t` of
/// version 003.
pub fn game_result(game: u64, result: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&game.to_le_bytes());
    out.extend_from_slice(&result.to_le_bytes());
    pad(&mut out, 8);
    out
}

/// `UserStatsReceived_t { uint64 m_nGameID; EResult m_eResult; CSteamID
/// m_steamIDUser; }`.
pub fn user_stats_received(game: u64, result: i32, user: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&game.to_le_bytes());
    out.extend_from_slice(&result.to_le_bytes());
    pad(&mut out, 8);
    out.extend_from_slice(&user.to_le_bytes());
    out
}

/// `char[128]`, NUL-terminated, truncated to fit.
fn name128(out: &mut Vec<u8>, name: &str) {
    let mut b = [0u8; 128];
    let n = name.len().min(127);
    b[..n].copy_from_slice(&name.as_bytes()[..n]);
    out.extend_from_slice(&b);
}

/// `UserAchievementStored_t { uint64 m_nGameID; bool
/// m_bGroupAchievement; char m_rgchAchievementName[128]; uint32
/// m_nCurProgress; uint32 m_nMaxProgress; }`.
pub fn user_achievement_stored(game: u64, name: &str, cur: u32, max: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(152);
    out.extend_from_slice(&game.to_le_bytes());
    out.push(0);
    name128(&mut out, name);
    pad(&mut out, 4);
    out.extend_from_slice(&cur.to_le_bytes());
    out.extend_from_slice(&max.to_le_bytes());
    pad(&mut out, 8);
    out
}

/// `UserAchievementIconFetched_t { CGameID m_nGameID; char
/// m_rgchAchievementName[128]; bool m_bAchieved; int m_nIconHandle; }`.
pub fn user_achievement_icon_fetched(game: u64, name: &str, achieved: bool, icon: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(144);
    out.extend_from_slice(&game.to_le_bytes());
    name128(&mut out, name);
    out.push(u8::from(achieved));
    pad(&mut out, 4);
    out.extend_from_slice(&icon.to_le_bytes());
    pad(&mut out, 8);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes of `isteamuserstats.md` §3.2.
    #[test]
    fn stats_callbacks_have_the_sdk_sizes() {
        let (received, short, stored, icon) = if cfg!(windows) {
            (24, 16, 152, 144)
        } else {
            (20, 12, 148, 144)
        };
        assert_eq!(user_stats_received(1, 1, 2).len(), received);
        assert_eq!(game_result(1, 1).len(), short);
        assert_eq!(user_achievement_stored(1, "a", 1, 2).len(), stored);
        assert_eq!(user_achievement_icon_fetched(1, "a", true, 3).len(), icon);
        // A name too long is cut, with its NUL.
        let long = "x".repeat(200);
        let b = user_achievement_stored(1, &long, 0, 0);
        assert_eq!(b[9 + 126], b'x');
        assert_eq!(b[9 + 127], 0);
    }
}
