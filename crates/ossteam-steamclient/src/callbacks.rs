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
/// m_steamIDUser; }`. `CSteamID` is aligned to 1 (§2.2), so it follows
/// the result at 12 everywhere; the `uint64` pads the end to 24 on
/// Windows.
pub fn user_stats_received(game: u64, result: i32, user: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&game.to_le_bytes());
    out.extend_from_slice(&result.to_le_bytes());
    out.extend_from_slice(&user.to_le_bytes());
    pad(&mut out, 8);
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

/// `ValidateAuthTicketResponse_t` (`isteamuser.md` §4.1).
pub const VALIDATE_AUTH_TICKET_RESPONSE: i32 = 143;
/// `EncryptedAppTicketResponse_t { EResult m_eResult; }`.
pub const ENCRYPTED_APP_TICKET_RESPONSE: i32 = 154;
/// `GetAuthSessionTicketResponse_t`.
pub const GET_AUTH_SESSION_TICKET_RESPONSE: i32 = 163;
/// `GetTicketForWebApiResponse_t`.
pub const GET_TICKET_FOR_WEB_API_RESPONSE: i32 = 168;
/// `m_rgubTicket` of `GetTicketForWebApiResponse_t`.
const WEB_API_TICKET: usize = 2560;

/// `{ CSteamID m_SteamID; EAuthSessionResponse m_eAuthSessionResponse;
/// CSteamID m_OwnerSteamID; }`: 20 bytes everywhere, as `CSteamID` is
/// aligned to 1 (§2.2).
pub fn validate_auth_ticket_response(steam_id: u64, response: u32, owner: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(&steam_id.to_le_bytes());
    out.extend_from_slice(&response.to_le_bytes());
    out.extend_from_slice(&owner.to_le_bytes());
    out
}

/// `{ HAuthTicket m_hAuthTicket; EResult m_eResult; }`.
pub fn auth_session_ticket_response(handle: u32, result: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&handle.to_le_bytes());
    out.extend_from_slice(&result.to_le_bytes());
    out
}

/// `{ HAuthTicket m_hAuthTicket; EResult m_eResult; int m_cubTicket;
/// uint8 m_rgubTicket[2560]; }`. A longer ticket is cut.
pub fn ticket_for_web_api_response(handle: u32, result: u32, ticket: &[u8]) -> Vec<u8> {
    let n = ticket.len().min(WEB_API_TICKET);
    let mut out = Vec::with_capacity(12 + WEB_API_TICKET);
    out.extend_from_slice(&handle.to_le_bytes());
    out.extend_from_slice(&result.to_le_bytes());
    out.extend_from_slice(&(n as i32).to_le_bytes());
    out.extend_from_slice(&ticket[..n]);
    out.resize(12 + WEB_API_TICKET, 0);
    out
}

/// `RemoteStorageFileWriteAsyncComplete_t { EResult m_eResult; }`
/// (`isteamremotestorage.md` §2).
pub const FILE_WRITE_ASYNC_COMPLETE: i32 = 1331;
/// `RemoteStorageFileReadAsyncComplete_t`.
pub const FILE_READ_ASYNC_COMPLETE: i32 = 1332;

pub fn file_write_async_complete(result: i32) -> Vec<u8> {
    result.to_le_bytes().to_vec()
}

/// `RemoteStorageFileReadAsyncComplete_t { SteamAPICall_t
/// m_hFileReadAsync; EResult m_eResult; uint32 m_nOffset; uint32
/// m_cubRead; }`.
pub fn file_read_async_complete(call: u64, result: i32, offset: u32, read: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&call.to_le_bytes());
    out.extend_from_slice(&result.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&read.to_le_bytes());
    pad(&mut out, 8);
    out
}

/// `GCMessageAvailable_t { uint32 m_nMessageSize; }`
/// (`isteamgamecoordinator.md` §3).
pub const GC_MESSAGE_AVAILABLE: i32 = 1701;
/// `GCMessageFailed_t` has no fields; as a C++ struct it is one byte.
pub const GC_MESSAGE_FAILED: i32 = 1702;

pub fn gc_message_available(size: u32) -> Vec<u8> {
    size.to_le_bytes().to_vec()
}

pub fn gc_message_failed() -> Vec<u8> {
    vec![0]
}

/// `SteamNetworkingSocketsCert_t` (`isteamnetworkingsocketsserialized.md`
/// §2): the result of `GetCertAsync`.
pub const NETWORKING_CERT: i32 = 1296;
/// Its size: every field falls on its natural alignment.
pub const NETWORKING_CERT_SIZE: usize = 792;

/// `SteamNetworkingSocketsCert_t`. On failure `cert_or_message` is a
/// message, written with a NUL. Parts too large for their fields have
/// been refused by the daemon (§3) and are cut here only as a last guard.
pub fn networking_cert(
    result: i32,
    cert_or_message: &[u8],
    ca_key_id: u64,
    signature: &[u8],
    private_key: &[u8],
) -> Vec<u8> {
    fn field(out: &mut [u8], at: usize, cap: usize, bytes: &[u8]) -> u32 {
        let n = bytes.len().min(cap);
        out[at..at + n].copy_from_slice(&bytes[..n]);
        n as u32
    }
    let mut out = vec![0u8; NETWORKING_CERT_SIZE];
    out[0..4].copy_from_slice(&result.to_le_bytes());
    let n = if result == 1 {
        field(&mut out, 8, 512, cert_or_message)
    } else {
        // Leave room for the NUL.
        field(&mut out, 8, 511, cert_or_message);
        0
    };
    out[4..8].copy_from_slice(&n.to_le_bytes());
    out[520..528].copy_from_slice(&ca_key_id.to_le_bytes());
    let n = field(&mut out, 532, 128, signature);
    out[528..532].copy_from_slice(&n.to_le_bytes());
    let n = field(&mut out, 664, 128, private_key);
    out[660..664].copy_from_slice(&n.to_le_bytes());
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
        assert_eq!(file_write_async_complete(1).len(), 4);
        assert_eq!(validate_auth_ticket_response(1, 0, 2).len(), 20);
        assert_eq!(auth_session_ticket_response(1, 1).len(), 8);
        assert_eq!(ticket_for_web_api_response(1, 1, &[1; 300]).len(), 2572);
        let read = if cfg!(windows) { 24 } else { 20 };
        assert_eq!(file_read_async_complete(1, 1, 0, 0).len(), read);
        // `CSteamID` follows the result directly (§2.2), on Windows too.
        assert_eq!(user_stats_received(1, 1, 2)[12..20], 2u64.to_le_bytes());
        assert_eq!(
            validate_auth_ticket_response(1, 0, 2)[12..20],
            2u64.to_le_bytes()
        );
        // A name too long is cut, with its NUL.
        let long = "x".repeat(200);
        let b = user_achievement_stored(1, &long, 0, 0);
        assert_eq!(b[9 + 126], b'x');
        assert_eq!(b[9 + 127], 0);
    }
}
