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
