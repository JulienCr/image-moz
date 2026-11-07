//! Single instance per user session: a named mutex elects the primary, and later launches
//! hand their argument to it over a named pipe before exiting.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GENERIC_WRITE, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_NONE, OPEN_EXISTING,
    PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::PCWSTR;

/// What a launch asks of the running instance: open this path, or (`None`) just come forward.
pub type Forwarded = Option<PathBuf>;

type Wake = Arc<dyn Fn() + Send + Sync>;

const MAGIC: &[u8; 4] = b"IMZ1";
/// A Windows path is at most 32 767 UTF-16 units; anything bigger is not from us.
const MAX_MESSAGE: usize = MAGIC.len() + 2 * 32_768;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("not an Image Moz message")]
    Magic,
    #[error("payload is not whole UTF-16 units")]
    Length,
}

pub fn encode(arg: Option<&Path>) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    if let Some(path) = arg {
        bytes.extend(path.as_os_str().encode_wide().flat_map(u16::to_le_bytes));
    }
    bytes
}

pub fn decode(bytes: &[u8]) -> Result<Forwarded, ProtocolError> {
    let payload = bytes.strip_prefix(MAGIC.as_slice()).ok_or(ProtocolError::Magic)?;
    if payload.len() % 2 != 0 {
        return Err(ProtocolError::Length);
    }
    let units: Vec<u16> = payload.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Ok((!units.is_empty()).then(|| PathBuf::from(OsString::from_wide(&units))))
}

/// The kernel object names; the mutex is session-local, the pipe namespace is machine-wide.
#[derive(Clone, Debug)]
pub struct Names {
    mutex: String,
    pipe: String,
}

impl Names {
    pub fn for_current_user() -> Self {
        Self::new(&std::env::var("USERNAME").unwrap_or_default())
    }

    fn new(scope: &str) -> Self {
        let scope: String = scope.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).collect();
        Self {
            mutex: format!(r"Local\ImageMoz-{scope}"),
            pipe: format!(r"\\.\pipe\ImageMoz-{scope}"),
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

/// Owns the right to be the running instance, and receives what later launches forward.
pub struct Primary {
    _mutex: OwnedHandle,
    incoming: Receiver<Forwarded>,
    wake: Arc<OnceLock<Wake>>,
    /// HWND of the window a forwarded launch should raise; 0 until the UI names one.
    target: Arc<AtomicIsize>,
}

impl Primary {
    /// Called once the UI exists: forwarded messages then repaint it.
    pub fn set_wake(&self, wake: Wake) {
        let _ = self.wake.set(wake);
    }

    /// Names the window a forwarded launch raises, from the thread that receives it.
    pub fn set_target_window(&self, hwnd: isize) {
        self.target.store(hwnd, Ordering::Relaxed);
    }

    pub fn try_iter(&self) -> Vec<Forwarded> {
        self.incoming.try_iter().collect()
    }
}

pub enum Acquired {
    First(Primary),
    Existing,
}

fn create_pipe(name: &[u16], first: bool) -> io::Result<OwnedHandle> {
    let open_mode = if first { PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_INBOUND };
    let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    let handle = unsafe {
        CreateNamedPipeW(PCWSTR(name.as_ptr()), open_mode, mode, PIPE_UNLIMITED_INSTANCES, 4096, 4096, 0, None)
    };
    if handle.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

/// Claims the instance mutex. The first launch also opens the pipe and starts serving it.
pub fn acquire(names: &Names) -> io::Result<Acquired> {
    let mutex = unsafe { CreateMutexW(None, false, PCWSTR(wide(&names.mutex).as_ptr())) }.map_err(io::Error::from)?;
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = unsafe { OwnedHandle::from_raw_handle(mutex.0) };
    if already {
        return Ok(Acquired::Existing);
    }
    let pipe_name = wide(&names.pipe);
    let first = create_pipe(&pipe_name, true)?;
    let (tx, incoming) = crossbeam_channel::unbounded();
    let wake: Arc<OnceLock<Wake>> = Arc::new(OnceLock::new());
    let target = Arc::new(AtomicIsize::new(0));
    let server = Server { tx, wake: wake.clone(), target: target.clone() };
    std::thread::Builder::new().name("instance-pipe".into()).spawn(move || serve(first, &pipe_name, &server))?;
    Ok(Acquired::First(Primary { _mutex: mutex, incoming, wake, target }))
}

/// What the pipe threads share with the UI.
#[derive(Clone)]
struct Server {
    tx: Sender<Forwarded>,
    wake: Arc<OnceLock<Wake>>,
    target: Arc<AtomicIsize>,
}

fn serve(first: OwnedHandle, name: &[u16], server: &Server) {
    let mut pipe = first;
    loop {
        // A client that connected and closed before we got here is reported as NO_DATA, but its bytes are still queued.
        let connected = match unsafe { ConnectNamedPipe(raw(&pipe), None) } {
            Ok(()) => true,
            Err(e) => [ERROR_PIPE_CONNECTED, ERROR_NO_DATA].iter().any(|code| e.code() == code.to_hresult()),
        };
        let ready = pipe;
        pipe = match create_pipe(name, false) {
            Ok(next) => next,
            Err(e) => {
                log::error!("cannot reopen the instance pipe: {e}");
                if connected {
                    handle_client(&ready, server);
                }
                return;
            }
        };
        if connected {
            let server = server.clone();
            let spawned =
                std::thread::Builder::new().name("instance-client".into()).spawn(move || handle_client(&ready, &server));
            if let Err(e) = spawned {
                log::warn!("cannot serve a forwarded launch: {e}");
            }
        }
    }
}

/// Reads one message up to the client closing its end, and passes it on.
fn handle_client(pipe: &OwnedHandle, server: &Server) {
    let mut message = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let mut read = 0u32;
        match unsafe { ReadFile(raw(pipe), Some(&mut chunk), Some(&mut read), None) } {
            Ok(()) if read > 0 => message.extend_from_slice(&chunk[..read as usize]),
            Ok(()) => break,
            Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => break,
            Err(e) => {
                log::warn!("cannot read a forwarded launch: {e}");
                return;
            }
        }
        if message.len() > MAX_MESSAGE {
            log::warn!("dropping an oversized forwarded launch");
            return;
        }
    }
    let _ = unsafe { DisconnectNamedPipe(raw(pipe)) };
    match decode(&message) {
        Ok(forwarded) => {
            if server.tx.send(forwarded).is_ok() {
                match server.target.load(Ordering::Relaxed) {
                    0 => crate::platform::raise_any_own_window(),
                    hwnd => crate::platform::raise_window(hwnd),
                }
                if let Some(wake) = server.wake.get() {
                    wake();
                }
            }
        }
        Err(e) => log::warn!("dropping a forwarded launch: {e}"),
    }
}

/// Hands `arg` to the running instance, retrying while its pipe is busy or not yet open.
pub fn forward(names: &Names, arg: Option<&Path>) -> io::Result<()> {
    forward_within(names, arg, CONNECT_TIMEOUT)
}

fn forward_within(names: &Names, arg: Option<&Path>, timeout: Duration) -> io::Result<()> {
    let name = wide(&names.pipe);
    let deadline = Instant::now() + timeout;
    let pipe = loop {
        let opened = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        match opened {
            Ok(handle) => break unsafe { OwnedHandle::from_raw_handle(handle.0) },
            Err(e) if Instant::now() >= deadline => return Err(e.into()),
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                let _ = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 200) };
            }
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.into()),
        }
    };
    // The server needs the right to take the foreground, which only a foreground-eligible process can grant.
    let mut server_pid = 0u32;
    if unsafe { GetNamedPipeServerProcessId(raw(&pipe), &mut server_pid) }.is_ok() {
        let _ = unsafe { AllowSetForegroundWindow(server_pid) };
    }
    let mut remaining = encode(arg);
    while !remaining.is_empty() {
        let mut written = 0u32;
        unsafe { WriteFile(raw(&pipe), Some(&remaining), Some(&mut written), None) }.map_err(io::Error::from)?;
        remaining.drain(..written as usize);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn unique_names() -> Names {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Names::new(&format!("test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }

    fn first(names: &Names) -> Primary {
        match acquire(names).unwrap() {
            Acquired::First(primary) => primary,
            Acquired::Existing => panic!("expected to be the first instance"),
        }
    }

    fn next(primary: &Primary) -> Forwarded {
        primary.incoming.recv_timeout(Duration::from_secs(5)).expect("nothing was forwarded")
    }

    #[test]
    fn a_path_survives_the_wire_format() {
        for path in [r"D:\Téléchargements\mes rendus\é.png", r"C:\a", "relative dir/x"] {
            let bytes = encode(Some(Path::new(path)));
            assert_eq!(decode(&bytes), Ok(Some(PathBuf::from(path))));
        }
    }

    #[test]
    fn no_argument_is_an_empty_payload() {
        assert_eq!(encode(None), b"IMZ1");
        assert_eq!(decode(&encode(None)), Ok(None));
    }

    #[test]
    fn foreign_or_torn_messages_are_rejected() {
        assert_eq!(decode(b"hello"), Err(ProtocolError::Magic));
        assert_eq!(decode(b""), Err(ProtocolError::Magic));
        assert_eq!(decode(b"IMZ1x"), Err(ProtocolError::Length));
    }

    #[test]
    fn names_never_carry_path_separators() {
        let names = Names::new(r"jo\hn smith");
        assert_eq!(names.mutex, r"Local\ImageMoz-johnsmith");
        assert_eq!(names.pipe, r"\\.\pipe\ImageMoz-johnsmith");
    }

    #[test]
    fn the_second_launch_finds_the_first_and_reaches_it() {
        let names = unique_names();
        let primary = first(&names);
        assert!(matches!(acquire(&names), Ok(Acquired::Existing)));

        forward(&names, Some(Path::new(r"D:\Rendus é\a.png"))).unwrap();
        assert_eq!(next(&primary), Some(PathBuf::from(r"D:\Rendus é\a.png")));
        forward(&names, None).unwrap();
        assert_eq!(next(&primary), None);
    }

    #[test]
    fn launches_in_a_burst_are_all_delivered() {
        let names = unique_names();
        let primary = first(&names);
        let senders: Vec<_> = (0..8)
            .map(|i| {
                let names = names.clone();
                std::thread::spawn(move || forward(&names, Some(Path::new(&format!(r"C:\burst\{i}")))).unwrap())
            })
            .collect();
        for sender in senders {
            sender.join().unwrap();
        }
        let mut got: Vec<_> = (0..8).map(|_| next(&primary).unwrap()).collect();
        got.sort();
        let expected: Vec<_> = (0..8).map(|i| PathBuf::from(format!(r"C:\burst\{i}"))).collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn forwarding_wakes_the_ui() {
        let names = unique_names();
        let primary = first(&names);
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();
        primary.set_wake(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        forward(&names, None).unwrap();
        next(&primary);
        let start = Instant::now();
        while woken.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn forwarding_without_a_primary_fails_instead_of_hanging() {
        let started = Instant::now();
        let result = forward_within(&unique_names(), None, Duration::from_millis(200));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
