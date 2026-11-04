// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of the Game Coordinator
//! (`docs/spec/steamworks/isteamgamecoordinator.md` §1, §2): games'
//! messages go to Steam in `ClientToGC`, and each `ClientFromGC` goes to
//! the connections of its app that talk to the GC.

use ossteam_gameipc::{GcMessage, GcSendRequest, GcSent, Message, NoticeBody, ResponseBody};
use ossteam_net::Packet;
use ossteam_proto::steam::{CMsgGcClient, EMsg};
use ossteam_types::AppId;

use super::Core;
use super::shim::error;
use crate::gameipc::ConnId;

/// `GcSent.result`.
const SENT: u32 = 0;
const NOT_LOGGED_ON: u32 = 3;

/// How many bytes of a message the debug log shows.
const LOGGED_BYTES: usize = 16;

impl Core {
    /// `GcSend` (§2): to the GC of the connection's own app, which the
    /// account owns.
    pub(super) fn gc_send(&mut self, conn: ConnId, r: GcSendRequest) -> ResponseBody {
        let Some(app) = self.shim_app_of(conn) else {
            return error("the connection is closing");
        };
        if let Err(e) = self.shim_owner(app) {
            return e;
        }
        let first = self.shim_talks_to_gc(conn);
        let Some(requester) = self
            .content
            .requester
            .clone()
            .filter(|_| self.session_state.logged_on)
        else {
            return ResponseBody::GcSent(GcSent {
                result: NOT_LOGGED_ON,
            });
        };
        // The first message's bytes show its layout (§1, §4).
        let start = first.then(|| &r.payload[..r.payload.len().min(LOGGED_BYTES)]);
        tracing::debug!(
            conn,
            app = %app,
            msg_type = format_args!("{:#010x}", r.msg_type),
            size = r.payload.len(),
            ?start,
            "a message to the game coordinator"
        );
        let body = CMsgGcClient {
            appid: Some(app.0),
            msgtype: Some(r.msg_type),
            payload: Some(r.payload),
            ..Default::default()
        };
        let mut packet = Packet::proto(EMsg::KEMsgClientToGc, &body);
        if let Some(h) = packet.proto_header_mut() {
            h.routing_appid = Some(app.0);
        }
        match requester.send(packet) {
            Ok(()) => ResponseBody::GcSent(GcSent { result: SENT }),
            Err(e) => {
                tracing::info!(conn, app = %app, "could not send a message to the game coordinator: {e}");
                ResponseBody::GcSent(GcSent {
                    result: NOT_LOGGED_ON,
                })
            }
        }
    }

    /// `ClientFromGC` (§2).
    pub(super) fn on_gc_message(&mut self, p: &Packet) {
        let Ok(m) = p.decode_body::<CMsgGcClient>() else {
            tracing::debug!("dropping an undecodable ClientFromGC");
            return;
        };
        let app = AppId(m.appid.unwrap_or(0));
        let msg_type = m.msgtype.unwrap_or(0);
        let payload = m.payload.unwrap_or_default();
        let conns = self.shim_gc_conns(app);
        if conns.is_empty() {
            tracing::debug!(app = %app, msg_type = format_args!("{msg_type:#010x}"), "no game talks to this game coordinator; dropping its message");
            return;
        }
        tracing::debug!(
            app = %app,
            msg_type = format_args!("{msg_type:#010x}"),
            size = payload.len(),
            ?conns,
            "a message from the game coordinator"
        );
        for conn in conns {
            let notice = NoticeBody::GcMessage(GcMessage {
                msg_type,
                payload: payload.clone(),
            });
            self.shim_send(conn, Message::Notice(notice));
        }
    }
}
