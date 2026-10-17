// SPDX-License-Identifier: GPL-2.0-or-later
//! Install script steps on Windows: the registry, processes and the
//! elevated helper (`install-scripts.md` §2, §4).

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use ossteam_types::CancelToken;
use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HANDLE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, KEY_WOW64_32KEY, REG_DWORD,
    REG_OPTION_NON_VOLATILE, RRF_RT_ANY, RRF_RT_REG_DWORD, RRF_SUBKEY_WOW6432KEY, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetCurrentProcess, GetExitCodeProcess, IsWow64Process, PROCESS_INFORMATION,
    STARTUPINFOW, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_Documents, FOLDERID_PublicDocuments, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW, SHGetKnownFolderPath, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};
use windows_sys::core::GUID;

use super::{HelperRequest, Key, Outcome, Step, System, ValueRef};

/// The longest command line Windows takes, in UTF-16 units, with room for
/// the program's path.
const MAX_COMMAND_LINE: usize = 32_767 - 1024;
/// How often a wait looks at the job's cancel token.
const POLL_MS: u32 = 250;

pub struct Windows;

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

fn os_error(code: u32) -> String {
    if code == NO_PROCESS {
        return "it started no process to wait for".into();
    }
    std::io::Error::from_raw_os_error(code as i32).to_string()
}

/// What [`shell_execute`] fails with when the shell handed the file to a
/// program already running.
const NO_PROCESS: u32 = ERROR_SUCCESS;

/// The hive's handle, and the flags that put `HKEY_LOCAL_MACHINE\Software`
/// in the 32-bit view the official client writes to (§2.1).
fn root(k: &Key) -> (HKEY, u32, u32) {
    if k.machine {
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, RRF_SUBKEY_WOW6432KEY)
    } else {
        (HKEY_CURRENT_USER, 0, 0)
    }
}

/// The value name, or null for the key's default value.
fn name(v: &ValueRef) -> Option<Vec<u16>> {
    v.name.as_ref().map(wide)
}

fn ptr(w: &Option<Vec<u16>>) -> *const u16 {
    w.as_ref().map_or(std::ptr::null(), |w| w.as_ptr())
}

fn set_dword(v: &ValueRef, n: u32) -> Result<(), String> {
    let (hive, view, _) = root(&v.key);
    let path = wide(&v.key.path);
    let name = name(v);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: the strings are NUL-terminated and outlive the calls; `key`
    // is closed once.
    unsafe {
        let r = RegCreateKeyExW(
            hive,
            path.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | view,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if r != ERROR_SUCCESS {
            return Err(os_error(r));
        }
        let bytes = n.to_le_bytes();
        let r = RegSetValueExW(key, ptr(&name), 0, REG_DWORD, bytes.as_ptr(), 4);
        RegCloseKey(key);
        if r != ERROR_SUCCESS {
            return Err(os_error(r));
        }
    }
    Ok(())
}

fn delete(v: &ValueRef) -> Result<(), String> {
    let (hive, view, _) = root(&v.key);
    let path = wide(&v.key.path);
    let name = name(v);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: as in `set_dword`.
    unsafe {
        let r = RegOpenKeyExW(hive, path.as_ptr(), 0, KEY_SET_VALUE | view, &mut key);
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if r != ERROR_SUCCESS {
            return Err(os_error(r));
        }
        let r = RegDeleteValueW(key, ptr(&name));
        RegCloseKey(key);
        if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
            return Err(os_error(r));
        }
    }
    Ok(())
}

/// Waits for a process, looking at `cancel` as it does, and closes its
/// handle. `None` when `cancel` ended the wait; the process runs on.
fn wait(process: HANDLE, cancel: &CancelToken) -> Option<Result<u32, String>> {
    // SAFETY: `process` is a process handle this module owns.
    unsafe {
        let out = loop {
            match WaitForSingleObject(process, POLL_MS) {
                WAIT_OBJECT_0 => {
                    let mut code = 0u32;
                    break Some(if GetExitCodeProcess(process, &mut code) != 0 {
                        Ok(code)
                    } else {
                        Err(os_error(GetLastError()))
                    });
                }
                WAIT_TIMEOUT if !cancel.is_cancelled() => {}
                WAIT_TIMEOUT => break None,
                _ => break Some(Err(os_error(GetLastError()))),
            }
        };
        CloseHandle(process);
        out
    }
}

/// Starts a program with `CreateProcess`, and with `ShellExecuteEx` if
/// that fails (§2.2). It starts in its own directory (*unverified*).
fn start(program: &str, arguments: &str) -> Result<HANDLE, String> {
    let dir = Path::new(program).parent().map(wide);
    let mut command = wide(format!("\"{program}\" {arguments}"));
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the strings are NUL-terminated and outlive the call; the
    // command line is writable, as CreateProcessW needs.
    let created = unsafe {
        CreateProcessW(
            std::ptr::null(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            ptr(&dir),
            &si,
            &mut pi,
        )
    };
    if created != 0 {
        // SAFETY: the thread handle is ours and not used.
        unsafe { CloseHandle(pi.hThread) };
        return Ok(pi.hProcess);
    }
    let first = unsafe { GetLastError() };
    let file = wide(program);
    let params = wide(arguments);
    shell_execute(None, &file, &params, ptr(&dir), SW_SHOWNORMAL).map_err(|e| {
        format!(
            "CreateProcess: {}; ShellExecuteEx: {}",
            os_error(first),
            os_error(e)
        )
    })
}

/// `ShellExecuteEx` with a process handle to wait on; the error code when
/// it fails.
fn shell_execute(
    verb: Option<&[u16]>,
    file: &[u16],
    params: &[u16],
    dir: *const u16,
    show: i32,
) -> Result<HANDLE, u32> {
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.map_or(std::ptr::null(), |v| v.as_ptr());
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.lpDirectory = dir;
    info.nShow = show;
    // SAFETY: the strings are NUL-terminated and outlive the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(unsafe { GetLastError() });
    }
    if info.hProcess.is_null() {
        // Handed to a running program; there is nothing to wait for.
        return Err(NO_PROCESS);
    }
    Ok(info.hProcess)
}

fn run(
    program: &str,
    arguments: &str,
    has_run: Option<&ValueRef>,
    cancel: &CancelToken,
) -> Outcome {
    if cancel.is_cancelled() {
        return Outcome::NotRun;
    }
    let process = match start(program, arguments) {
        Ok(p) => p,
        Err(e) => return Outcome::Failed(e),
    };
    match wait(process, cancel) {
        None => Outcome::Failed("the job stopped while it ran".into()),
        Some(Err(e)) => Outcome::Failed(e),
        Some(Ok(0)) => {
            if let Some(v) = has_run
                && let Err(e) = set_dword(v, 1)
            {
                // It ran, so it counts; it runs again at the next start.
                tracing::warn!("recording that {program} ran in {v}: {e}");
            }
            Outcome::Done
        }
        Some(Ok(code)) => Outcome::Failed(format!("exit code {code}")),
    }
}

fn known_folder(id: &GUID) -> Option<String> {
    let mut path: *mut u16 = std::ptr::null_mut();
    // SAFETY: on success `path` is a NUL-terminated string this frees.
    unsafe {
        let hr = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut path);
        let out = (hr >= 0 && !path.is_null()).then(|| {
            let len = (0..).take_while(|&i| *path.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(path, len))
        });
        CoTaskMemFree(path as *const c_void);
        out
    }
}

impl System for Windows {
    fn dword(&self, v: &ValueRef) -> Option<u32> {
        let (hive, _, view) = root(&v.key);
        let path = wide(&v.key.path);
        let name = name(v);
        let mut n = 0u32;
        let mut size = 4u32;
        // SAFETY: `n` has room for the `size` bytes asked for.
        let r = unsafe {
            RegGetValueW(
                hive,
                path.as_ptr(),
                ptr(&name),
                RRF_RT_REG_DWORD | view,
                std::ptr::null_mut(),
                &mut n as *mut u32 as *mut c_void,
                &mut size,
            )
        };
        (r == ERROR_SUCCESS).then_some(n)
    }

    fn exists(&self, v: &ValueRef) -> bool {
        let (hive, _, view) = root(&v.key);
        let path = wide(&v.key.path);
        let name = name(v);
        // SAFETY: no data is asked for.
        let r = unsafe {
            RegGetValueW(
                hive,
                path.as_ptr(),
                ptr(&name),
                RRF_RT_ANY | view,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        r == ERROR_SUCCESS
    }

    fn apply(&self, step: &Step, cancel: &CancelToken) -> Outcome {
        match step {
            Step::Delete(v) => match delete(v) {
                Ok(()) => Outcome::Done,
                Err(e) => Outcome::Failed(e),
            },
            Step::Run {
                program,
                arguments,
                has_run,
            } => run(program, arguments, has_run.as_ref(), cancel),
        }
    }

    fn apply_elevated(&self, steps: &[Step], cancel: &CancelToken) -> Result<Vec<Outcome>, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let result = std::env::temp_dir().join(format!(
            "ossteamd-script-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let request = HelperRequest {
            steps: steps.to_vec(),
            result: result.clone(),
        }
        .encode();
        if request.len() > MAX_COMMAND_LINE {
            return Err(format!(
                "{} steps do not fit on the helper's command line",
                steps.len()
            ));
        }
        let exe = std::env::current_exe().map_err(|e| format!("finding ossteamd: {e}"))?;
        let verb = wide("runas");
        let file = wide(&exe);
        let params = wide(format!("--script-helper {request}"));
        let _ = std::fs::remove_file(&result);
        let process = shell_execute(Some(&verb), &file, &params, std::ptr::null(), SW_HIDE)
            .map_err(|e| {
                if e == ERROR_CANCELLED {
                    "the user declined the UAC prompt".to_owned()
                } else {
                    format!("starting the helper: {}", os_error(e))
                }
            })?;
        let code = match wait(process, cancel) {
            None => return Err("the job stopped while the helper ran".into()),
            Some(r) => r?,
        };
        let read = std::fs::read(&result);
        let _ = std::fs::remove_file(&result);
        let bytes =
            read.map_err(|e| format!("the helper exited with {code} and no results: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("the helper's results: {e}"))
    }

    fn is_64bit(&self) -> bool {
        if cfg!(target_pointer_width = "64") {
            return true;
        }
        let mut wow = 0;
        // SAFETY: the pseudo handle needs no closing.
        unsafe { IsWow64Process(GetCurrentProcess(), &mut wow) != 0 && wow != 0 }
    }

    /// *Unverified:* the names beyond `WinXP` and `Win7` (§2.2).
    fn os_type(&self) -> String {
        let mut v: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        v.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        // SAFETY: `v` is the size it says.
        unsafe { RtlGetVersion(&mut v) };
        match (v.dwMajorVersion, v.dwMinorVersion) {
            (5, _) => "WinXP",
            (6, 0) => "WinVista",
            (6, 1) => "Win7",
            (6, 2) => "Win8",
            (6, 3) => "Win81",
            (10, _) if v.dwBuildNumber >= 22_000 => "Win11",
            _ => "Win10",
        }
        .to_owned()
    }

    fn var(&self, name: &str) -> Option<String> {
        match name {
            "WINDIR" => std::env::var("WINDIR")
                .or_else(|_| std::env::var("SystemRoot"))
                .ok(),
            "APPDATA" | "LOCALAPPDATA" => std::env::var(name).ok(),
            "USER_MYDOCS" => known_folder(&FOLDERID_Documents),
            "COMMON_MYDOCS" => known_folder(&FOLDERID_PublicDocuments),
            _ => None,
        }
    }
}
