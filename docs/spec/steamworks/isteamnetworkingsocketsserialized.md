# ISteamNetworkingSocketsSerialized

| | |
|---|---|
| Status | Partly implemented: relay certificates and the relay ticket cache. Peer-to-peer signalling, STUN servers, direct connections, FakeIP, the network config and connection reports fail truthfully (§4). |
| Requirements | SPEC §12.3, GIPC-1, FAITH-1 to FAITH-3 |
| Versions | `SteamNetworkingSocketsSerialized002` and `003` (`vtables.txt`), `005` (`vtables-observed.txt`), reached through `ISteamClient::GetISteamGenericInterface`. `001` and `004` are not served: their layouts are not known. |
| Code | `crates/ossteam-steamclient/src/networking.rs`; the daemon's side in `crates/ossteamd/src/core/networking.rs` |

Games that use Valve's networking library, `steamnetworkingsockets`,
talk to Steam through this interface: the library ships with the game,
and the Steam client provides what only an account can have. Above all
that is a certificate, which the library shows to Steam Datagram Relay
(SDR) and to peers to prove which account it is. The interface is not
in any published SDK.

**Verification.**

- **Layouts of `002` and `003`:** the generated wrappers in Proton's
  `lsteamclient`.
- **Layout of `005`:** read from the official client's `steamclient.so`
  (structure only, `research-sources.md`). Its adapter class for
  `005` has 11 virtual methods, and each forwards to a method of an
  internal interface whose name the client's IPC layer carries as a
  string.
- **Arguments and results:** from Counter-Strike 2's copy of the
  library (`libsteamnetworkingsockets.so`): how it calls each slot and
  reads each callback. It asks for `005` and falls back to `003`.
- **The certificate flow (§3):** read from the official library's code
  that fills the certificate callback.

What these sources do not show is marked *unverified*.

## 1. Slots

`005`:

| Slot | Method | Status |
|---|---|---|
| 0 | `void SendP2PRendezvous(CSteamID remote, uint32 connectionIdSrc, const void *msg, uint32 cb)` | fails truthfully (§4) |
| 1 | `void SendP2PConnectionFailure(CSteamID remote, uint32 connectionIdDest, uint32 reason, const char *message)` | fails truthfully |
| 2 | `SteamAPICall_t GetCertAsync()` | implemented (§3) |
| 3 | `int GetNetworkConfigJSON(char *buf, uint32 cb, const char *launcherPartner)` | fails truthfully; the arguments are `003`'s, *unverified* for `005` |
| 4 | `void CacheRelayTicket(const void *ticket, uint32 cb)` | implemented (§5) |
| 5 | `uint32 GetCachedRelayTicketCount()` | implemented |
| 6 | `int GetCachedRelayTicket(uint32 index, void *buf, uint32 cb)` | implemented |
| 7 | `void PostConnectionStateMsg(const void *msg, uint32 cb)` | fails truthfully |
| 8 | `bool GetSTUNServer(int index, char *buf, uint32 cb)` | fails truthfully |
| 9 | `uint32 AllowDirectConnectToPeer(const SteamNetworkingIdentity &peer)` | fails truthfully |
| 10 | `SteamAPICall_t BeginAsyncRequestFakeIP(int numPorts)` | fails truthfully |

The official adapter answers slots 3 and 7 from another object than the
rest. Slot 9 hands the internal method the peer's identity as a string
(`SteamNetworkingIdentity::ToString`, 128 bytes). Slot 9's result is a
set of bits, not a bool: Counter-Strike 2 turns bits 1, 2, 4 and 8 into
the ICE transports it may use, and 0 into relays only.

`002` and `003` have slots 0 to 7, the same methods. `002`'s
`GetNetworkConfigJSON` has no third argument.

## 2. Callbacks

| Callback | Size | Layout | Sent |
|---|---|---|---|
| `SteamNetworkingSocketsCert_t` (1296) | 792 | below | as the result of `GetCertAsync` |
| `SteamNetworkingSerializedFakeIP_t` (1294) | 140 | *unverified* | never (§4) |
| `SteamNetworkingSocketsRecvP2PFailure_t` (1297) | 148 | *unverified* past `uint64 remote` | never (§4) |
| `SteamNetworkingSocketsRecvP2PRendezvous_t` (1298) | 8208 | `uint64 remote`, `uint32 connectionId`, `uint32 cb`, `uint8 data[8192]` | never (§4) |

`SteamNetworkingSocketsCert_t`, as the official library writes it and
Counter-Strike 2 reads it (64-bit); every field falls on its natural
alignment:

| Offset | Field | Meaning |
|---|---|---|
| 0 | `int32 m_eResult` | an `EResult`; 1 when the rest holds a certificate |
| 4 | `uint32 m_cbCert` | bytes in `m_certOrMsg` |
| 8 | `uint8 m_certOrMsg[512]` | the certificate, as Steam's reply carries it (§3); on failure a NUL-terminated message |
| 520 | `uint64 m_caKeyID` | the id of the key that signed it |
| 528 | `uint32 m_cbSignature` | 64 |
| 532 | `uint8 m_signature[128]` | the signature |
| 660 | `uint32 m_cbPrivKey` | 64 |
| 664 | `uint8 m_privKey[128]` | the certificate's Ed25519 public key (32 bytes), then its private key (the 32-byte seed) |

The library checks the result: the certificate parses and holds an
Ed25519 key, the signature is 64 bytes, the private key is 64 bytes,
and the private key belongs to the certificate's public key. It
reads the private key from offset 696, the second half of
`m_privKey`. ossteam uses the same layout on 32-bit, where it needs no
padding either (*unverified* against a 32-bit game).

## 3. Certificates

A certificate belongs to one account and one app. The daemon keeps the
last one Steam gave for each pair in memory, with its key pair, and
`GetCertAsync` answers from it when it can. This is the official
client's rule, read from its library:

1. When the kept certificate has more than 4 hours left, it is the
   answer.
2. Otherwise the daemon makes a new Ed25519 key pair and sends
   `ClientNetworkingCertRequest` (5621) as a job:
   `CMsgClientNetworkingCertRequest{key_data, app_id}`, where `key_data`
   is the raw 32-byte public key and `app_id` the connection's app.
3. The reply is `ClientNetworkingCertRequestResponse` (5622),
   `CMsgClientNetworkingCertReply{cert, ca_key_id, ca_signature}`, with
   the result in its header. When it is OK, the daemon keeps the
   certificate with the key pair, and that is the answer.
4. Otherwise, with the result of the reply, or 16 (`Timeout`) when no
   reply came in 15 seconds, the kept certificate is the answer if it
   has more than a minute left. If it does not, the answer is that
   result and a message.

A certificate's time left is its `time_expiry` (field 9 of the
`CMsgSteamDatagramCertificate` that `cert` encodes) less Steam's clock.
While the account is not logged on there is nothing to send: step 4
applies with result 3 (`NoConnection`), *unverified* against the
official client. The official client stores its certificates somewhere
that may outlive it, *unverified*; ossteam's die with the daemon.

A certificate whose parts do not fit the callback (more than 512 bytes,
or a signature over 128) is not kept, and the answer is result 2
(`Fail`), as in the official library.

## 4. What fails truthfully

ossteam has not implemented these yet. The shim's stubs answer them
(`steamworks-abi.md` §2.4), and the game sees what it sees from a Steam
that cannot help:

- **Peer-to-peer signalling.** Slots 0 and 1 send nothing. Callbacks
  1297 and 1298 never come. The official client carries these through
  the CM (`ClientP2PConnectionInfo`, 5434; `ClientP2PConnectionFailInfo`,
  5435). Checking them needs a second account (`docs/status.md`).
- **STUN servers.** Slot 8 returns false: the library has no STUN
  server, and finds no address of its own. The official client's log
  names a STUN server at a Steam CM's address on port 3478, which its
  CM layer resolves (*unverified* how).
- **Direct connections.** Slot 9 returns 0: relays only. The official
  client decides from a user setting ("AllowShareIPUserSetting") that
  ossteam does not have. The player's address is not shared.
- **FakeIP.** Slot 10 returns `k_uAPICallInvalid`. The official client
  would call `GameNetworking.AllocateFakeIP`, and the result layout
  (callback 1294) is not known.
- **The network config and connection reports.** Slot 3 returns 0 and
  slot 7 sends nothing. Their sources in the official client are not
  known. Recent libraries fetch the relay config over HTTPS themselves.

## 5. Game IPC and relay tickets

| Tag | Request | Response |
|---|---|---|
| 14 | `NetworkingCert{}` | `NetworkingCert{result: u32, message: string, cert: list<u8>, ca_key_id: u64, signature: list<u8>, private_key: list<u8>}` |
| 15 | `CacheRelayTicket{ticket: list<u8>}` | `Done{}` |
| 16 | `RelayTickets{}` | `RelayTickets{tickets: list<list<u8>>}` |

The requests act on the connection's own app id, which the account must
own (`steamworks-abi.md` §3.6).

A game that is given a ticket for a relayed server, as by its Game
Coordinator, stores it with `CacheRelayTicket`. It can read it back
later, even from another process. The daemon keeps the tickets for each account and app in memory, oldest
first. It does not read them. A ticket already kept is not kept twice,
and past 64 tickets the oldest goes. A ticket that is empty, or larger
than 16 KiB, is not kept. *Unverified:* the official client's limits,
and whether it drops expired tickets.

- `GetCachedRelayTicketCount` asks for the list and returns its length;
  it returns 0 without a pipe.
- `GetCachedRelayTicket(index, buf, cb)` asks for the list and copies
  ticket `index` into `buf`, returning its size. It returns 0 when there
  is no such ticket, `buf` is null, or `cb` is too small (*unverified*).
  Counter-Strike 2 reads with a 2048-byte buffer and stops at the first
  result that is not positive.
- `CacheRelayTicket` sends without waiting.

## 6. Checks

| Item | Status |
|---|---|
| `005` slot layout | from the official library's adapter and Counter-Strike 2's calls |
| `SteamNetworkingSocketsCert_t` layout | from both sides: the official library fills it, Counter-Strike 2 reads it |
| Steam answers ossteam's `ClientNetworkingCertRequest` | done 2026-11-04 with the test account: a certificate for app 730, good for 48 hours |
| Counter-Strike 2 accepts the certificate | done 2026-11-04, in the Steam Linux Runtime 3.0, from the main menu, no match joined. It asked at start, and its console logged "Certificate expires in 47h59m … will renew in 45h59m", then "AuthStatus … OK". A second launch got the kept certificate without a new request. Its STUN server list was empty (§4). |
| Relays with the certificate | done 2026-11-04: CS2 measured its pings to 25 relays and logged "SDR RelayNetworkStatus: avail=OK". It fetches the relay config through `ISteamHTTP`; before ossteam had that interface its first attempts failed, and seconds later it logged "Got SDR network config" from a source not known. Since `ISteamHTTP` (`isteamhttp.md` §9), the config comes from ossteam's answer, and 26 relays were measured. |
| The official client's ticket limits and certificate storage | unverified |
