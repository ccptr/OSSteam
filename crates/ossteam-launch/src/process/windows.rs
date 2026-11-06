// SPDX-License-Identifier: GPL-2.0-or-later
//! Game processes on Windows: a job object holds the tree (`launch.md`
//! §4 and §5). Type-checked only; not yet run on Windows.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, HWND, INVALID_HANDLE_VALUE, LPARAM,
    SetHandleInformation,
};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_ASSOCIATE_COMPLETION_PORT,
    JobObjectAssociateCompletionPortInformation, JobObjectBasicProcessIdList,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    GetExitCodeProcess, INFINITE, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOW, TerminateProcess,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};
use windows_sys::core::BOOL;

use super::{Exit, Spec};

/// `JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO`: the job's last process exited.
const ACTIVE_PROCESS_ZERO: u32 = 4;

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

/// A handle closed when the last owner drops it.
struct Owned(HANDLE);

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

pub struct Game {
    pid: u32,
    process: Owned,
    job: Arc<Owned>,
    port: Owned,
    done: Arc<AtomicBool>,
}

impl Game {
    pub fn spawn(spec: &Spec) -> io::Result<Self> {
        let log = spec.open_log()?;
        let null = File::open("NUL")?;
        for h in [log.as_raw_handle(), null.as_raw_handle()] {
            // SAFETY: valid handles owned by the files above.
            if unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let app = wide(&spec.executable);
        let mut line = wide(&spec.command_line);
        let cwd = wide(&spec.working_dir);
        let mut env = environment_block(&spec.env);

        // SAFETY: the job and port are checked and owned at once.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Arc::new(Owned(job));
        // SAFETY: as above.
        let port =
            unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1) };
        if port.is_null() {
            return Err(io::Error::last_os_error());
        }
        let port = Owned(port);
        let assoc = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
            CompletionKey: std::ptr::null_mut(),
            CompletionPort: port.0,
        };
        // SAFETY: `assoc` is the structure this class takes, by size.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectAssociateCompletionPortInformation,
                (&raw const assoc).cast(),
                size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: zeroed is a valid STARTUPINFOW apart from `cb`.
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput = null.as_raw_handle();
        si.hStdOutput = log.as_raw_handle();
        si.hStdError = log.as_raw_handle();
        // SAFETY: zeroed is valid for an out parameter.
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: every string is NUL-terminated and outlives the call;
        // `line` is mutable as CreateProcessW requires; the environment
        // block is UTF-16 as the flag says.
        let ok = unsafe {
            CreateProcessW(
                app.as_ptr(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | CREATE_NEW_PROCESS_GROUP,
                env.as_mut_ptr().cast::<c_void>(),
                cwd.as_ptr(),
                &si,
                &mut pi,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let process = Owned(pi.hProcess);
        let thread = Owned(pi.hThread);
        // In the job before it runs, so its children are too.
        // SAFETY: valid handles.
        if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
            let e = io::Error::last_os_error();
            // SAFETY: the suspended process is ours to end.
            unsafe { TerminateProcess(process.0, 1) };
            return Err(e);
        }
        // SAFETY: the primary thread's handle.
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            let e = io::Error::last_os_error();
            // SAFETY: as above.
            unsafe { TerminateJobObject(job.0, 1) };
            return Err(e);
        }
        Ok(Self {
            pid: pi.dwProcessId,
            process,
            job,
            port,
            done: Arc::default(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn stopper(&self) -> Stopper {
        Stopper {
            job: self.job.clone(),
            done: self.done.clone(),
        }
    }

    pub fn wait(self) -> io::Result<Exit> {
        loop {
            let mut code = 0u32;
            let mut key = 0usize;
            let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
            // SAFETY: out parameters for the port's next message.
            let ok = unsafe {
                GetQueuedCompletionStatus(
                    self.port.0,
                    &mut code,
                    &mut key,
                    &mut overlapped,
                    INFINITE,
                )
            };
            if ok == 0 {
                self.done.store(true, Ordering::SeqCst);
                return Err(io::Error::last_os_error());
            }
            if code == ACTIVE_PROCESS_ZERO {
                break;
            }
        }
        self.done.store(true, Ordering::SeqCst);
        let mut code = 0u32;
        // SAFETY: the process handle is valid.
        if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Exit::Code(i64::from(code)))
    }
}

/// The inherited environment with `vars` set, as a sorted UTF-16 block.
fn environment_block(vars: &[(String, String)]) -> Vec<u16> {
    let mut all: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !vars.iter().any(|(n, _)| k.eq_ignore_ascii_case(n)))
        .collect();
    all.extend(
        vars.iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
    );
    all.sort_by_key(|(k, _)| k.to_ascii_uppercase());
    let mut block = Vec::new();
    for (k, v) in all {
        block.extend(k.encode_wide());
        block.push(u16::from(b'='));
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

#[derive(Clone)]
pub struct Stopper {
    job: Arc<Owned>,
    done: Arc<AtomicBool>,
}

impl Stopper {
    pub fn stop(&self, timeout: Duration) {
        if self.done.load(Ordering::SeqCst) {
            return;
        }
        if close_windows(&self.pids()) == 0 {
            // Nothing to ask.
            // SAFETY: the job handle is valid while `self` holds it.
            unsafe { TerminateJobObject(self.job.0, 1) };
            return;
        }
        let s = self.clone();
        std::thread::Builder::new()
            .name("game-stop".into())
            .spawn(move || {
                let deadline = Instant::now() + timeout;
                while Instant::now() < deadline {
                    if s.done.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if !s.done.load(Ordering::SeqCst) {
                    tracing::info!("the game did not close in time; terminating its job");
                    // SAFETY: as above.
                    unsafe { TerminateJobObject(s.job.0, 1) };
                }
            })
            .map(drop)
            .unwrap_or_else(|e| tracing::error!("starting the stop timer failed: {e}"));
    }

    pub fn contains(&self, pid: u32) -> bool {
        !self.done.load(Ordering::SeqCst) && self.pids().contains(&pid)
    }

    /// The processes in the job.
    fn pids(&self) -> Vec<u32> {
        // JOBOBJECT_BASIC_PROCESS_ID_LIST: two u32 counts, then the ids as
        // ULONG_PTRs. A usize buffer keeps them aligned.
        const IDS: usize = 8 / size_of::<usize>();
        let mut cap = 64usize;
        loop {
            let mut buf = vec![0usize; IDS + cap];
            // SAFETY: the buffer is as large as the size passed.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.job.0,
                    JobObjectBasicProcessIdList,
                    buf.as_mut_ptr().cast(),
                    (buf.len() * size_of::<usize>()) as u32,
                    std::ptr::null_mut(),
                )
            };
            // SAFETY: the first 8 bytes are the two counts.
            let (assigned, listed) = unsafe {
                let counts = buf.as_ptr().cast::<u32>();
                (*counts as usize, *counts.add(1) as usize)
            };
            if ok != 0 && listed >= assigned {
                return buf[IDS..IDS + listed.min(cap)]
                    .iter()
                    .map(|&p| p as u32)
                    .collect();
            }
            if cap >= 65_536 {
                return Vec::new();
            }
            cap = (assigned + 16).max(cap * 2);
        }
    }
}

struct Search<'a> {
    pids: &'a [u32],
    posted: usize,
}

/// Posts `WM_CLOSE` to each top-level window of these processes and
/// returns how many there were.
fn close_windows(pids: &[u32]) -> usize {
    let mut s = Search { pids, posted: 0 };
    // SAFETY: the callback gets the pointer to `s` back, which lives
    // across the call.
    unsafe { EnumWindows(Some(each_window), (&raw mut s) as LPARAM) };
    s.posted
}

unsafe extern "system" fn each_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `Search` passed to EnumWindows.
    let s = unsafe { &mut *(lparam as *mut Search<'_>) };
    let mut pid = 0u32;
    // SAFETY: a window handle from EnumWindows.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    // SAFETY: as above.
    let owned = !unsafe { GetWindow(hwnd, GW_OWNER) }.is_null();
    if s.pids.contains(&pid) && !owned {
        // SAFETY: as above.
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        s.posted += 1;
    }
    1
}
