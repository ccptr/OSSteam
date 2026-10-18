// SPDX-License-Identifier: GPL-2.0-or-later
//! A named pipe per user, opened for overlapped I/O so that one thread
//! can read while another writes: synchronous I/O on one pipe handle is
//! serialised, and a pending read would block every write.

use std::io;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{GetNamedPipeServerProcessId, WaitNamedPipeW};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::{Address, Connection};

/// How long to wait for a busy pipe.
const BUSY_WAIT_MS: u32 = 2000;

struct Handle(HANDLE);

/// The pipe and whether it was closed, shared by both directions.
struct Shared {
    pipe: Handle,
    closed: AtomicBool,
}

impl Shared {
    fn cancel(&self) {
        // SAFETY: the handle is open; cancelling no operation is harmless.
        unsafe { CancelIoEx(self.pipe.0, ptr::null()) };
    }
}

// SAFETY: a kernel handle may be used from any thread.
unsafe impl Send for Handle {}
// SAFETY: as above; overlapped operations on it are independent.
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned open by a Win32 call and is
        // closed exactly once here.
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn locate() -> io::Result<Address> {
    Ok(Address {
        name: format!("ossteam-game-{}", current_user_sid()?),
    })
}

pub fn connect(addr: &Address) -> io::Result<Connection> {
    let path = wide(&format!(r"\\.\pipe\{}", addr.name));
    let mut waited = false;
    let pipe = loop {
        // SAFETY: `path` is NUL-terminated; the other arguments are plain
        // values or null.
        let h = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            break Handle(h);
        }
        // SAFETY: no preconditions.
        let err = unsafe { GetLastError() };
        // SAFETY: `path` is NUL-terminated.
        if err == ERROR_PIPE_BUSY
            && !waited
            && unsafe { WaitNamedPipeW(path.as_ptr(), BUSY_WAIT_MS) } != 0
        {
            waited = true;
            continue;
        }
        return Err(io::Error::from_raw_os_error(err as i32));
    };
    let mut server = 0u32;
    // SAFETY: `pipe` is an open pipe handle; `server` is a valid out-pointer.
    if unsafe { GetNamedPipeServerProcessId(pipe.0, &mut server) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if process_user_sid(server)? != current_user_sid()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the process serving {} runs as another user", addr.name),
        ));
    }
    let shared = Arc::new(Shared {
        pipe,
        closed: AtomicBool::new(false),
    });
    let closer = Arc::clone(&shared);
    Ok(Connection {
        reader: Box::new(Pipe::new(Arc::clone(&shared))?),
        writer: Box::new(Pipe::new(shared)?),
        closer: Box::new(move || {
            closer.closed.store(true, Ordering::SeqCst);
            closer.cancel();
        }),
    })
}

/// One direction of the pipe, with its own completion event.
struct Pipe {
    shared: Arc<Shared>,
    event: Handle,
}

impl Pipe {
    fn new(shared: Arc<Shared>) -> io::Result<Self> {
        // SAFETY: a manual-reset event, unnamed, without security
        // attributes.
        let e = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if e.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            shared,
            event: Handle(e),
        })
    }

    /// Runs one overlapped read or write and waits for it. A broken pipe
    /// reads as the end of the stream.
    fn io(&mut self, buf: *mut u8, len: usize, write: bool) -> io::Result<usize> {
        if self.shared.closed.load(Ordering::SeqCst) {
            return closed(write);
        }
        let pipe = self.shared.pipe.0;
        let len = len.min(u32::MAX as usize) as u32;
        // SAFETY: OVERLAPPED is plain data; zero is its initial state.
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = self.event.0;
        // SAFETY: `buf` holds `len` bytes and, like `ov`, outlives the
        // operation, which completes before this function returns.
        let ok = unsafe {
            if write {
                WriteFile(pipe, buf, len, ptr::null_mut(), &mut ov)
            } else {
                ReadFile(pipe, buf, len, ptr::null_mut(), &mut ov)
            }
        };
        if ok == 0 {
            // SAFETY: no preconditions.
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                return broken(err, write);
            }
            // The closer cancels what is pending when it runs; an
            // operation that started after it checked is cancelled here.
            if self.shared.closed.load(Ordering::SeqCst) {
                self.shared.cancel();
            }
        }
        let mut n = 0u32;
        // SAFETY: `ov` belongs to the operation started above; waiting
        // for it is what keeps `buf` borrowed long enough.
        if unsafe { GetOverlappedResult(pipe, &ov, &mut n, 1) } == 0 {
            if self.shared.closed.load(Ordering::SeqCst) {
                return closed(write);
            }
            // SAFETY: no preconditions.
            return broken(unsafe { GetLastError() }, write);
        }
        Ok(n as usize)
    }
}

/// What I/O on a closed connection returns: the end of the stream for
/// reads.
fn closed(write: bool) -> io::Result<usize> {
    if write {
        Err(io::ErrorKind::BrokenPipe.into())
    } else {
        Ok(0)
    }
}

fn broken(err: u32, write: bool) -> io::Result<usize> {
    if err == ERROR_BROKEN_PIPE && !write {
        Ok(0)
    } else {
        Err(io::Error::from_raw_os_error(err as i32))
    }
}

impl io::Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.io(buf.as_mut_ptr(), buf.len(), false)
    }
}

impl io::Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // WriteFile does not write through the pointer.
        self.io(buf.as_ptr().cast_mut(), buf.len(), true)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The string SID (`S-1-5-21-...`) of the user this process runs as.
pub fn current_user_sid() -> io::Result<String> {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no
    // closing.
    token_user_sid(unsafe { GetCurrentProcess() })
}

/// The string SID of the user that process `pid` runs as.
fn process_user_sid(pid: u32) -> io::Result<String> {
    // SAFETY: plain Win32 call; a null result is handled below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(h);
    token_user_sid(process.0)
}

fn token_user_sid(process: HANDLE) -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `token` is a valid out-pointer.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut len = 0u32;
    // SAFETY: a size query with a null buffer; it fails with
    // ERROR_INSUFFICIENT_BUFFER and sets `len`.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut len) };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    // u64 elements keep the buffer aligned for TOKEN_USER.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes.
    let ok =
        unsafe { GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: GetTokenInformation(TokenUser) filled `buf` with a TOKEN_USER.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut wide: *mut u16 = ptr::null_mut();
    // SAFETY: `sid` points into `buf`, which outlives this call; `wide` is
    // a valid out-pointer.
    if unsafe { ConvertSidToStringSidW(sid, &mut wide) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success `wide` is a NUL-terminated string allocated with
    // LocalAlloc, read here and then freed exactly once.
    let s = unsafe {
        let len = (0..).take_while(|&i| *wide.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
        LocalFree(wide.cast());
        s
    };
    Ok(s)
}
