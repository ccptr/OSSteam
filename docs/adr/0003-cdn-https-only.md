# ADR 0003: Content is downloaded over HTTPS only

| | |
|---|---|
| Status | Accepted |
| Date | 2026-10-17 |
| Requirements | SEC-3, CONT-1, CONT-3, §19.3, §20 (download throughput) |
| Code | `crates/ossteam-content/src/cdn.rs` |

## Context

The official client downloads manifests and chunks from the servers that
`GetServersForSteamPipe` lists (`content.md` §1), and it uses plain HTTP
for many of them, including ISP caches (`SteamCache`) that may not offer
HTTPS at all. Integrity does not depend on the transport: chunks are
encrypted with the depot key and checked against their checksums and ids
(`content.md` §5), and a manifest is only useful with its request code.

§19.3 says a deviation from the official client on the wire is a bug
unless an ADR justifies it. SEC-3 says every HTTPS connection uses
rustls with the platform verifier, but does not say whether plain HTTP
is allowed.

## Decision

ossteam uses HTTPS for every content request. Servers that do not offer
HTTPS are left out of the rotation. If none is left for an app, the job
fails with a network error that says so; there is no fallback to HTTP
and no setting that enables one.

## Consequences

- An observer on the network cannot see which depots and chunks a user
  downloads, which the official client reveals in its HTTP request lines.
  That is the privacy reason for this choice; integrity alone would not
  need it.
- Users whose ISP provides a Steam cache that only speaks HTTP do not
  benefit from it. Throughput is then that of Valve's CDN, which is
  expected to meet the §20 budget on its own; the M1b acceptance run
  measures it.
- The request pattern differs from the official client's in scheme and
  possibly in which servers are used. The acceptance run compares the
  server choice; if Valve's servers treat HTTPS clients differently, this
  ADR is revisited.
- No code path for plain HTTP exists, so there is nothing to misconfigure.
