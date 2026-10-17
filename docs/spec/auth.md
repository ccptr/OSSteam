# Authentication

| | |
|---|---|
| Status | Draft for M1a; §1 to §4 implemented in `ossteam-auth`, not yet tried live |
| Requirements | SPEC §9.2 (AUTH-1 to AUTH-4), §8.5 (IPC-9, IPC-10), D-8, D-10, SEC-2 |
| Code | `crates/ossteam-auth`; storage (§4, §5) and accounts (§6) belong to `ossteamd` |

Signing in is done with the `Authentication` unified service
(`IAuthenticationService`). That service issues a **refresh token**, which
is long-lived and stored, and **access tokens**, which are short-lived and
derived from it. The CM logon in `cm.md` §4 then uses the refresh token.
Request and response messages come from the vendored
`steammessages_auth.steamclient.proto`.

**Verification.** As in `cm.md`, this draft comes from public community
documentation. Each statement must be confirmed against a capture of the
official client on our test account before code depends on it. §8 tracks
which statements have been confirmed. Rows marked *live* were accepted
by Valve's servers when the account-free probes ran
(`crates/ossteam-net/examples/cm_probe.rs`,
`crates/ossteam-auth/examples/qr_probe.rs`). That confirms the format,
but it is not a capture of the official client.

## 1. Transport and identity

- For the Steam client platform, the auth calls are non-authed service
  method calls on the CM connection (`cm.md` §3.5). They are not HTTPS
  WebAPI calls.
- Every begin-session request identifies the client:
  - platform type `SteamClient`;
  - persistence `Persistent`, unless the user chose `--no-remember`, in
    which case it is `Ephemeral`;
  - a website id of `Client`;
  - device details: a friendly name (the host name), the platform type, the
    OS type, and the gaming-device type.

  The exact values the official client sends are recorded from a capture
  (§8). Until then ossteam sends the platform type, persistence, website
  id and friendly name, and leaves the OS type, gaming-device type and
  machine id unset.

## 2. Credential login (AUTH-1)

1. `GetPasswordRSAPublicKey{account_name}` returns an RSA modulus and
   exponent as hex strings, and a key timestamp.
2. The password is encrypted with RSA PKCS#1 v1.5 under that key and
   base64-encoded. The password buffer is zeroed right after this step
   (IPC-10, SEC-2).
3. `BeginAuthSessionViaCredentials` is sent with the account name, the
   encrypted password, the key timestamp, the identity from §1, and any
   stored guard data for this account (§5). The reply contains:
   - a `client_id` and `request_id` for the session;
   - the polling `interval`;
   - the session's Steam id;
   - `allowed_confirmations`, a list of guard types, each with an
     optional message.
4. The daemon turns `allowed_confirmations` into IPC events:

   | Guard type | Daemon action |
   |---|---|
   | None | Nothing to ask for; go to step 5 |
   | Email code | `LoginPrompt{email_code}` with the hint (the email domain); the client answers with `SubmitGuardCode` |
   | Device code | `LoginPrompt{device_code}`; the client answers with `SubmitGuardCode` |
   | Device confirmation | `LoginPrompt{device_confirmation}`; no answer is needed, because approval happens in the Steam mobile app |
   | Email confirmation | `LoginPrompt{device_confirmation}` with the text "confirm in email" |
   | Machine token | Handled silently by the stored guard data (§5) |

   When several types are allowed, the user may satisfy any one of them.
   A code is sent with
   `UpdateAuthSessionWithSteamGuardCode{client_id, steamid, code, code_type}`.
5. The daemon calls `PollAuthSessionStatus{client_id, request_id}` every
   `interval` seconds until it returns tokens, or until the session expires
   or fails. A reply can carry a new `client_id`, which replaces the old
   one. A successful poll returns `refresh_token`, `access_token`,
   `account_name`, and possibly `new_guard_data`.
6. `CancelLogin` from IPC stops polling and drops the session.

## 3. QR login (AUTH-2)

`BeginAuthSessionViaQR` takes the identity from §1 and returns a
`challenge_url`, a `client_id`, a `request_id` and an `interval`. The
daemon emits `LoginPrompt{qr_url}` with that URL. The GUI renders it as a
QR code, and the CLI renders it with terminal block characters. Polling
works as in §2 step 5. A poll may return a new challenge URL, because the
code rotates. Each new URL is sent to clients as another
`LoginPrompt{qr_url}`. The user approves by scanning the code with the
Steam mobile app.

## 4. Tokens (AUTH-3, D-8)

- The **refresh token** is a JWT that the server signs. ossteam reads its
  payload (subject Steam id, expiry, audience) only to schedule renewal and
  to show the account. It never relies on the token's signature, never
  edits a token, and never builds one itself (FAITH-3).
- **CM logon uses the refresh token.** Community clients put the refresh
  token in the `ClientLogon` field named `access_token`. *Unverified.* If
  a capture confirms this, SPEC AUTH-3's wording ("Logon to the CM uses the
  access token") must be corrected in the same change.
- **Access tokens** are needed for some service calls and WebAPI requests.
  They are obtained with `GenerateAccessTokenForApp{refresh_token, steamid}`,
  and that same call may also renew the refresh token. The daemon renews
  ahead of expiry, never on a fixed schedule it invented.
- **Storage.** The refresh token goes in the OS keyring under the service
  name `ossteam` with the account name as the key. If no keyring is
  available, it goes in a mode-0600 file, but only after the user opts in
  (D-8). Access tokens are kept in memory only. Tokens use secret types that
  are zeroed when dropped and have no `Debug` or `Display` output for their
  content (D-10, SEC-2).
- **Revocation.** If logon with a stored refresh token fails with an
  authentication EResult, the daemon deletes the token, emits
  `SessionChanged` (logged off, with the reason), and asks the user to log
  in again. It does not retry with that token. *The exact EResults are
  unverified.*

## 5. Guard data

A successful email-guarded login can return `new_guard_data`, a machine
token. Sending it with later logins lets the account skip the email code
on this machine. It is stored like a refresh token, in the keyring under
`ossteam-guard`, and handled as a secret. It is the user's real
authorisation for this machine and is never shared between machines.

## 6. Accounts (AUTH-4)

One account is active per daemon. `ListAccounts` returns the accounts
that have a stored refresh token. `SwitchAccount` logs off, which sends
`ClientLogOff` and waits for `ClientLoggedOff` or a timeout, and then logs
on with the other account's token. `Logout` logs off and deletes that
account's stored token.

## 7. What the daemon never does

- It never stores a password (D-8) and never logs one. Codes are not
  logged either (D-10).
- It never automates Steam Guard. Generating codes as a mobile
  authenticator is a non-goal (§1.2).
- It never reuses the official client's stored tokens or files.

## 8. Verification status

| Statement | Status |
|---|---|
| Auth calls go over CM as non-authed service calls for the client platform (§1) | live 2026-10-17 (`GetPasswordRSAPublicKey`, `BeginAuthSessionViaQR`, `PollAuthSessionStatus`); capture pending |
| Identity fields and values (§1) | not captured |
| RSA scheme and encoding of the password (§2) | unverified |
| Guard types and their numbering (§2) | taken from the vendored proto; behaviour unverified |
| QR begin-session and polling (§3) | live 2026-10-17: `https://s.team/q/1/…` challenge, 5 s interval, device confirmation or device code offered, an unapproved poll is pending |
| QR URL rotation during polling (§3) | unverified (no rotation within one poll) |
| Refresh token in `ClientLogon.access_token` (§4) | unverified |
| Revocation EResults (§4) | unverified |
