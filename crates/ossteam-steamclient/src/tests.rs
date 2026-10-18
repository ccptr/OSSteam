// SPDX-License-Identifier: GPL-2.0-or-later
//! The library as `steam_api` uses it: through the exports and the
//! vtables, against a fake daemon on a socket of its own.
//!
//! The shim's state is global, as in a game, so one test walks through
//! everything in order.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ossteam_gameipc::endpoint::Address;
use ossteam_gameipc::{
    App, AppState, Dlc, ErrorReply, Hello, Message, Report, RequestBody, Response, ResponseBody,
    Session, Welcome, read_message, write_message,
};

use crate::abi::{CSteamID, CallbackMsg, find_version};
use crate::vtables::{SLOTS, VERSIONS};

const STEAM_ID: u64 = 76561197960287930;

enum Event {
    Hello(Hello),
    Request(RequestBody),
    Closed,
}

struct Fake {
    dir: PathBuf,
    events: mpsc::Receiver<Event>,
    reports: mpsc::Receiver<Report>,
    conns: Arc<Mutex<Vec<UnixStream>>>,
}

fn session(logged_on: bool) -> Session {
    Session {
        active: true,
        logged_on,
        steam_id: STEAM_ID,
        persona_name: "Gabe".into(),
        persona_state: 1,
        ip_country: "DE".into(),
        server_time: 1_790_000_000,
        universe: 1,
        ui_language: "german".into(),
    }
}

impl Fake {
    fn start(dir: PathBuf, app: App) -> Self {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let listener = UnixListener::bind(dir.join("game.sock")).unwrap();
        let (tx, events) = mpsc::channel();
        let (report_tx, reports) = mpsc::channel();
        let conns = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&conns);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                shared.lock().unwrap().push(stream.try_clone().unwrap());
                let tx = tx.clone();
                let report_tx = report_tx.clone();
                let app = app.clone();
                std::thread::spawn(move || serve(&mut stream, &tx, &report_tx, app));
            }
        });
        Self {
            dir,
            events,
            reports,
            conns,
        }
    }

    fn next(&self) -> Event {
        self.events
            .recv_timeout(Duration::from_secs(5))
            .expect("an event")
    }

    fn send_all(&self, msg: &Message) {
        for c in self.conns.lock().unwrap().iter_mut() {
            let _ = write_message(c, msg);
        }
    }
}

fn serve(
    stream: &mut UnixStream,
    tx: &mpsc::Sender<Event>,
    reports: &mpsc::Sender<Report>,
    app: App,
) {
    let mut writer = stream.try_clone().unwrap();
    while let Ok(msg) = read_message(stream) {
        match msg {
            Message::Hello(h) => {
                let _ = tx.send(Event::Hello(h));
                let welcome = Message::Welcome(Welcome {
                    session: session(true),
                    app: app.clone(),
                });
                write_message(&mut writer, &welcome).unwrap();
            }
            Message::Report(r) => {
                let _ = reports.send(r);
            }
            Message::Request(r) => {
                let body = match &r.body {
                    RequestBody::AppState(a) if a.app_id == 10 => {
                        ResponseBody::AppState(AppState {
                            owned: true,
                            installed: true,
                            install_dir: "/games/ten".into(),
                            build_id: 3,
                            depots: vec![11, 12],
                        })
                    }
                    RequestBody::AppState(_) => ResponseBody::Error(ErrorReply {
                        message: "unknown app".into(),
                    }),
                };
                let _ = tx.send(Event::Request(r.body));
                let _ = write_message(&mut writer, &Message::Response(Response { id: r.id, body }));
            }
            _ => break,
        }
    }
    let _ = tx.send(Event::Closed);
}

/// The slot of `method` in `version`'s vtable.
fn slot(obj: *mut c_void, version: &str, method: &str) -> *const c_void {
    let v = find_version(version.as_bytes()).unwrap();
    let info = &VERSIONS[v];
    let index = (0..info.vtable.len())
        .find(|&i| SLOTS[info.first_slot + i].method == method)
        .unwrap_or_else(|| panic!("{version} has no {method}"));
    // SAFETY: an object starts with its vtable pointer, as in C++.
    unsafe {
        let vt = *(obj as *const *const *const c_void);
        *vt.add(index)
    }
}

/// Calls a method through the vtable with the given signature.
macro_rules! call {
    ($obj:expr, $version:literal, $method:literal, fn($($t:ty),*) -> $r:ty $(, $arg:expr)*) => {{
        let f = slot($obj, $version, $method);
        // SAFETY: the signature is the one vtables.txt lists for the slot.
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void $(, $t)*) -> $r = std::mem::transmute(f);
            f($obj $(, $arg)*)
        }
    }};
}

fn create_interface(version: &CStr) -> (*mut c_void, c_int) {
    let mut rc = -1;
    // SAFETY: a valid string and out-pointer.
    let obj = unsafe { crate::CreateInterface(version.as_ptr(), &mut rc) };
    (obj, rc)
}

fn next_callback(pipe: i32) -> Option<(i32, Vec<u8>)> {
    // SAFETY: zero is a valid CallbackMsg.
    let mut msg: CallbackMsg = unsafe { std::mem::zeroed() };
    // SAFETY: `msg` is writable.
    if !unsafe { crate::Steam_BGetCallback(pipe, &mut msg, std::ptr::null_mut()) } {
        return None;
    }
    let (id, param, size) = (msg.callback, msg.param, msg.size);
    // SAFETY: the shim keeps the callback until it is freed.
    let data = unsafe { std::slice::from_raw_parts(param, size as usize) }.to_vec();
    assert!(crate::Steam_FreeLastCallback(pipe));
    Some((id, data))
}

/// Pumps callbacks for `ms` milliseconds, as a game's frames would.
fn pump(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        // SAFETY: a null message is allowed and returns false.
        unsafe { crate::Steam_BGetCallback(0, std::ptr::null_mut(), std::ptr::null_mut()) };
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_callback(pipe: i32, id: i32) -> Vec<u8> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some((got, data)) = next_callback(pipe)
            && got == id
        {
            return data;
        }
        assert!(Instant::now() < end, "callback {id} did not arrive");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn read_c(buf: &[u8]) -> &str {
    CStr::from_bytes_until_nul(buf).unwrap().to_str().unwrap()
}

fn str_at<'a>(p: *const c_char) -> &'a str {
    // SAFETY: the shim returns strings that live as long as the process.
    unsafe { CStr::from_ptr(p) }.to_str().unwrap()
}

#[test]
fn a_game_session() {
    let base = std::env::temp_dir().join(format!("ossteam-shim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let user_dir = base.join("userdata/22202/480");
    // SAFETY: no other thread reads the environment yet.
    unsafe { std::env::set_var("SteamAppId", "480") };

    // Without a daemon, there is no pipe.
    crate::pipe::override_endpoint(Address::in_dir(base.join("none")));
    let (client, rc) = create_interface(c"SteamClient020");
    assert!(!client.is_null());
    assert_eq!(rc, 0);
    assert_eq!(
        call!(client, "SteamClient020", "CreateSteamPipe", fn() -> i32),
        0
    );
    let (none, rc) = create_interface(c"SteamClient999");
    assert!(none.is_null());
    assert_eq!(rc, 1);
    // SAFETY: a valid string.
    assert!(unsafe { crate::Steam_IsKnownInterface(c"SteamUser021".as_ptr()) });
    // SAFETY: a valid string.
    assert!(!unsafe { crate::Steam_IsKnownInterface(c"SteamUser999".as_ptr()) });

    let app = App {
        app_id: 480,
        owned: true,
        launched: true,
        installed: true,
        install_dir: "/games/spacewar".into(),
        build_id: 42,
        branch: String::new(),
        language: "german".into(),
        languages: vec!["english".into(), "german".into()],
        depots: vec![481],
        dlc: vec![Dlc {
            app_id: 110902,
            owned: true,
            installed: false,
            available: true,
            name: "Pieterw test DLC".into(),
        }],
        user_dir: user_dir.to_string_lossy().into_owned(),
        owner: STEAM_ID,
        low_violence: false,
    };
    let fake = Fake::start(base.join("run"), app);
    crate::pipe::override_endpoint(Address::in_dir(&fake.dir));

    // The handshake.
    let pipe = call!(client, "SteamClient020", "CreateSteamPipe", fn() -> i32);
    assert_eq!(pipe, 1);
    let Event::Hello(hello) = fake.next() else {
        panic!("no hello")
    };
    assert_eq!(hello.app_id, 480);
    assert_eq!(hello.pid, std::process::id());
    assert_eq!(hello.protocol, ossteam_gameipc::PROTOCOL);
    let user = call!(
        client,
        "SteamClient020",
        "ConnectToGlobalUser",
        fn(i32) -> i32,
        pipe
    );
    assert_eq!(user, 1);

    // Getters.
    let get_user = |u: i32, version: &CStr| {
        call!(
            client,
            "SteamClient020",
            "GetISteamUser",
            fn(i32, i32, *const c_char) -> *mut c_void,
            u,
            pipe,
            version.as_ptr()
        )
    };
    let steam_user = get_user(user, c"SteamUser021");
    assert!(!steam_user.is_null());
    assert_eq!(get_user(user, c"SteamUser021"), steam_user);
    assert!(get_user(0, c"SteamUser021").is_null());
    assert!(get_user(user, c"SteamFriends015").is_null());
    let utils = call!(
        client,
        "SteamClient020",
        "GetISteamUtils",
        fn(i32, *const c_char) -> *mut c_void,
        pipe,
        c"SteamUtils010".as_ptr()
    );
    assert!(!utils.is_null());
    let apps = call!(
        client,
        "SteamClient020",
        "GetISteamApps",
        fn(i32, i32, *const c_char) -> *mut c_void,
        user,
        pipe,
        c"STEAMAPPS_INTERFACE_VERSION008".as_ptr()
    );
    assert!(!apps.is_null());
    let friends = call!(
        client,
        "SteamClient020",
        "GetISteamGenericInterface",
        fn(i32, i32, *const c_char) -> *mut c_void,
        user,
        pipe,
        c"SteamFriends017".as_ptr()
    );
    assert!(!friends.is_null());

    // ISteamUser.
    assert_eq!(
        call!(steam_user, "SteamUser021", "GetHSteamUser", fn() -> i32),
        user
    );
    assert_eq!(
        call!(steam_user, "SteamUser021", "BLoggedOn", fn() -> u8),
        1
    );
    assert_eq!(
        call!(steam_user, "SteamUser021", "GetSteamID", fn() -> CSteamID),
        CSteamID(STEAM_ID)
    );
    let mut buf = [0u8; 512];
    assert_eq!(
        call!(
            steam_user,
            "SteamUser021",
            "GetUserDataFolder",
            fn(*mut u8, i32) -> u8,
            buf.as_mut_ptr(),
            512
        ),
        1
    );
    assert_eq!(PathBuf::from(read_c(&buf)), user_dir.join("local"));
    assert!(user_dir.join("local").is_dir());
    assert_eq!(
        call!(
            steam_user,
            "SteamUser021",
            "GetUserDataFolder",
            fn(*mut u8, i32) -> u8,
            buf.as_mut_ptr(),
            4
        ),
        0
    );
    assert_eq!(buf[0], 0);

    // ISteamApps, from the welcome and from the daemon.
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "BIsSubscribed",
            fn() -> u8
        ),
        1
    );
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "BIsSubscribedApp",
            fn(u32) -> u8,
            10
        ),
        1
    );
    assert!(matches!(fake.next(), Event::Request(RequestBody::AppState(a)) if a.app_id == 10));
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "BIsSubscribedApp",
            fn(u32) -> u8,
            11
        ),
        0
    );
    assert!(matches!(fake.next(), Event::Request(_)));
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "BIsDlcInstalled",
            fn(u32) -> u8,
            110902
        ),
        0
    );
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "GetDLCCount",
            fn() -> i32
        ),
        1
    );
    let (mut dlc, mut available) = (0u32, 0u8);
    let mut name = [0xffu8; 8];
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "BGetDLCDataByIndex",
            fn(i32, *mut u32, *mut u8, *mut u8, i32) -> u8,
            0,
            &mut dlc,
            &mut available,
            name.as_mut_ptr(),
            8
        ),
        1
    );
    assert_eq!((dlc, available, read_c(&name)), (110902, 1, "Pieterw"));
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "BGetDLCDataByIndex",
            fn(i32, *mut u32, *mut u8, *mut u8, i32) -> u8,
            1,
            &mut dlc,
            &mut available,
            name.as_mut_ptr(),
            8
        ),
        0
    );
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "GetAppBuildId",
            fn() -> i32
        ),
        42
    );
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "GetCurrentBetaName",
            fn(*mut u8, i32) -> u8,
            buf.as_mut_ptr(),
            512
        ),
        0
    );
    let n = call!(
        apps,
        "STEAMAPPS_INTERFACE_VERSION008",
        "GetAppInstallDir",
        fn(u32, *mut u8, u32) -> u32,
        10,
        buf.as_mut_ptr(),
        512
    );
    assert_eq!((n, read_c(&buf)), (11, "/games/ten"));
    let _ = fake.next();
    let mut depots = [0u32; 1];
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "GetInstalledDepots",
            fn(u32, *mut u32, u32) -> u32,
            10,
            depots.as_mut_ptr(),
            1
        ),
        1
    );
    assert_eq!(depots, [11]);
    let _ = fake.next();
    assert_eq!(
        str_at(call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "GetCurrentGameLanguage",
            fn() -> *const c_char
        )),
        "german"
    );
    assert_eq!(
        str_at(call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "GetAvailableGameLanguages",
            fn() -> *const c_char
        )),
        "english,german"
    );
    assert_eq!(
        call!(
            apps,
            "STEAMAPPS_INTERFACE_VERSION008",
            "GetAppOwner",
            fn() -> CSteamID
        ),
        CSteamID(STEAM_ID)
    );

    // ISteamUtils and ISteamFriends.
    assert_eq!(call!(utils, "SteamUtils010", "GetAppID", fn() -> u32), 480);
    assert_eq!(
        call!(utils, "SteamUtils010", "GetConnectedUniverse", fn() -> u32),
        1
    );
    assert!(call!(utils, "SteamUtils010", "GetServerRealTime", fn() -> u32) >= 1_790_000_000);
    assert_eq!(
        str_at(call!(
            utils,
            "SteamUtils010",
            "GetIPCountry",
            fn() -> *const c_char
        )),
        "DE"
    );
    assert_eq!(
        str_at(call!(
            utils,
            "SteamUtils010",
            "GetSteamUILanguage",
            fn() -> *const c_char
        )),
        "german"
    );
    assert_eq!(
        call!(utils, "SteamUtils010", "IsOverlayEnabled", fn() -> u8),
        0
    );
    assert_eq!(
        call!(
            utils,
            "SteamUtils010",
            "GetAPICallFailureReason",
            fn(u64) -> u32,
            99
        ),
        2
    );
    assert_eq!(
        str_at(call!(
            friends,
            "SteamFriends017",
            "GetPersonaName",
            fn() -> *const c_char
        )),
        "Gabe"
    );
    assert_eq!(
        call!(friends, "SteamFriends017", "GetPersonaState", fn() -> u32),
        1
    );

    // A stub returns its failure value and is reported, with the
    // interfaces handed out.
    assert_eq!(
        call!(
            steam_user,
            "SteamUser021",
            "GetPlayerSteamLevel",
            fn() -> i32
        ),
        0
    );
    // SAFETY: a valid string.
    unsafe { crate::Steam_NotifyMissingInterface(pipe, c"SteamTimeline999".as_ptr()) };
    crate::Breakpad_SteamSetAppID(480);
    let mut seen = Report::default();
    // Collect everything that arrives in a short while.
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        pump(10);
        while let Ok(r) = fake.reports.try_recv() {
            seen.interfaces.extend(r.interfaces);
            seen.unknown.extend(r.unknown);
            seen.calls.extend(r.calls);
        }
    }
    for i in [
        "SteamClient020",
        "SteamUser021",
        "SteamUtils010",
        "STEAMAPPS_INTERFACE_VERSION008",
        "SteamFriends017",
    ] {
        assert!(
            seen.interfaces.iter().any(|s| s == i),
            "{i} not reported: {seen:?}"
        );
    }
    for u in ["SteamClient999", "SteamFriends015", "SteamTimeline999"] {
        assert!(
            seen.unknown.iter().any(|s| s == u),
            "{u} not reported: {seen:?}"
        );
    }
    assert!(seen.calls.iter().any(|c| c.interface == "SteamUser021"
        && c.method == "GetPlayerSteamLevel"
        && c.count == 1));
    assert!(
        seen.calls
            .iter()
            .any(|c| c.interface == "exports" && c.method == "Breakpad_SteamSetAppID")
    );

    // The connection to Steam drops and comes back.
    fake.send_all(&Message::SessionUpdate(session(false)));
    assert_eq!(wait_callback(pipe, 103), 3i32.to_le_bytes());
    assert_eq!(
        call!(steam_user, "SteamUser021", "BLoggedOn", fn() -> u8),
        0
    );
    assert_eq!(
        call!(friends, "SteamFriends017", "GetPersonaState", fn() -> u32),
        0
    );
    fake.send_all(&Message::SessionUpdate(session(true)));
    assert_eq!(wait_callback(pipe, 101), [0]);
    assert_eq!(
        call!(steam_user, "SteamUser021", "BLoggedOn", fn() -> u8),
        1
    );

    // An API call, and what happens to one pending when the daemon goes.
    let p = crate::pipe::get(pipe).unwrap();
    fn decode(b: &ResponseBody) -> Option<Vec<u8>> {
        match b {
            ResponseBody::AppState(s) => Some(s.build_id.to_le_bytes().to_vec()),
            ResponseBody::Error(_) => None,
        }
    }
    let call = p.begin_call(
        RequestBody::AppState(ossteam_gameipc::AppStateRequest { app_id: 10 }),
        1234,
        4,
        decode,
    );
    assert_ne!(call, 0);
    let done = wait_callback(pipe, 703);
    assert_eq!(done[..8], call.to_le_bytes());
    assert_eq!(done[8..12], 1234i32.to_le_bytes());
    assert_eq!(done[12..16], 4u32.to_le_bytes());
    let mut failed = 0xffu8;
    assert_eq!(
        call!(
            utils,
            "SteamUtils010",
            "IsAPICallCompleted",
            fn(u64, *mut u8) -> u8,
            call,
            &mut failed
        ),
        1
    );
    assert_eq!(failed, 0);
    assert_eq!(
        call!(
            utils,
            "SteamUtils010",
            "GetAPICallFailureReason",
            fn(u64) -> u32,
            call
        ),
        u32::MAX
    );
    let mut result = [0u8; 4];
    // Wrong size or id: kept.
    // SAFETY: `result` holds 4 bytes.
    assert!(!unsafe {
        crate::Steam_GetAPICallResult(
            pipe,
            call,
            result.as_mut_ptr().cast(),
            4,
            1,
            std::ptr::null_mut(),
        )
    });
    let mut failed = true;
    // SAFETY: as above.
    assert!(unsafe {
        crate::Steam_GetAPICallResult(pipe, call, result.as_mut_ptr().cast(), 4, 1234, &mut failed)
    });
    assert_eq!((result, failed), (3u32.to_le_bytes(), false));
    // SAFETY: as above.
    assert!(!unsafe {
        crate::Steam_GetAPICallResult(pipe, call, result.as_mut_ptr().cast(), 4, 1234, &mut failed)
    });

    // The daemon closes the connection.
    for c in fake.conns.lock().unwrap().drain(..) {
        c.shutdown(std::net::Shutdown::Both).unwrap();
    }
    assert_eq!(wait_callback(pipe, 103), 3i32.to_le_bytes());
    assert_eq!(
        call!(steam_user, "SteamUser021", "BLoggedOn", fn() -> u8),
        0
    );
    // The Steam id stays, as for an account that is logged off.
    assert_eq!(
        call!(steam_user, "SteamUser021", "GetSteamID", fn() -> CSteamID),
        CSteamID(STEAM_ID)
    );
    let call = p.begin_call(
        RequestBody::AppState(ossteam_gameipc::AppStateRequest { app_id: 10 }),
        1234,
        4,
        decode,
    );
    assert_eq!(call, 0);
    drop(p);

    // A new pipe, then releasing it ends the daemon's side.
    let pipe2 = call!(client, "SteamClient020", "CreateSteamPipe", fn() -> i32);
    assert_eq!(pipe2, 2);
    while !matches!(fake.next(), Event::Hello(_)) {}
    assert_eq!(
        call!(
            client,
            "SteamClient020",
            "BShutdownIfAllPipesClosed",
            fn() -> u8
        ),
        0
    );
    assert_eq!(
        call!(
            client,
            "SteamClient020",
            "BReleaseSteamPipe",
            fn(i32) -> u8,
            pipe
        ),
        1
    );
    assert_eq!(
        call!(
            client,
            "SteamClient020",
            "BReleaseSteamPipe",
            fn(i32) -> u8,
            pipe2
        ),
        1
    );
    assert_eq!(
        call!(
            client,
            "SteamClient020",
            "BReleaseSteamPipe",
            fn(i32) -> u8,
            pipe2
        ),
        0
    );
    loop {
        if matches!(fake.next(), Event::Closed) {
            break;
        }
    }
    assert_eq!(
        call!(
            client,
            "SteamClient020",
            "BShutdownIfAllPipesClosed",
            fn() -> u8
        ),
        1
    );
    // Objects of a released pipe answer as logged off.
    assert_eq!(
        call!(steam_user, "SteamUser021", "BLoggedOn", fn() -> u8),
        0
    );
    assert_eq!(
        call!(steam_user, "SteamUser021", "GetSteamID", fn() -> CSteamID),
        CSteamID(0)
    );

    let _ = std::io::stderr().flush();
    let _ = std::fs::remove_dir_all(&base);
}
