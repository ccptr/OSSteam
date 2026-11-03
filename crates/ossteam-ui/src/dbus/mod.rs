// SPDX-License-Identifier: GPL-2.0-or-later
//! A minimal, synchronous D-Bus client (ADR 0004), enough for the desktop
//! portal: connect to the session bus, log in with `EXTERNAL`, call
//! methods, add match rules and wait for signals. Everything blocks the
//! thread that uses it; a thread waiting for a signal sleeps in `read`
//! and does not wake until a message comes (GUI-5).
//!
//! Not here, because the portal needs none of it: file descriptors,
//! serving objects (calls to us are answered with an error, `Ping`
//! excepted), and buses other than local sockets.

mod wire;

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;

#[cfg(test)]
pub use wire::Type;
pub use wire::{Kind, Message, Value, WireError};

const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";
pub const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";

#[derive(Debug)]
pub enum Error {
    /// No session bus address we can connect to.
    NoBus(String),
    Io(io::Error),
    /// The bus refused our login.
    Auth(String),
    Wire(WireError),
    /// An error reply: its name and message.
    Remote(String, String),
    /// A reply whose body is not what the method returns.
    Unexpected(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoBus(why) => write!(f, "no session bus: {why}"),
            Error::Io(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                f.write_str("the session bus closed the connection")
            }
            Error::Io(e) => write!(f, "the session bus: {e}"),
            Error::Auth(line) => write!(f, "the session bus refused the login: {line}"),
            Error::Wire(e) => e.fmt(f),
            Error::Remote(name, message) if message.is_empty() => f.write_str(name),
            Error::Remote(name, message) => write!(f, "{name}: {message}"),
            Error::Unexpected(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<WireError> for Error {
    fn from(e: WireError) -> Self {
        Error::Wire(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A connection to a bus, logged in and with its unique name.
pub struct Connection {
    stream: UnixStream,
    last_serial: u32,
    unique_name: String,
    /// Signals that came while a call waited for its reply.
    signals: VecDeque<Message>,
}

impl Connection {
    /// A new connection to the session bus.
    pub fn session() -> Result<Self> {
        Self::on(session_bus_socket()?)
    }

    /// Logs in on a connected bus socket and says `Hello`.
    pub fn on(mut stream: UnixStream) -> Result<Self> {
        log_in(&mut stream)?;
        let mut c = Connection {
            stream,
            last_serial: 0,
            unique_name: String::new(),
            signals: VecDeque::new(),
        };
        let reply = c.call(Message::call(BUS, BUS_PATH, BUS, "Hello", Vec::new()))?;
        match reply.body.first() {
            Some(Value::Str(name)) => c.unique_name.clone_from(name),
            _ => return Err(Error::Unexpected("the bus's Hello did not answer a name")),
        }
        Ok(c)
    }

    /// The name the bus gave this connection, such as `:1.42`.
    pub fn unique_name(&self) -> &str {
        &self.unique_name
    }

    /// Calls a method and waits for its reply, which is returned; an
    /// error reply is [`Error::Remote`].
    pub fn call(&mut self, call: Message) -> Result<Message> {
        let serial = self.send(call)?;
        loop {
            let m = self.receive()?;
            match m.kind {
                Kind::Return | Kind::Error if m.reply_serial == Some(serial) => {
                    return if m.kind == Kind::Return {
                        Ok(m)
                    } else {
                        let name = m.error_name.unwrap_or_default();
                        let message = match m.body.first() {
                            Some(Value::Str(s)) => s.clone(),
                            _ => String::new(),
                        };
                        Err(Error::Remote(name, message))
                    };
                }
                Kind::Signal => self.signals.push_back(m),
                _ => {}
            }
        }
    }

    /// Asks the bus to send this connection the signals `rule` matches.
    pub fn add_match(&mut self, rule: &str) -> Result<()> {
        let call = Message::call(
            BUS,
            BUS_PATH,
            BUS,
            "AddMatch",
            vec![Value::Str(rule.into())],
        );
        self.call(call).map(drop)
    }

    /// The next signal, waiting as long as it takes.
    pub fn next_signal(&mut self) -> Result<Message> {
        if let Some(m) = self.signals.pop_front() {
            return Ok(m);
        }
        loop {
            let m = self.receive()?;
            if m.kind == Kind::Signal {
                return Ok(m);
            }
        }
    }

    fn send(&mut self, mut m: Message) -> Result<u32> {
        self.last_serial = self.last_serial.checked_add(1).unwrap_or(1);
        m.serial = self.last_serial;
        self.stream.write_all(&m.encode())?;
        Ok(m.serial)
    }

    /// The next message from the bus. Method calls to us are answered
    /// here and not returned.
    fn receive(&mut self) -> Result<Message> {
        loop {
            let m = read_message(&mut self.stream)?;
            match m.kind {
                Kind::Call => self.answer(&m)?,
                Kind::Other(_) => {}
                _ => return Ok(m),
            }
        }
    }

    /// `org.freedesktop.DBus.Peer.Ping` gets its empty reply; anything
    /// else that wants a reply gets `UnknownMethod`.
    fn answer(&mut self, call: &Message) -> Result<()> {
        if call.flags & wire::NO_REPLY_EXPECTED != 0 {
            return Ok(());
        }
        let ping = call.interface.as_deref() == Some("org.freedesktop.DBus.Peer")
            && call.member.as_deref() == Some("Ping");
        let reply = Message {
            kind: if ping { Kind::Return } else { Kind::Error },
            error_name: (!ping).then(|| UNKNOWN_METHOD.into()),
            reply_serial: Some(call.serial),
            destination: call.sender.clone(),
            ..Message::default()
        };
        self.send(reply).map(drop)
    }
}

fn read_message(stream: &mut impl Read) -> Result<Message> {
    let mut start = [0; 16];
    stream.read_exact(&mut start)?;
    let mut buf = vec![0; Message::len_from_start(&start)?];
    buf[..16].copy_from_slice(&start);
    stream.read_exact(&mut buf[16..])?;
    Ok(Message::decode(&buf)?)
}

/// `EXTERNAL`: the bus checks our user id against the socket's peer
/// credentials. The leading nul byte is where libdbus would pass
/// credentials on systems that need them sent; Linux does not.
fn log_in(stream: &mut UnixStream) -> Result<()> {
    let uid = rustix::process::getuid().as_raw().to_string();
    let hex: String = uid.bytes().map(|b| format!("{b:02x}")).collect();
    stream.write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes())?;
    let line = read_line(stream)?;
    if !line.starts_with("OK ") {
        return Err(Error::Auth(line));
    }
    stream.write_all(b"BEGIN\r\n")?;
    Ok(())
}

/// One line of the login exchange, read a byte at a time so that nothing
/// after it is consumed.
fn read_line(stream: &mut impl Read) -> Result<String> {
    let mut line = Vec::new();
    while !line.ends_with(b"\r\n") {
        if line.len() > 1024 {
            return Err(Error::Auth("a line longer than 1 KiB".into()));
        }
        let mut b = [0];
        stream.read_exact(&mut b)?;
        line.push(b[0]);
    }
    line.truncate(line.len() - 2);
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// The session bus's socket, from `DBUS_SESSION_BUS_ADDRESS` or else
/// `$XDG_RUNTIME_DIR/bus`.
fn session_bus_socket() -> Result<UnixStream> {
    let runtime_bus = || {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            Error::NoBus("neither DBUS_SESSION_BUS_ADDRESS nor XDG_RUNTIME_DIR is set".into())
        })?;
        Ok(UnixStream::connect(Path::new(&dir).join("bus"))?)
    };
    let Ok(address) = std::env::var("DBUS_SESSION_BUS_ADDRESS") else {
        return runtime_bus();
    };
    let mut last = None;
    for addr in parse_address(&address) {
        let connected = match addr {
            Address::Path(path) => UnixStream::connect(std::ffi::OsStr::from_bytes(&path)),
            #[cfg(target_os = "linux")]
            Address::Abstract(name) => {
                use std::os::linux::net::SocketAddrExt;
                std::os::unix::net::SocketAddr::from_abstract_name(&name)
                    .and_then(|a| UnixStream::connect_addr(&a))
            }
            #[cfg(not(target_os = "linux"))]
            Address::Abstract(_) => continue,
            Address::Runtime => return runtime_bus(),
        };
        match connected {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => Error::Io(e),
        None => Error::NoBus(format!("no local socket in the address {address:?}")),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Address {
    Path(Vec<u8>),
    Abstract(Vec<u8>),
    /// `unix:runtime=yes`: `$XDG_RUNTIME_DIR/bus`.
    Runtime,
}

/// The local sockets a bus address lists, in its order: `unix:path=…`,
/// `unix:abstract=…` and `unix:runtime=yes`. Other transports are skipped.
fn parse_address(address: &str) -> Vec<Address> {
    let mut out = Vec::new();
    for entry in address.split(';') {
        let Some(params) = entry.strip_prefix("unix:") else {
            continue;
        };
        for param in params.split(',') {
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            let Some(value) = percent_decode(value) else {
                continue;
            };
            match key {
                "path" => out.push(Address::Path(value)),
                "abstract" => out.push(Address::Abstract(value)),
                "runtime" if value == b"yes" => out.push(Address::Runtime),
                _ => {}
            }
        }
    }
    out
}

/// Undoes `%XX` escapes (D-Bus addresses, URIs).
pub fn percent_decode(value: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    Some(out)
}

/// A bus for tests, on the other end of a socket pair.
#[cfg(test)]
pub mod test_bus {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::thread::JoinHandle;

    use super::{Connection, Kind, Message, Value};

    /// The unique name the test bus gives its client.
    pub const NAME: &str = ":1.42";

    /// A connection to a bus that takes any login, answers `Hello`, and
    /// passes every other message to `answer`, which pushes what to send
    /// back (serials are filled in) and returns false to hang up.
    pub fn connect(
        mut answer: impl FnMut(&Message, &mut Vec<Message>) -> bool + Send + 'static,
    ) -> (Connection, JoinHandle<()>) {
        let (client, mut bus) = UnixStream::pair().unwrap();
        let thread = std::thread::spawn(move || {
            let line = super::read_line(&mut bus).unwrap();
            assert!(line.starts_with("\0AUTH EXTERNAL "), "{line:?}");
            bus.write_all(b"OK 0123456789abcdef0123456789abcdef\r\n")
                .unwrap();
            let mut begin = [0; 7];
            bus.read_exact(&mut begin).unwrap();
            assert_eq!(&begin, b"BEGIN\r\n");
            let mut serial = 1000;
            while let Ok(m) = super::read_message(&mut bus) {
                let mut out = Vec::new();
                let more = if m.member.as_deref() == Some("Hello") {
                    out.push(reply(&m, vec![Value::Str(NAME.into())]));
                    true
                } else {
                    answer(&m, &mut out)
                };
                for mut r in out {
                    serial += 1;
                    r.serial = serial;
                    bus.write_all(&r.encode()).unwrap();
                }
                if !more {
                    return;
                }
            }
        });
        (Connection::on(client).unwrap(), thread)
    }

    pub fn reply(to: &Message, body: Vec<Value>) -> Message {
        Message {
            kind: Kind::Return,
            reply_serial: Some(to.serial),
            destination: Some(NAME.into()),
            sender: Some(":1.1".into()),
            body,
            ..Message::default()
        }
    }

    pub fn error(to: &Message, name: &str) -> Message {
        Message {
            kind: Kind::Error,
            error_name: Some(name.into()),
            ..reply(to, vec![Value::Str("no".into())])
        }
    }

    pub fn signal(path: &str, interface: &str, member: &str, body: Vec<Value>) -> Message {
        Message {
            kind: Kind::Signal,
            path: Some(path.into()),
            interface: Some(interface.into()),
            member: Some(member.into()),
            sender: Some(":1.1".into()),
            body,
            ..Message::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_bus::{self, error, reply, signal};
    use super::*;

    #[test]
    fn hello_gives_the_unique_name() {
        let (c, _bus) = test_bus::connect(|_, _| true);
        assert_eq!(c.unique_name(), test_bus::NAME);
    }

    #[test]
    fn signals_that_come_before_a_reply_wait_for_next_signal() {
        let (mut c, _bus) = test_bus::connect(|m, out| {
            out.push(signal("/a", "a.b", "Early", vec![]));
            out.push(reply(m, vec![Value::U32(7)]));
            out.push(signal("/a", "a.b", "Late", vec![]));
            true
        });
        let r = c
            .call(Message::call("a.b", "/a", "a.b", "M", vec![]))
            .unwrap();
        assert_eq!(r.body, [Value::U32(7)]);
        assert_eq!(c.next_signal().unwrap().member.as_deref(), Some("Early"));
        assert_eq!(c.next_signal().unwrap().member.as_deref(), Some("Late"));
    }

    #[test]
    fn error_replies_are_errors() {
        let (mut c, _bus) = test_bus::connect(|m, out| {
            out.push(error(m, UNKNOWN_METHOD));
            true
        });
        match c.call(Message::call("a.b", "/a", "a.b", "M", vec![])) {
            Err(Error::Remote(name, message)) => {
                assert_eq!((&*name, &*message), (UNKNOWN_METHOD, "no"))
            }
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn calls_to_us_are_answered() {
        let mut call_serial = 0;
        let (mut c, _bus) = test_bus::connect(move |m, out| {
            if m.member.as_deref() == Some("M") {
                // Two calls to the client before the reply: serials 1002 and
                // 1003, after the 1001 of the reply to Hello.
                call_serial = m.serial;
                let to_us = |interface, member| Message {
                    sender: Some(":1.1".into()),
                    ..Message::call(test_bus::NAME, "/", interface, member, vec![])
                };
                out.extend([
                    to_us("org.freedesktop.DBus.Peer", "Ping"),
                    to_us("a.b", "Other"),
                ]);
            } else {
                // The client's answers, sent back to it as signals; the
                // reply once both have come.
                let seen = format!("{:?}", (m.kind, m.reply_serial, m.error_name.as_deref()));
                out.push(signal("/a", "a.b", "Seen", vec![Value::Str(seen)]));
                if m.reply_serial == Some(1003) {
                    out.push(reply(
                        &Message {
                            serial: call_serial,
                            ..Message::default()
                        },
                        vec![],
                    ));
                }
            }
            true
        });
        c.call(Message::call("a.b", "/a", "a.b", "M", vec![]))
            .unwrap();
        let mut seen = || {
            c.next_signal().unwrap().body[0]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(seen(), "(Return, Some(1002), None)");
        assert_eq!(
            seen(),
            format!("(Error, Some(1003), Some({UNKNOWN_METHOD:?}))")
        );
    }

    #[test]
    fn a_closed_bus_is_an_error() {
        let (mut c, bus) = test_bus::connect(|_, _| false);
        let r = c.call(Message::call("a.b", "/a", "a.b", "M", vec![]));
        bus.join().unwrap();
        assert!(
            matches!(r, Err(Error::Io(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof),
            "{r:?}"
        );
        assert!(c.next_signal().is_err());
    }

    #[test]
    fn a_refused_login_is_an_error() {
        let (client, mut bus) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            read_line(&mut bus).unwrap();
            bus.write_all(b"REJECTED EXTERNAL\r\n").unwrap();
        });
        assert!(matches!(Connection::on(client), Err(Error::Auth(l)) if l == "REJECTED EXTERNAL"));
        t.join().unwrap();
    }

    #[test]
    fn addresses() {
        assert_eq!(
            parse_address(
                "unix:path=/run/user/1000/a%2cb,guid=00;tcp:host=x;unix:abstract=/tmp/dbus-x;unix:runtime=yes"
            ),
            [
                Address::Path(b"/run/user/1000/a,b".to_vec()),
                Address::Abstract(b"/tmp/dbus-x".to_vec()),
                Address::Runtime,
            ]
        );
        assert_eq!(parse_address("tcp:host=localhost,port=1"), []);
    }

    #[test]
    fn percent_decodes() {
        assert_eq!(
            percent_decode("/run/user/1000/a%2cb").as_deref(),
            Some(&b"/run/user/1000/a,b"[..])
        );
        assert_eq!(percent_decode("%2"), None);
    }
}
