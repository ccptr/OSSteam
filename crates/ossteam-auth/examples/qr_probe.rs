// SPDX-License-Identifier: GPL-2.0-or-later
//! Checks the QR begin-session request (`docs/spec/auth.md` §3) against
//! the live servers without an account: begins a session, polls once and
//! lets it expire unapproved.
//!
//! ```sh
//! cargo run -p ossteam-auth --example qr_probe
//! ```

use std::time::{Duration, Instant};

use ossteam_auth::{DeviceIdentity, Poll, session};
use ossteam_net::{CmKind, Connection, ServerPicker};
use ossteam_types::CancelToken;

fn main() -> anyhow::Result<()> {
    let servers = ossteam_net::directory::fetch(&ossteam_net::tls::http_agent(), 0)?;
    let picker = ServerPicker::new(servers);
    let server = picker
        .next(CmKind::Websockets, Instant::now())
        .map_err(|_| anyhow::anyhow!("no websocket server"))?
        .clone();
    let cancel = CancelToken::new();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let conn = Connection::connect(&server, Duration::from_secs(10), tx, &cancel)?;
    conn.hello()?;

    let identity = DeviceIdentity {
        friendly_name: "ossteam qr probe".into(),
    };
    let mut pending = session::begin_qr(&conn, &identity, &cancel)?;
    let url = pending.challenge_url.clone().unwrap_or_default();
    // Print the shape, not the URL: anyone holding it could approve it.
    let shape = url.rsplit_once('/').map_or("?", |(base, _)| base);
    println!(
        "QR session: challenge {shape}/<{} chars>, interval {:?}, guards {:?}",
        url.rsplit('/').next().unwrap_or("").len(),
        pending.interval(),
        pending.guards
    );
    std::thread::sleep(pending.interval());
    match pending.poll(&conn, &cancel)? {
        Poll::Pending { new_challenge_url } => {
            println!(
                "poll: pending, challenge rotated: {}",
                new_challenge_url.is_some()
            )
        }
        Poll::Done(_) => println!("poll: unexpectedly approved"),
    }
    conn.close();
    Ok(())
}
