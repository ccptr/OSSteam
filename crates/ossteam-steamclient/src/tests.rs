// SPDX-License-Identifier: GPL-2.0-or-later
//! The library as `steam_api` uses it: through the exports and the
//! vtables, against a fake daemon on a socket of its own.
//!
//! The shim's state is global, as in a game, so one test walks through
//! everything in order.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ossteam_gameipc::endpoint::Address;
use ossteam_gameipc::{
    AchievementDef, AchievementPercent, App, AppState, AuthTicket, BeginAuthSession, Dlc, Done,
    EncryptedAppTicket, ErrorReply, GlobalAchievements, Hello, Image, Message, NoticeBody, Report,
    RequestBody, Response, ResponseBody, Session, StatDef, StatValue, StatsSchema, StoreStats,
    TicketActive, TicketValidated, Unlock, UserLicense, UserStats, Welcome, read_message,
    write_message,
};

use crate::abi::{CSteamID, CallbackMsg, find_version};
use crate::vtables::{SLOTS, VERSIONS};

const STEAM_ID: u64 = 76561197960287930;
/// Another user, whose stats a game may ask for.
const OTHER: u64 = 76561197960287931;

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
                    other @ (RequestBody::UserStats(_)
                    | RequestBody::StoreStats(_)
                    | RequestBody::GlobalAchievements(_)
                    | RequestBody::AchievementIcon(_)) => stats_answer(other),
                    other => ticket_answer(other),
                };
                let notice = ticket_notice(&r.body, &body);
                let _ = tx.send(Event::Request(r.body));
                let _ = write_message(&mut writer, &Message::Response(Response { id: r.id, body }));
                if let Some(n) = notice {
                    let _ = write_message(&mut writer, &Message::Notice(n));
                }
            }
            _ => break,
        }
    }
    let _ = tx.send(Event::Closed);
}

fn schema() -> StatsSchema {
    StatsSchema {
        version: 20,
        stats: vec![
            StatDef {
                id: 1,
                kind: 4,
                achievements: vec![
                    AchievementDef {
                        bit: 0,
                        name: "ACH_WIN_ONE_GAME".into(),
                        display_name: "Winner".into(),
                        description: "Win one game.".into(),
                        icon: "win.jpg".into(),
                        icon_gray: "win_gray.jpg".into(),
                        ..Default::default()
                    },
                    AchievementDef {
                        bit: 1,
                        name: "ACH_TRAVEL_FAR".into(),
                        hidden: true,
                        icon_gray: "far_gray.jpg".into(),
                        progress_stat: "FeetTraveled".into(),
                        progress_min: 0f32.to_bits(),
                        progress_max: 5280f32.to_bits(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            StatDef {
                id: 2,
                name: "NumGames".into(),
                kind: 1,
                ..Default::default()
            },
            StatDef {
                id: 3,
                name: "FeetTraveled".into(),
                kind: 2,
                default: 1.5f32.to_bits(),
                ..Default::default()
            },
        ],
    }
}

/// What the fake daemon answers to the ticket requests (`isteamuser.md`
/// §3): a ticket of 40 bytes, its CRC (counting from 1000) then its kind;
/// sessions
/// and licenses for [`OTHER`] only; an "encrypted" ticket that is the
/// user data reversed.
fn ticket_answer(body: &RequestBody) -> ResponseBody {
    match body {
        RequestBody::AuthTicket(r) if r.max != 0 && r.max < 40 => {
            ResponseBody::AuthTicket(AuthTicket {
                result: 2,
                ..Default::default()
            })
        }
        RequestBody::AuthTicket(r) => {
            static CRC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1000);
            let crc = CRC.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut ticket = vec![r.kind as u8; 40];
            ticket[..4].copy_from_slice(&crc.to_le_bytes());
            ResponseBody::AuthTicket(AuthTicket {
                result: 1,
                crc,
                ticket,
            })
        }
        RequestBody::CancelAuthTicket(_) | RequestBody::EndAuthSession(_) => {
            ResponseBody::Done(Done {})
        }
        RequestBody::BeginAuthSession(r) => ResponseBody::BeginAuthSession(BeginAuthSession {
            result: if r.steam_id == OTHER && !r.ticket.is_empty() {
                0
            } else {
                1
            },
        }),
        RequestBody::UserLicense(r) => ResponseBody::UserLicense(UserLicense {
            result: if r.steam_id == OTHER && r.app_id == 480 {
                0
            } else {
                2
            },
        }),
        RequestBody::EncryptedAppTicket(r) => {
            ResponseBody::EncryptedAppTicket(EncryptedAppTicket {
                result: 1,
                ticket: r.userdata.iter().rev().copied().collect(),
            })
        }
        _ => ResponseBody::Error(ErrorReply {
            message: "unexpected".into(),
        }),
    }
}

/// What the fake daemon tells after answering: a ticket is active at
/// once, and Steam accepts a session.
fn ticket_notice(request: &RequestBody, answer: &ResponseBody) -> Option<NoticeBody> {
    match (request, answer) {
        (_, ResponseBody::AuthTicket(a)) if a.result == 1 => {
            Some(NoticeBody::TicketActive(TicketActive {
                crc: a.crc,
                result: 1,
            }))
        }
        (RequestBody::BeginAuthSession(r), ResponseBody::BeginAuthSession(b)) if b.result == 0 => {
            Some(NoticeBody::TicketValidated(TicketValidated {
                steam_id: r.steam_id,
                owner: r.steam_id,
                response: 0,
            }))
        }
        _ => None,
    }
}

/// What the fake daemon answers to the stats requests
/// (`isteamuserstats.md` §2). A store reverts `NumGames` above 100, as
/// Steam would for a stat whose `max` is 100.
fn stats_answer(body: &RequestBody) -> ResponseBody {
    match body {
        RequestBody::UserStats(r) if r.steam_id == 0 => ResponseBody::UserStats(UserStats {
            result: 1,
            steam_id: STEAM_ID,
            crc: 7,
            schema: schema(),
            values: vec![StatValue { id: 2, value: 5 }],
            unlocks: vec![],
        }),
        RequestBody::UserStats(r) => ResponseBody::UserStats(UserStats {
            result: 1,
            steam_id: r.steam_id,
            crc: 3,
            schema: schema(),
            values: vec![StatValue { id: 1, value: 1 }, StatValue { id: 2, value: 9 }],
            unlocks: vec![Unlock {
                stat: 1,
                bit: 0,
                time: 1234,
            }],
        }),
        RequestBody::StoreStats(r) => ResponseBody::StoreStats(StoreStats {
            result: 1,
            crc: r.crc + 1,
            reverted: r
                .values
                .iter()
                .filter(|v| v.id == 2 && v.value > 100)
                .map(|v| StatValue {
                    id: v.id,
                    value: 100,
                })
                .collect(),
            out_of_date: false,
        }),
        RequestBody::GlobalAchievements(_) => {
            ResponseBody::GlobalAchievements(GlobalAchievements {
                result: 1,
                achievements: vec![
                    AchievementPercent {
                        name: "ACH_TRAVEL_FAR".into(),
                        percent: 50f32.to_bits(),
                    },
                    AchievementPercent {
                        name: "ACH_WIN_ONE_GAME".into(),
                        percent: 75.5f32.to_bits(),
                    },
                ],
            })
        }
        RequestBody::AchievementIcon(r) if r.file == "win.jpg" => ResponseBody::Image(Image {
            result: 1,
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
        }),
        _ => ResponseBody::Error(ErrorReply {
            message: "unexpected".into(),
        }),
    }
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

/// Overloads keep their declaration order outside Windows (§2.4).
#[test]
fn overloads_in_declaration_order() {
    let v = &VERSIONS[find_version(b"STEAMUSERSTATS_INTERFACE_VERSION013").unwrap()];
    let methods: Vec<&str> = (0..4).map(|i| SLOTS[v.first_slot + i].method).collect();
    assert_eq!(methods, ["GetStat", "GetStat_2", "SetStat", "SetStat_2"]);
    let v = &VERSIONS[find_version(b"STEAMINVENTORY_INTERFACE_V003").unwrap()];
    let at = (0..v.vtable.len())
        .find(|&i| SLOTS[v.first_slot + i].method == "SetProperty")
        .unwrap();
    let methods: Vec<&str> = (at..at + 4)
        .map(|i| SLOTS[v.first_slot + i].method)
        .collect();
    assert_eq!(
        methods,
        [
            "SetProperty",
            "SetProperty_2",
            "SetProperty_3",
            "SetProperty_4"
        ]
    );
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
        cloud_quota: 1000,
        cloud_max_files: 3,
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

    stats_session(&fake, client, user, pipe, utils);
    remote_storage_session(client, user, pipe, &user_dir);
    tickets_session(&fake, client, user, pipe);

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
    fn decode(_: &crate::pipe::Pipe, b: &ResponseBody) -> Option<Vec<u8>> {
        match b {
            ResponseBody::AppState(s) => Some(s.build_id.to_le_bytes().to_vec()),
            _ => None,
        }
    }
    let call = p.begin_call(
        RequestBody::AppState(ossteam_gameipc::AppStateRequest { app_id: 10 }),
        1234,
        4,
        Box::new(decode),
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
        Box::new(decode),
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

fn next_request(fake: &Fake) -> RequestBody {
    loop {
        if let Event::Request(r) = fake.next() {
            return r;
        }
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// `ISteamUserStats` and the images of `ISteamUtils`
/// (`isteamuserstats.md`).
fn stats_session(fake: &Fake, client: *mut c_void, user: i32, pipe: i32, utils: *mut c_void) {
    let stats = call!(
        client,
        "SteamClient020",
        "GetISteamUserStats",
        fn(i32, i32, *const c_char) -> *mut c_void,
        user,
        pipe,
        c"STEAMUSERSTATS_INTERFACE_VERSION013".as_ptr()
    );
    assert!(!stats.is_null());
    macro_rules! us {
        ($method:literal, fn($($t:ty),*) -> $r:ty $(, $arg:expr)*) => {
            call!(stats, "STEAMUSERSTATS_INTERFACE_VERSION013", $method, fn($($t),*) -> $r $(, $arg)*)
        };
    }

    // Version 013 fetches without being asked.
    assert!(matches!(next_request(fake), RequestBody::UserStats(r) if r.steam_id == 0));
    let received = wait_callback(pipe, 1101);
    assert_eq!(received.len(), 20);
    assert_eq!(received[..8], 480u64.to_le_bytes());
    assert_eq!(u32_at(&received, 8), 1);
    assert_eq!(received[12..20], STEAM_ID.to_le_bytes());

    // Stats.
    let mut n = 0i32;
    assert_eq!(
        us!(
            "GetStat",
            fn(*const c_char, *mut i32) -> u8,
            c"NumGames".as_ptr(),
            &mut n
        ),
        1
    );
    assert_eq!(n, 5);
    let mut f = 0f32;
    assert_eq!(
        us!(
            "GetStat_2",
            fn(*const c_char, *mut f32) -> u8,
            c"NumGames".as_ptr(),
            &mut f
        ),
        0
    );
    assert_eq!(
        us!(
            "GetStat_2",
            fn(*const c_char, *mut f32) -> u8,
            c"FeetTraveled".as_ptr(),
            &mut f
        ),
        1
    );
    assert_eq!(f, 1.5);
    assert_eq!(
        us!(
            "GetStat",
            fn(*const c_char, *mut i32) -> u8,
            c"Nope".as_ptr(),
            &mut n
        ),
        0
    );
    assert_eq!(
        us!(
            "SetStat",
            fn(*const c_char, i32) -> u8,
            c"NumGames".as_ptr(),
            150
        ),
        1
    );
    assert_eq!(
        us!(
            "SetStat_2",
            fn(*const c_char, f32) -> u8,
            c"NumGames".as_ptr(),
            2.0
        ),
        0
    );
    assert_eq!(
        us!(
            "SetStat_2",
            fn(*const c_char, f32) -> u8,
            c"FeetTraveled".as_ptr(),
            2.5
        ),
        1
    );

    // Achievements and the schema.
    let (mut got, mut time) = (9u8, 9u32);
    let win = c"ACH_WIN_ONE_GAME".as_ptr();
    let far = c"ACH_TRAVEL_FAR".as_ptr();
    assert_eq!(
        us!(
            "GetAchievement",
            fn(*const c_char, *mut u8) -> u8,
            win,
            &mut got
        ),
        1
    );
    assert_eq!(got, 0);
    assert_eq!(us!("SetAchievement", fn(*const c_char) -> u8, win), 1);
    assert_eq!(
        us!(
            "GetAchievementAndUnlockTime",
            fn(*const c_char, *mut u8, *mut u32) -> u8,
            win,
            &mut got,
            &mut time
        ),
        1
    );
    assert_eq!(got, 1);
    assert!(time >= 1_790_000_000);
    assert_eq!(us!("GetNumAchievements", fn() -> u32), 2);
    assert_eq!(
        str_at(us!("GetAchievementName", fn(u32) -> *const c_char, 1)),
        "ACH_TRAVEL_FAR"
    );
    assert_eq!(
        str_at(us!("GetAchievementName", fn(u32) -> *const c_char, 2)),
        ""
    );
    let attr = |a: *const c_char, k: &CStr| {
        str_at(us!(
            "GetAchievementDisplayAttribute",
            fn(*const c_char, *const c_char) -> *const c_char,
            a,
            k.as_ptr()
        ))
    };
    assert_eq!(attr(win, c"name"), "Winner");
    assert_eq!(attr(win, c"desc"), "Win one game.");
    assert_eq!(attr(win, c"hidden"), "0");
    assert_eq!(attr(far, c"hidden"), "1");
    assert_eq!(attr(far, c"icon"), "");
    let (mut lo, mut hi) = (1f32, 1f32);
    assert_eq!(
        us!(
            "GetAchievementProgressLimits_2",
            fn(*const c_char, *mut f32, *mut f32) -> u8,
            far,
            &mut lo,
            &mut hi
        ),
        1
    );
    assert_eq!((lo, hi), (0.0, 5280.0));
    let (mut ilo, mut ihi) = (1i32, 1i32);
    assert_eq!(
        us!(
            "GetAchievementProgressLimits",
            fn(*const c_char, *mut i32, *mut i32) -> u8,
            far,
            &mut ilo,
            &mut ihi
        ),
        0
    );
    assert_eq!(
        us!(
            "IndicateAchievementProgress",
            fn(*const c_char, u32, u32) -> u8,
            far,
            10,
            100
        ),
        1
    );
    let progress = wait_callback(pipe, 1103);
    assert_eq!(progress.len(), 148);
    assert_eq!(read_c(&progress[9..137]), "ACH_TRAVEL_FAR");
    assert_eq!((u32_at(&progress, 140), u32_at(&progress, 144)), (10, 100));
    assert_eq!(
        us!(
            "IndicateAchievementProgress",
            fn(*const c_char, u32, u32) -> u8,
            win,
            1,
            2
        ),
        0
    );

    // Storing: Steam reverts NumGames; the achievement is announced.
    assert_eq!(us!("StoreStats", fn() -> u8), 1);
    let RequestBody::StoreStats(sent) = next_request(fake) else {
        panic!("no store")
    };
    assert_eq!(sent.crc, 7);
    assert!(!sent.reset);
    assert_eq!(
        sent.values,
        [
            StatValue { id: 1, value: 1 },
            StatValue { id: 2, value: 150 },
            StatValue {
                id: 3,
                value: 2.5f32.to_bits()
            },
        ]
    );
    let unlocked = wait_callback(pipe, 1103);
    assert_eq!(read_c(&unlocked[9..137]), "ACH_WIN_ONE_GAME");
    assert_eq!((u32_at(&unlocked, 140), u32_at(&unlocked, 144)), (0, 0));
    let stored = next_callback(pipe).expect("UserStatsStored_t");
    assert_eq!(stored.0, 1102);
    assert_eq!(stored.1.len(), 12);
    assert_eq!(u32_at(&stored.1, 8), 8);
    assert_eq!(
        us!(
            "GetStat",
            fn(*const c_char, *mut i32) -> u8,
            c"NumGames".as_ptr(),
            &mut n
        ),
        1
    );
    assert_eq!(n, 100);
    // Nothing changed since: still sent, with the new crc.
    assert_eq!(us!("StoreStats", fn() -> u8), 1);
    let RequestBody::StoreStats(sent) = next_request(fake) else {
        panic!("no store")
    };
    assert_eq!((sent.crc, sent.values.len()), (8, 0));
    assert_eq!(u32_at(&wait_callback(pipe, 1102), 8), 1);

    // Icons.
    assert_eq!(us!("GetAchievementIcon", fn(*const c_char) -> i32, win), 0);
    assert!(matches!(next_request(fake), RequestBody::AchievementIcon(r) if r.file == "win.jpg"));
    let fetched = wait_callback(pipe, 1109);
    assert_eq!(fetched.len(), 144);
    assert_eq!(read_c(&fetched[8..136]), "ACH_WIN_ONE_GAME");
    assert_eq!(fetched[136], 1);
    let handle = u32_at(&fetched, 140) as i32;
    assert!(handle > 0);
    assert_eq!(
        us!("GetAchievementIcon", fn(*const c_char) -> i32, win),
        handle
    );
    let (mut w, mut h) = (0u32, 0u32);
    assert_eq!(
        call!(
            utils,
            "SteamUtils010",
            "GetImageSize",
            fn(i32, *mut u32, *mut u32) -> u8,
            handle,
            &mut w,
            &mut h
        ),
        1
    );
    assert_eq!((w, h), (2, 1));
    let mut rgba = [0u8; 8];
    assert_eq!(
        call!(
            utils,
            "SteamUtils010",
            "GetImageRGBA",
            fn(i32, *mut u8, i32) -> u8,
            handle,
            rgba.as_mut_ptr(),
            4
        ),
        0
    );
    assert_eq!(
        call!(
            utils,
            "SteamUtils010",
            "GetImageRGBA",
            fn(i32, *mut u8, i32) -> u8,
            handle,
            rgba.as_mut_ptr(),
            8
        ),
        1
    );
    assert_eq!(rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
    assert_eq!(
        call!(
            utils,
            "SteamUtils010",
            "GetImageSize",
            fn(i32, *mut u32, *mut u32) -> u8,
            999,
            &mut w,
            &mut h
        ),
        0
    );
    // An icon the daemon cannot give: handle 0, and asked again later.
    assert_eq!(us!("GetAchievementIcon", fn(*const c_char) -> i32, far), 0);
    assert!(matches!(
        next_request(fake),
        RequestBody::AchievementIcon(_)
    ));
    assert_eq!(u32_at(&wait_callback(pipe, 1109), 140), 0);

    // Another user's stats.
    let call = us!("RequestUserStats", fn(u64) -> u64, OTHER);
    assert_ne!(call, 0);
    assert!(matches!(next_request(fake), RequestBody::UserStats(r) if r.steam_id == OTHER));
    let done = wait_callback(pipe, 703);
    assert_eq!(done[..8], call.to_le_bytes());
    let mut result = [0u8; 20];
    let mut failed = true;
    // SAFETY: `result` holds 20 bytes.
    assert!(unsafe {
        crate::Steam_GetAPICallResult(
            pipe,
            call,
            result.as_mut_ptr().cast(),
            20,
            1101,
            &mut failed,
        )
    });
    assert!(!failed);
    assert_eq!(
        (u32_at(&result, 8), &result[12..20]),
        (1, &OTHER.to_le_bytes()[..])
    );
    assert_eq!(
        us!(
            "GetUserStat",
            fn(u64, *const c_char, *mut i32) -> u8,
            OTHER,
            c"NumGames".as_ptr(),
            &mut n
        ),
        1
    );
    assert_eq!(n, 9);
    assert_eq!(
        us!(
            "GetUserAchievementAndUnlockTime",
            fn(u64, *const c_char, *mut u8, *mut u32) -> u8,
            OTHER,
            win,
            &mut got,
            &mut time
        ),
        1
    );
    assert_eq!((got, time), (1, 1234));
    assert_eq!(
        us!(
            "GetUserStat",
            fn(u64, *const c_char, *mut i32) -> u8,
            5,
            c"NumGames".as_ptr(),
            &mut n
        ),
        0
    );

    // Global achievement percentages.
    let mut name = [0u8; 64];
    let mut pct = 0f32;
    assert_eq!(
        us!(
            "GetMostAchievedAchievementInfo",
            fn(*mut u8, u32, *mut f32, *mut u8) -> i32,
            name.as_mut_ptr(),
            64,
            &mut pct,
            &mut got
        ),
        -1
    );
    let call = us!("RequestGlobalAchievementPercentages", fn() -> u64);
    assert!(matches!(
        next_request(fake),
        RequestBody::GlobalAchievements(_)
    ));
    let done = wait_callback(pipe, 703);
    assert_eq!(done[..8], call.to_le_bytes());
    assert_eq!(u32_at(&done, 8), 1110);
    let mut ready = [0u8; 12];
    // SAFETY: `ready` holds 12 bytes.
    assert!(unsafe {
        crate::Steam_GetAPICallResult(pipe, call, ready.as_mut_ptr().cast(), 12, 1110, &mut failed)
    });
    assert_eq!(u32_at(&ready, 8), 1);
    assert_eq!(
        us!(
            "GetMostAchievedAchievementInfo",
            fn(*mut u8, u32, *mut f32, *mut u8) -> i32,
            name.as_mut_ptr(),
            64,
            &mut pct,
            &mut got
        ),
        0
    );
    assert_eq!((read_c(&name), pct, got), ("ACH_WIN_ONE_GAME", 75.5, 1));
    assert_eq!(
        us!(
            "GetNextMostAchievedAchievementInfo",
            fn(i32, *mut u8, u32, *mut f32, *mut u8) -> i32,
            0,
            name.as_mut_ptr(),
            64,
            &mut pct,
            &mut got
        ),
        1
    );
    assert_eq!((read_c(&name), pct, got), ("ACH_TRAVEL_FAR", 50.0, 0));
    assert_eq!(
        us!(
            "GetNextMostAchievedAchievementInfo",
            fn(i32, *mut u8, u32, *mut f32, *mut u8) -> i32,
            1,
            name.as_mut_ptr(),
            64,
            &mut pct,
            &mut got
        ),
        -1
    );
    assert_eq!(
        us!(
            "GetAchievementAchievedPercent",
            fn(*const c_char, *mut f32) -> u8,
            far,
            &mut pct
        ),
        1
    );
    assert_eq!(pct, 50.0);

    // Resetting stores the defaults.
    assert_eq!(us!("ResetAllStats", fn(u8) -> u8, 1), 1);
    let RequestBody::StoreStats(sent) = next_request(fake) else {
        panic!("no store")
    };
    assert!(sent.reset);
    assert_eq!(
        sent.values,
        [
            StatValue { id: 1, value: 0 },
            StatValue { id: 2, value: 0 },
            StatValue {
                id: 3,
                value: 1.5f32.to_bits()
            },
        ]
    );
    assert_eq!(u32_at(&wait_callback(pipe, 1102), 8), 1);
    assert_eq!(
        us!(
            "GetAchievement",
            fn(*const c_char, *mut u8) -> u8,
            win,
            &mut got
        ),
        1
    );
    assert_eq!(got, 0);
}

/// `ISteamRemoteStorage`'s local files (`isteamremotestorage.md`), with
/// the quota of 1000 bytes and 3 files the fake app has.
fn remote_storage_session(client: *mut c_void, user: i32, pipe: i32, user_dir: &Path) {
    let get = |version: &CStr| {
        call!(
            client,
            "SteamClient020",
            "GetISteamRemoteStorage",
            fn(i32, i32, *const c_char) -> *mut c_void,
            user,
            pipe,
            version.as_ptr()
        )
    };
    let rs = get(c"STEAMREMOTESTORAGE_INTERFACE_VERSION016");
    assert!(!rs.is_null());
    macro_rules! rs {
        ($method:literal, fn($($t:ty),*) -> $r:ty $(, $arg:expr)*) => {
            call!(rs, "STEAMREMOTESTORAGE_INTERFACE_VERSION016", $method, fn($($t),*) -> $r $(, $arg)*)
        };
    }
    let write = |name: &CStr, data: &[u8]| {
        rs!(
            "FileWrite",
            fn(*const c_char, *const u8, i32) -> u8,
            name.as_ptr(),
            data.as_ptr(),
            data.len() as i32
        ) == 1
    };

    assert!(write(c"save\\one.dat", b"0123456789"));
    assert_eq!(
        fs::read(user_dir.join("remote/save/one.dat")).unwrap(),
        b"0123456789"
    );
    assert!(!write(c"../escape", b"x"));
    assert!(!write(c"/abs", b"x"));
    assert_eq!(
        rs!(
            "FileExists",
            fn(*const c_char) -> u8,
            c"save/one.dat".as_ptr()
        ),
        1
    );
    assert_eq!(
        rs!(
            "GetFileSize",
            fn(*const c_char) -> i32,
            c"save/one.dat".as_ptr()
        ),
        10
    );
    assert!(
        rs!(
            "GetFileTimestamp",
            fn(*const c_char) -> i64,
            c"save/one.dat".as_ptr()
        ) > 0
    );
    let mut buf = [0u8; 4];
    assert_eq!(
        rs!(
            "FileRead",
            fn(*const c_char, *mut u8, i32) -> i32,
            c"save/one.dat".as_ptr(),
            buf.as_mut_ptr(),
            4
        ),
        4
    );
    assert_eq!(&buf, b"0123");
    assert_eq!(
        rs!(
            "FileRead",
            fn(*const c_char, *mut u8, i32) -> i32,
            c"missing".as_ptr(),
            buf.as_mut_ptr(),
            4
        ),
        0
    );

    // The asynchronous methods complete at once.
    let call = rs!(
        "FileWriteAsync",
        fn(*const c_char, *const u8, u32) -> u64,
        c"two".as_ptr(),
        b"abc".as_ptr(),
        3
    );
    assert_ne!(call, 0);
    assert_eq!(wait_callback(pipe, 703)[..8], call.to_le_bytes());
    let mut result = [0u8; 20];
    let mut failed = true;
    // SAFETY: `result` holds 20 bytes.
    assert!(unsafe {
        crate::Steam_GetAPICallResult(pipe, call, result.as_mut_ptr().cast(), 4, 1331, &mut failed)
    });
    assert!(!failed);
    assert_eq!(u32_at(&result, 0), 1);
    let call = rs!(
        "FileReadAsync",
        fn(*const c_char, u32, u32) -> u64,
        c"save/one.dat".as_ptr(),
        2,
        5
    );
    assert_ne!(call, 0);
    assert_eq!(wait_callback(pipe, 703)[..8], call.to_le_bytes());
    // SAFETY: `result` holds 20 bytes.
    assert!(unsafe {
        crate::Steam_GetAPICallResult(
            pipe,
            call,
            result.as_mut_ptr().cast(),
            20,
            1332,
            &mut failed,
        )
    });
    assert_eq!(result[..8], call.to_le_bytes());
    assert_eq!(
        (u32_at(&result, 8), u32_at(&result, 12), u32_at(&result, 16)),
        (1, 2, 5)
    );
    let mut five = [0u8; 5];
    assert_eq!(
        rs!(
            "FileReadAsyncComplete",
            fn(u64, *mut u8, u32) -> u8,
            call,
            five.as_mut_ptr(),
            5
        ),
        1
    );
    assert_eq!(&five, b"23456");
    assert_eq!(
        rs!(
            "FileReadAsyncComplete",
            fn(u64, *mut u8, u32) -> u8,
            call,
            five.as_mut_ptr(),
            5
        ),
        0
    );
    assert_eq!(
        rs!(
            "FileReadAsync",
            fn(*const c_char, u32, u32) -> u64,
            c"save/one.dat".as_ptr(),
            8,
            5
        ),
        0
    );

    // A stream, and one cancelled.
    let stream = rs!(
        "FileWriteStreamOpen",
        fn(*const c_char) -> u64,
        c"three".as_ptr()
    );
    assert_ne!(stream, u64::MAX);
    for chunk in [&b"de"[..], b"f"] {
        assert_eq!(
            rs!(
                "FileWriteStreamWriteChunk",
                fn(u64, *const u8, i32) -> u8,
                stream,
                chunk.as_ptr(),
                chunk.len() as i32
            ),
            1
        );
    }
    assert_eq!(
        rs!("FileExists", fn(*const c_char) -> u8, c"three".as_ptr()),
        0
    );
    assert_eq!(rs!("FileWriteStreamClose", fn(u64) -> u8, stream), 1);
    assert_eq!(fs::read(user_dir.join("remote/three")).unwrap(), b"def");
    let cancelled = rs!(
        "FileWriteStreamOpen",
        fn(*const c_char) -> u64,
        c"two".as_ptr()
    );
    assert_eq!(rs!("FileWriteStreamCancel", fn(u64) -> u8, cancelled), 1);
    assert_eq!(rs!("FileWriteStreamClose", fn(u64) -> u8, cancelled), 0);

    // The quota: bytes, then files.
    assert!(!write(c"two", &[0; 1000]));
    assert!(!write(c"four", b"x"));
    assert!(write(c"two", &[0; 900]));

    // The listing, sorted, without temporary files.
    assert_eq!(rs!("GetFileCount", fn() -> i32), 3);
    let mut size = 0i32;
    let names: Vec<(String, i32)> = (0..4)
        .map(|i| {
            size = -1;
            let n = str_at(rs!(
                "GetFileNameAndSize",
                fn(i32, *mut i32) -> *const c_char,
                i,
                &mut size
            ));
            (n.to_owned(), size)
        })
        .collect();
    assert_eq!(
        names,
        [
            ("save/one.dat".to_owned(), 10),
            ("three".to_owned(), 3),
            ("two".to_owned(), 900),
            (String::new(), -1)
        ]
    );
    let (mut total, mut available) = (0u64, 0u64);
    assert_eq!(
        rs!(
            "GetQuota",
            fn(*mut u64, *mut u64) -> u8,
            &mut total,
            &mut available
        ),
        1
    );
    assert_eq!((total, available), (1000, 87));
    let old = get(c"STEAMREMOTESTORAGE_INTERFACE_VERSION013");
    let (mut total, mut available) = ([0i32; 2], [0i32; 2]);
    call!(
        old,
        "STEAMREMOTESTORAGE_INTERFACE_VERSION013",
        "GetQuota",
        fn(*mut i32, *mut i32) -> u8,
        total.as_mut_ptr(),
        available.as_mut_ptr()
    );
    assert_eq!((total, available), ([1000, 0], [87, 0]));

    assert_eq!(
        rs!("FileDelete", fn(*const c_char) -> u8, c"two".as_ptr()),
        1
    );
    assert_eq!(
        rs!("FileDelete", fn(*const c_char) -> u8, c"two".as_ptr()),
        0
    );
    assert_eq!(
        rs!("FilePersisted", fn(*const c_char) -> u8, c"three".as_ptr()),
        0
    );
    assert_eq!(
        rs!("FileForget", fn(*const c_char) -> u8, c"three".as_ptr()),
        1
    );
    assert_eq!(rs!("IsCloudEnabledForApp", fn() -> u8), 0);
    assert_eq!(rs!("BeginFileWriteBatch", fn() -> u8), 1);
    assert_eq!(rs!("BeginFileWriteBatch", fn() -> u8), 0);
    assert_eq!(rs!("EndFileWriteBatch", fn() -> u8), 1);
    assert_eq!(rs!("EndFileWriteBatch", fn() -> u8), 0);
    let mut leftovers = Vec::new();
    for e in fs::read_dir(user_dir.join("remote")).unwrap() {
        leftovers.push(e.unwrap().file_name().into_string().unwrap());
    }
    leftovers.sort();
    assert_eq!(leftovers, ["save", "three"]);
}

/// `ISteamUser`'s tickets and auth sessions (`isteamuser.md` §4).
fn tickets_session(fake: &Fake, client: *mut c_void, user: i32, pipe: i32) {
    let get = |version: &CStr| {
        let obj = call!(
            client,
            "SteamClient020",
            "GetISteamUser",
            fn(i32, i32, *const c_char) -> *mut c_void,
            user,
            pipe,
            version.as_ptr()
        );
        assert!(!obj.is_null());
        obj
    };
    let old = get(c"SteamUser021");
    let new = get(c"SteamUser023");
    let mut buf = [0u8; 1024];
    let mut size = 99u32;

    // A session ticket, and its callback once Steam has it.
    let handle = call!(
        old,
        "SteamUser021",
        "GetAuthSessionTicket",
        fn(*mut u8, i32, *mut u32) -> u32,
        buf.as_mut_ptr(),
        1024,
        &mut size
    );
    assert_ne!(handle, 0);
    assert_eq!(size, 40);
    assert_eq!(buf[4..40], [2; 36]);
    let first_crc = u32_at(&buf, 0);
    assert!(
        matches!(next_request(fake), RequestBody::AuthTicket(r) if r.kind == 2 && r.max == 1024)
    );
    let ready = wait_callback(pipe, 163);
    assert_eq!((u32_at(&ready, 0), u32_at(&ready, 4)), (handle, 1));

    // Too small a buffer.
    assert_eq!(
        call!(
            old,
            "SteamUser021",
            "GetAuthSessionTicket",
            fn(*mut u8, i32, *mut u32) -> u32,
            buf.as_mut_ptr(),
            10,
            &mut size
        ),
        0
    );
    assert_eq!(size, 0);
    assert!(matches!(next_request(fake), RequestBody::AuthTicket(r) if r.max == 10));

    // Version 023 takes an identity, which is not used.
    let second = call!(
        new,
        "SteamUser023",
        "GetAuthSessionTicket",
        fn(*mut u8, i32, *mut u32, *const c_void) -> u32,
        buf.as_mut_ptr(),
        1024,
        &mut size,
        std::ptr::null()
    );
    assert!(second != 0 && second != handle);
    assert!(matches!(next_request(fake), RequestBody::AuthTicket(_)));
    assert_eq!(u32_at(&wait_callback(pipe, 163), 0), second);

    // A Web API ticket comes in its callback.
    let web = call!(
        new,
        "SteamUser023",
        "GetAuthTicketForWebApi",
        fn(*const c_char) -> u32,
        c"svc".as_ptr()
    );
    assert_ne!(web, 0);
    assert!(
        matches!(next_request(fake), RequestBody::AuthTicket(r) if r.kind == 5 && r.identity == "svc")
    );
    let w = wait_callback(pipe, 168);
    assert_eq!(w.len(), 2572);
    assert_eq!((u32_at(&w, 0), u32_at(&w, 4), u32_at(&w, 8)), (web, 1, 40));
    assert_eq!(w[16..52], [5; 36]);
    assert!(w[52..].iter().all(|&b| b == 0));

    // Cancelling tells the daemon which ticket; an unknown handle is
    // ignored.
    call!(old, "SteamUser021", "CancelAuthTicket", fn(u32) -> (), 999);
    call!(
        old,
        "SteamUser021",
        "CancelAuthTicket",
        fn(u32) -> (),
        handle
    );
    assert!(matches!(next_request(fake), RequestBody::CancelAuthTicket(r) if r.crc == first_crc));

    // A session with another user.
    let theirs = [7u8; 64];
    assert_eq!(
        call!(
            old,
            "SteamUser021",
            "BeginAuthSession",
            fn(*const u8, i32, u64) -> u32,
            theirs.as_ptr(),
            64,
            OTHER
        ),
        0
    );
    assert!(matches!(next_request(fake), RequestBody::BeginAuthSession(r) if r.ticket == theirs));
    let v = wait_callback(pipe, 143);
    assert_eq!(v.len(), 20);
    assert_eq!(v[..8], OTHER.to_le_bytes());
    assert_eq!(u32_at(&v, 8), 0);
    assert_eq!(v[12..20], OTHER.to_le_bytes());
    assert_eq!(
        call!(
            old,
            "SteamUser021",
            "BeginAuthSession",
            fn(*const u8, i32, u64) -> u32,
            std::ptr::null(),
            64,
            OTHER
        ),
        1
    );
    assert_eq!(
        call!(
            old,
            "SteamUser021",
            "UserHasLicenseForApp",
            fn(u64, u32) -> u32,
            OTHER,
            480
        ),
        0
    );
    assert!(matches!(next_request(fake), RequestBody::UserLicense(_)));
    call!(old, "SteamUser021", "EndAuthSession", fn(u64) -> (), OTHER);
    assert!(matches!(next_request(fake), RequestBody::EndAuthSession(r) if r.steam_id == OTHER));

    // An encrypted app ticket: the call's result, then the ticket.
    assert!(!call!(
        old,
        "SteamUser021",
        "GetEncryptedAppTicket",
        fn(*mut u8, i32, *mut u32) -> bool,
        buf.as_mut_ptr(),
        1024,
        &mut size
    ));
    let data = [1u8, 2, 3];
    let call = call!(
        old,
        "SteamUser021",
        "RequestEncryptedAppTicket",
        fn(*const u8, i32) -> u64,
        data.as_ptr(),
        3
    );
    assert_ne!(call, 0);
    assert!(matches!(next_request(fake), RequestBody::EncryptedAppTicket(r) if r.userdata == data));
    assert_eq!(wait_callback(pipe, 703)[..8], call.to_le_bytes());
    let mut result = [0u8; 4];
    let mut failed = true;
    // SAFETY: `result` holds 4 bytes.
    assert!(unsafe {
        crate::Steam_GetAPICallResult(pipe, call, result.as_mut_ptr().cast(), 4, 154, &mut failed)
    });
    assert!(!failed);
    assert_eq!(u32::from_le_bytes(result), 1);
    assert!(call!(
        old,
        "SteamUser021",
        "GetEncryptedAppTicket",
        fn(*mut u8, i32, *mut u32) -> bool,
        buf.as_mut_ptr(),
        1024,
        &mut size
    ));
    assert_eq!((size, &buf[..3]), (3, &[3u8, 2, 1][..]));
    assert!(!call!(
        old,
        "SteamUser021",
        "GetEncryptedAppTicket",
        fn(*mut u8, i32, *mut u32) -> bool,
        buf.as_mut_ptr(),
        2,
        &mut size
    ));
}
