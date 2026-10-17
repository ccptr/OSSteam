// SPDX-License-Identifier: GPL-2.0-or-later
//! Checks the framing statements of `docs/spec/cm.md` against the live
//! Steam network, without an account: directory lookup, WSS connect,
//! `ClientHello`, and one non-authed service call. Prints what the server
//! sends.
//!
//! ```sh
//! cargo run -p ossteam-net --example cm_probe
//! ```

use std::time::Duration;

use ossteam_net::{CmKind, Connection, Incoming, ServerPicker};
use ossteam_proto::steam::{CAuthenticationGetPasswordRsaPublicKeyRequest, authentication};
use ossteam_types::CancelToken;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ossteam_net=debug".into()),
        )
        .init();

    let agent = ossteam_net::tls::http_agent();
    let servers = ossteam_net::directory::fetch(&agent, 0)?;
    let ws = servers
        .iter()
        .filter(|s| s.kind == CmKind::Websockets)
        .count();
    println!("directory: {} servers ({ws} websockets)", servers.len());
    for s in servers.iter().take(3) {
        println!("  {:?} {} {}", s.kind, s.endpoint, s.dc);
    }

    let picker = ServerPicker::new(servers);
    let server = picker
        .next(CmKind::Websockets, std::time::Instant::now())
        .map_err(|_| anyhow::anyhow!("no websocket server"))?
        .clone();
    let cancel = CancelToken::new();
    let (tx, rx) = crossbeam_channel::unbounded();
    let conn = Connection::connect(&server, Duration::from_secs(10), tx, &cancel)?;
    println!("connected to {}", server.endpoint);
    conn.hello()?;

    let reply = conn.call::<authentication::GetPasswordRsaPublicKey>(
        &CAuthenticationGetPasswordRsaPublicKeyRequest {
            account_name: Some("ossteam-cm-probe".into()),
        },
        Duration::from_secs(10),
        &cancel,
    );
    match reply {
        Ok(r) => println!(
            "GetPasswordRSAPublicKey: modulus {} hex digits, exponent {:?}, timestamp {:?}",
            r.publickey_mod().len(),
            r.publickey_exp(),
            r.timestamp()
        ),
        Err(e) => println!("GetPasswordRSAPublicKey failed: {e}"),
    }

    let until = std::time::Instant::now() + Duration::from_secs(5);
    while let Ok(m) = rx.recv_deadline(until) {
        match m {
            Incoming::Message(p) => {
                println!("unsolicited: {:?} ({} body bytes)", p.kind(), p.body.len())
            }
            Incoming::Closed(r) => {
                println!("closed: {r}");
                return Ok(());
            }
        }
    }
    conn.close();
    println!("closed cleanly");
    Ok(())
}
