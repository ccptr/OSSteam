// SPDX-License-Identifier: GPL-2.0-or-later
//! The daemon connection. A blocking thread owns the IPC client and
//! forwards updates into an async channel that a gpui task drains
//! (SPEC CONC-3).

use std::thread;

use ossteam_ipc::v1::{self, PeerKind, Topic, event};
use ossteam_ipc::{Client, ClientInfo, ConnectOptions};

pub enum Update {
    Ready {
        status: v1::GetStatusResponse,
        version: v1::GetVersionResponse,
    },
    Unreachable(String),
}

/// Connects (starting the daemon if needed) on a new thread and reports
/// what happens until the connection ends.
pub fn connect() -> async_channel::Receiver<Update> {
    let (tx, rx) = async_channel::unbounded();
    let on_spawn_error = tx.clone();
    let spawned = thread::Builder::new()
        .name("daemon-connection".into())
        .spawn(move || {
            let info = ClientInfo {
                kind: PeerKind::Gui,
                name: "ossteam-gui".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            };
            let result = Client::connect(&ConnectOptions::new(info)).and_then(|client| {
                let status = client.get_status()?;
                let version = client.get_version()?;
                client.subscribe(&[Topic::Daemon])?;
                Ok((client, status, version))
            });
            let client = match result {
                Ok((client, status, version)) => {
                    let _ = tx.send_blocking(Update::Ready { status, version });
                    client
                }
                Err(e) => {
                    let _ = tx.send_blocking(Update::Unreachable(e.to_string()));
                    return;
                }
            };
            let reason = loop {
                match client.next_event().and_then(|e| e.kind) {
                    Some(event::Kind::DaemonStopping(_)) => break "ossteamd stopped".to_owned(),
                    Some(_) => {}
                    None => break "the connection to ossteamd was lost".to_owned(),
                }
            };
            let _ = tx.send_blocking(Update::Unreachable(reason));
        });
    if let Err(e) = spawned {
        let _ = on_spawn_error.try_send(Update::Unreachable(format!(
            "cannot start the connection thread: {e}"
        )));
    }
    rx
}
