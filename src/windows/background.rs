use super::{
    clipboard,
    ipc::{self, Action, Request, Response},
};
use anyhow::{Context, Result, bail, ensure};
use std::os::windows::io::AsRawHandle;
use std::{
    collections::HashMap,
    fs::File,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{
                CREATE_NO_WINDOW, CreateProcessW, GetCurrentThreadId, PROCESS_INFORMATION,
                STARTF_USESHOWWINDOW, STARTUPINFOW,
            },
        },
        UI::{Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, PWSTR, w},
};

struct Session {
    active: bool,
    window: isize,
    updated: Instant,
    busy: bool,
    events: SyncSender<Response>,
}
struct Daemon {
    sessions: Mutex<HashMap<u32, Session>>,
    paused: AtomicBool,
    swallow_v: AtomicBool,
    thread: u32,
}
static DAEMON: OnceLock<Arc<Daemon>> = OnceLock::new();
const TRAY_MESSAGE: u32 = WM_APP + 7;

pub fn ensure_running() -> Result<()> {
    if ipc::connect().is_ok() {
        return Ok(());
    }
    let executable = ipc::sibling("clippy-background.exe")?;
    ensure!(
        executable.is_file(),
        "clippy-background.exe is missing; use the full Windows package"
    );
    // No handles are inherited, including a launcher's captured stdout/stderr.
    // A long-lived GUI child must never keep its caller's output pipes open.
    let application = ipc::wide(&executable.to_string_lossy());
    let mut command = ipc::wide(&format!("\"{}\"", executable.display()));
    unsafe {
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: SW_HIDE.0 as u16,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &startup,
            &mut process,
        )
        .context("start background app")?;
        let _ = CloseHandle(process.hThread);
        let _ = CloseHandle(process.hProcess);
    }
    for _ in 0..80 {
        if ipc::connect().is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!("background app did not become ready; run clippy doctor")
}
pub fn control(action: Action) -> Result<Response> {
    let mut pipe = ipc::connect()?;
    ipc::send(&mut pipe, &Request::Control(action))?;
    ipc::receive(&mut pipe)
}
pub fn command(args: &[String]) -> Result<()> {
    if args.len() == 1 && args[0] == "check" {
        ensure_running()?;
        let mut pipe = ipc::connect()?;
        ipc::send(&mut pipe, &Request::Register { busy: false })?;
        ensure!(
            matches!(ipc::receive::<Response>(&mut pipe)?, Response::Registered),
            "registration failed"
        );
        // Exercise repeated bidirectional traffic, not just registration. A
        // cloned synchronous pipe cannot read and write concurrently on Windows.
        for _ in 0..100 {
            ipc::send(
                &mut pipe,
                &Request::Poll {
                    active: false,
                    window: 0,
                    finished: true,
                },
            )?;
            ensure!(
                matches!(ipc::receive::<Response>(&mut pipe)?, Response::Idle),
                "unexpected background poll response"
            );
        }
        ipc::send(&mut pipe, &Request::Unregister)?;
        ensure!(
            ipc::receive::<Response>(&mut pipe).is_err(),
            "unregister did not close the session"
        );
        println!("Background registration, 100 focus exchanges, and cleanup verified.");
        return Ok(());
    }
    ensure!(
        args.len() <= 1,
        "usage: clippy background start|status|pause|resume|stop"
    );
    let action = match args.first().map(String::as_str).unwrap_or("status") {
        "start" => {
            ensure_running()?;
            Action::Status
        }
        "status" => Action::Status,
        "pause" => Action::Pause,
        "resume" => Action::Resume,
        "stop" => Action::Stop,
        _ => bail!("usage: clippy background start|status|pause|resume|stop"),
    };
    let response = control(action)?;
    if let Response::Status {
        pid,
        paused,
        sessions,
    } = response
    {
        println!(
            "Clippy background: {} (PID {pid}, {sessions} session(s))",
            if matches!(action, Action::Stop) {
                "stopping"
            } else if paused {
                "paused"
            } else {
                "running"
            }
        );
    }
    Ok(())
}
fn status(state: &Daemon) -> Response {
    Response::Status {
        pid: std::process::id(),
        paused: state.paused.load(Ordering::SeqCst),
        sessions: state.sessions.lock().unwrap().len(),
    }
}
struct Registration {
    pid: u32,
    state: Arc<Daemon>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.state.sessions.lock().unwrap().remove(&self.pid);
    }
}
fn connection(mut file: File, state: Arc<Daemon>) -> Result<()> {
    let pid = ipc::validate_client(&file)?;
    ipc::send(
        &mut file,
        &Response::Hello {
            version: env!("CARGO_PKG_VERSION").into(),
        },
    )?;
    match ipc::receive::<Request>(&mut file)? {
        Request::Control(action) => {
            match action {
                Action::Pause => state.paused.store(true, Ordering::SeqCst),
                Action::Resume => state.paused.store(false, Ordering::SeqCst),
                _ => {}
            }
            ipc::send(&mut file, &status(&state))?;
            if matches!(action, Action::Stop) {
                unsafe {
                    let _ = PostThreadMessageW(state.thread, WM_QUIT, WPARAM(0), LPARAM(0));
                }
            }
        }
        Request::Register { busy } => {
            let (events, receive) = mpsc::sync_channel(8);
            {
                let mut sessions = state.sessions.lock().unwrap();
                ensure!(
                    sessions.len() < 32 && !sessions.contains_key(&pid),
                    "session registration refused"
                );
                sessions.insert(
                    pid,
                    Session {
                        active: false,
                        window: 0,
                        updated: Instant::now(),
                        busy,
                        events,
                    },
                );
            }
            let _registration = Registration {
                pid,
                state: state.clone(),
            };
            ipc::send(&mut file, &Response::Registered)?;
            loop {
                let message = ipc::receive::<Request>(&mut file)?;
                let response = {
                    let mut sessions = state.sessions.lock().unwrap();
                    let session = sessions.get_mut(&pid).context("session closed")?;
                    match message {
                        Request::Poll {
                            active,
                            window,
                            finished,
                        } => poll_session(session, &receive, active, window, finished),
                        Request::Unregister => break,
                        _ => bail!("unexpected session message"),
                    }
                };
                // Never hold the registry mutex during pipe I/O.
                ipc::send(&mut file, &response)?;
            }
        }
        _ => bail!("expected session registration or control request"),
    }
    Ok(())
}

fn poll_session(
    session: &mut Session,
    receive: &mpsc::Receiver<Response>,
    active: bool,
    window: isize,
    finished: bool,
) -> Response {
    session.active = active;
    session.window = window;
    session.updated = Instant::now();
    if finished {
        session.busy = false;
    }
    receive.try_recv().unwrap_or(Response::Idle)
}
pub fn run() -> Result<()> {
    let first = ipc::create_listener(true)?;
    let state = Arc::new(Daemon {
        sessions: Mutex::new(HashMap::new()),
        paused: AtomicBool::new(false),
        swallow_v: AtomicBool::new(false),
        thread: unsafe { GetCurrentThreadId() },
    });
    DAEMON
        .set(state.clone())
        .map_err(|_| anyhow::anyhow!("background app already initialized"))?;
    unsafe {
        // Create the message queue before a control request can post to it.
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    let server = state.clone();
    thread::spawn(move || {
        let mut listener = first;
        loop {
            if ipc::accept(&listener).is_err() {
                break;
            }
            let connection_state = server.clone();
            thread::spawn(move || {
                let Ok(disconnect) = listener.try_clone() else {
                    return;
                };
                let completed = connection(listener, connection_state).is_ok();
                unsafe {
                    let handle = HANDLE(disconnect.as_raw_handle());
                    if completed {
                        let _ = windows::Win32::Storage::FileSystem::FlushFileBuffers(handle);
                    }
                    let _ = windows::Win32::System::Pipes::DisconnectNamedPipe(handle);
                }
            });
            match ipc::create_listener(false) {
                Ok(next) => listener = next,
                Err(_) => break,
            }
        }
    });
    unsafe {
        let module = GetModuleHandleW(None)?;
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook),
            Some(HINSTANCE(module.0)),
            0,
        )?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(tray_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("ClippyBackground"),
            ..Default::default()
        };
        ensure!(RegisterClassW(&class) != 0, "cannot register tray window");
        let window = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("ClippyBackground"),
            w!("Clippy"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?;
        notify_icon(window, NIM_ADD);
        SetTimer(Some(window), 1, 1000, None);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        notify_icon(window, NIM_DELETE);
        let _ = KillTimer(Some(window), 1);
        let _ = DestroyWindow(window);
        let _ = UnhookWindowsHookEx(hook);
        let _ = UnregisterClassW(w!("ClippyBackground"), Some(HINSTANCE(module.0)));
    }
    Ok(())
}
pub fn check_hook_support() -> Result<()> {
    unsafe {
        let module = GetModuleHandleW(None)?;
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook),
            Some(HINSTANCE(module.0)),
            0,
        )?;
        UnhookWindowsHookEx(hook)?;
    }
    Ok(())
}
unsafe fn notify_icon(window: HWND, action: NOTIFY_ICON_MESSAGE) {
    unsafe {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: TRAY_MESSAGE,
            hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
            ..Default::default()
        };
        let state = DAEMON.get().unwrap();
        let tip = format!(
            "Clippy — {} — {} session(s)",
            if state.paused.load(Ordering::SeqCst) {
                "paused"
            } else {
                "ready"
            },
            state.sessions.lock().unwrap().len()
        );
        for (to, from) in data.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
            *to = from;
        }
        let _ = Shell_NotifyIconW(action, &data);
    }
}
unsafe extern "system" fn tray_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == RegisterWindowMessageW(w!("TaskbarCreated")) {
            notify_icon(window, NIM_ADD);
            return LRESULT(0);
        }
        if message == WM_TIMER {
            notify_icon(window, NIM_MODIFY);
            return LRESULT(0);
        }
        if message == TRAY_MESSAGE && matches!(lparam.0 as u32, WM_RBUTTONUP | WM_CONTEXTMENU) {
            if let Ok(menu) = CreatePopupMenu() {
                let state = DAEMON.get().unwrap();
                let label = if state.paused.load(Ordering::SeqCst) {
                    w!("Resume clipboard paste")
                } else {
                    w!("Pause clipboard paste")
                };
                let _ = AppendMenuW(menu, MF_STRING, 1, label);
                let _ = AppendMenuW(menu, MF_STRING, 2, w!("Exit Clippy background"));
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                let _ = SetForegroundWindow(window);
                let choice = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY,
                    point.x,
                    point.y,
                    None,
                    window,
                    None,
                )
                .0;
                if choice == 1 {
                    state.paused.fetch_xor(true, Ordering::SeqCst);
                    notify_icon(window, NIM_MODIFY);
                }
                if choice == 2 {
                    PostQuitMessage(0);
                }
                let _ = DestroyMenu(menu);
            }
            return LRESULT(0);
        }
        DefWindowProcW(window, message, wparam, lparam)
    }
}
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(state) = DAEMON.get() {
            let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            if key.vkCode == VK_V.0 as u32 && up && state.swallow_v.swap(false, Ordering::SeqCst) {
                return LRESULT(1);
            }
            if down && !state.paused.load(Ordering::SeqCst) {
                let ctrl = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
                let alt = unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } < 0;
                if ctrl && !alt {
                    let foreground = unsafe { GetForegroundWindow().0 as isize };
                    // Never block the hook on IPC, clipboard payload reads, COM, or a mutex.
                    if let Ok(mut sessions) = state.sessions.try_lock() {
                        if let Some(session) = focused_session(&mut sessions, foreground) {
                            if key.vkCode == VK_C.0 as u32
                                && session.busy
                                && session.events.try_send(Response::Cancel).is_ok()
                            {
                                return LRESULT(1);
                            }
                            if key.vkCode == VK_V.0 as u32 && clipboard::has_rich_content() {
                                if session.busy {
                                    state.swallow_v.store(true, Ordering::SeqCst);
                                    return LRESULT(1);
                                }
                                let sequence = unsafe {
                                    windows::Win32::System::DataExchange::GetClipboardSequenceNumber(
                                    )
                                };
                                if session
                                    .events
                                    .try_send(Response::Paste {
                                        sequence,
                                        window: foreground,
                                    })
                                    .is_ok()
                                {
                                    session.busy = true;
                                    state.swallow_v.store(true, Ordering::SeqCst);
                                    return LRESULT(1);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn focused_session(sessions: &mut HashMap<u32, Session>, window: isize) -> Option<&mut Session> {
    let mut candidates = sessions.values_mut().filter(|s| {
        s.active
            && window != 0
            && s.window == window
            && s.updated.elapsed() < Duration::from_secs(2)
    });
    let candidate = candidates.next()?;
    if candidates.next().is_some() {
        None
    } else {
        Some(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(window: isize) -> Session {
        Session {
            active: true,
            window,
            updated: Instant::now(),
            busy: false,
            events: mpsc::sync_channel(8).0,
        }
    }
    #[test]
    fn routing_requires_one_fresh_focused_session() {
        let mut sessions = HashMap::from([(1, session(100)), (2, session(200))]);
        assert!(focused_session(&mut sessions, 100).is_some());
        assert!(focused_session(&mut sessions, 300).is_none());
        sessions.insert(3, session(100));
        assert!(focused_session(&mut sessions, 100).is_none());
        sessions.get_mut(&3).unwrap().active = false;
        assert!(focused_session(&mut sessions, 100).is_some());
        sessions.get_mut(&1).unwrap().updated = Instant::now() - Duration::from_secs(3);
        assert!(focused_session(&mut sessions, 100).is_none());
    }
    #[test]
    fn polling_delivers_events_and_releases_busy_after_completion() {
        let (events, receive) = mpsc::sync_channel(8);
        let mut s = Session {
            events,
            ..session(100)
        };
        s.busy = true;
        s.events
            .try_send(Response::Paste {
                sequence: 42,
                window: 100,
            })
            .unwrap();
        assert!(matches!(
            poll_session(&mut s, &receive, true, 100, false),
            Response::Paste {
                sequence: 42,
                window: 100
            }
        ));
        assert!(s.busy);
        s.events.try_send(Response::Cancel).unwrap();
        assert!(matches!(
            poll_session(&mut s, &receive, true, 100, false),
            Response::Cancel
        ));
        assert!(matches!(
            poll_session(&mut s, &receive, false, 0, true),
            Response::Idle
        ));
        assert!(!s.busy && !s.active);
        for _ in 0..100 {
            assert!(matches!(
                poll_session(&mut s, &receive, true, 100, false),
                Response::Idle
            ));
        }
    }
    #[test]
    fn disconnect_removes_session_registration() {
        let state = Arc::new(Daemon {
            sessions: Mutex::new(HashMap::from([(42, session(100))])),
            paused: AtomicBool::new(false),
            swallow_v: AtomicBool::new(false),
            thread: 0,
        });
        drop(Registration {
            pid: 42,
            state: state.clone(),
        });
        assert!(state.sessions.lock().unwrap().is_empty());
    }
}
