use super::{
    background,
    clipboard::{self, Item},
    console,
    ipc::{self, Request, Response},
};
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::Com::*,
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

pub enum Event {
    Paste { items: Vec<Item> },
    Cancel,
    Error(String),
}
pub struct State {
    pub busy: AtomicBool,
    pub cancel: AtomicBool,
    active: AtomicBool,
    stop: AtomicBool,
    finished: AtomicBool,
    pub status: Mutex<(String, Instant)>,
}
impl State {
    pub fn show(&self, text: impl Into<String>, seconds: u64) {
        *self.status.lock().unwrap() = (text.into(), Instant::now() + Duration::from_secs(seconds));
    }
    pub fn finish(&self) {
        self.busy.store(false, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
        self.finished.store(true, Ordering::SeqCst);
    }
}
pub struct Bridge {
    pub state: Arc<State>,
    pub events: Receiver<Event>,
}
fn register(state: &State) -> Result<File> {
    let mut pipe = ipc::connect()?;
    ipc::send(
        &mut pipe,
        &Request::Register {
            busy: state.busy.load(Ordering::SeqCst),
        },
    )?;
    anyhow::ensure!(
        matches!(ipc::receive::<Response>(&mut pipe)?, Response::Registered),
        "session registration failed"
    );
    Ok(pipe)
}
impl Bridge {
    pub fn start() -> Result<Self> {
        background::ensure_running()?;
        let (event_tx, events) = mpsc::channel();
        let state = Arc::new(State {
            busy: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            active: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            status: Mutex::new((String::new(), Instant::now())),
        });
        let mut pipe = register(&state)?;
        let marker = format!("Clippy-register-{}", uuid::Uuid::new_v4());
        let title = console::title();
        console::set_title(&marker)?;
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker_state = state.clone();
        thread::spawn(move || {
            let init = (|| -> Result<(IUIAutomation, IUIAutomationElement)> {
                unsafe {
                    CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
                    let automation: IUIAutomation =
                        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
                    for _ in 0..160 {
                        if let Ok(element) = automation.GetFocusedElement() {
                            if element.CurrentClassName()? == "TermControl"
                                && element.CurrentHelpText()? == marker
                            {
                                return Ok((automation, element));
                            }
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                    bail!(
                        "cannot identify this Windows Terminal pane; focus it and allow application titles, then reconnect"
                    )
                }
            })();
            let (automation, registered) = match init {
                Ok(value) => {
                    let _ = ready_tx.send(Ok(()));
                    value
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    let _ = ipc::send(&mut pipe, &Request::Unregister);
                    return;
                }
            };
            let mut pipe = Some(pipe);
            let mut retry = Instant::now();
            while !worker_state.stop.load(Ordering::Relaxed) {
                let is_current = || unsafe {
                    automation
                        .GetFocusedElement()
                        .and_then(|focus| automation.CompareElements(&registered, &focus))
                        .map(|v| v.as_bool())
                        .unwrap_or(false)
                };
                let active = is_current();
                worker_state.active.store(active, Ordering::SeqCst);
                let window = if active {
                    unsafe { GetForegroundWindow().0 as isize }
                } else {
                    0
                };
                if pipe.is_none() && retry.elapsed() > Duration::from_secs(1) {
                    retry = Instant::now();
                    if let Ok(connection) = register(&worker_state) {
                        pipe = Some(connection);
                    }
                }
                // One owner performs alternating writes/reads. Concurrent I/O on
                // cloned synchronous pipe handles deadlocks the focus heartbeat.
                let next = pipe.as_mut().map(|connection| -> Result<Response> {
                    ipc::send(
                        connection,
                        &Request::Poll {
                            active,
                            window,
                            finished: worker_state.finished.swap(false, Ordering::SeqCst),
                        },
                    )?;
                    ipc::receive(connection)
                });
                match next {
                    Some(Ok(Response::Paste { sequence, window })) => {
                        if worker_state.busy.swap(true, Ordering::SeqCst) {
                            continue;
                        }
                        worker_state.cancel.store(false, Ordering::SeqCst);
                        let snapshot = (|| -> Result<Vec<Item>> {
                            if !is_current()
                                || unsafe { GetForegroundWindow().0 as isize } != window
                            {
                                bail!("Focus changed; paste again in the intended pane.");
                            }
                            if unsafe {
                                windows::Win32::System::DataExchange::GetClipboardSequenceNumber()
                            } != sequence
                            {
                                bail!("Clipboard changed; paste again.");
                            }
                            clipboard::snapshot()
                        })();
                        match snapshot {
                            Ok(items) => {
                                let _ = event_tx.send(Event::Paste { items });
                            }
                            Err(error) => {
                                worker_state.finish();
                                let _ = event_tx.send(Event::Error(error.to_string()));
                            }
                        }
                    }
                    Some(Ok(Response::Cancel)) => {
                        worker_state.cancel.store(true, Ordering::SeqCst);
                        let _ = event_tx.send(Event::Cancel);
                    }
                    Some(Err(_)) => {
                        pipe = None;
                        let _ = event_tx.send(Event::Error("Background app stopped. SSH stays connected; run clippy background start to restore file paste.".into()));
                    }
                    _ => {}
                }
                thread::sleep(Duration::from_millis(25));
            }
            if let Some(mut connection) = pipe {
                let _ = ipc::send(&mut connection, &Request::Unregister);
            }
            unsafe {
                CoUninitialize();
            }
        });
        let ready = ready_rx
            .recv_timeout(Duration::from_secs(10))
            .context("Windows Terminal registration timed out");
        let _ = console::set_title(&title);
        match ready {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                state.stop.store(true, Ordering::SeqCst);
                bail!("{error}");
            }
            Err(error) => {
                state.stop.store(true, Ordering::SeqCst);
                return Err(error);
            }
        }
        let popup_state = state.clone();
        thread::spawn(move || popup(popup_state));
        Ok(Self { state, events })
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
    }
}

unsafe extern "system" fn popup_proc(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        if message == WM_NCHITTEST {
            return LRESULT(HTTRANSPARENT as isize);
        }
        if message == WM_PAINT {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(window, &mut paint);
            let font = GetStockObject(DEFAULT_GUI_FONT);
            let previous = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00eeeeee));
            let mut text = vec![0; GetWindowTextLengthW(window).max(0) as usize + 1];
            let n = GetWindowTextW(window, &mut text);
            let mut rect = RECT::default();
            let _ = GetClientRect(window, &mut rect);
            rect.left += 14;
            rect.top += 10;
            rect.right -= 14;
            rect.bottom -= 8;
            DrawTextW(
                dc,
                &mut text[..n as usize],
                &mut rect,
                DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
            );
            SelectObject(dc, previous);
            let _ = EndPaint(window, &paint);
            return LRESULT(0);
        }
        DefWindowProcW(window, message, w, l)
    }
}
fn popup(state: Arc<State>) {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let Ok(module) = windows::Win32::System::LibraryLoader::GetModuleHandleW(None) else {
            return;
        };
        let brush = CreateSolidBrush(COLORREF(0x002b2927));
        let class = WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("ClippyStatus"),
            hbrBackground: brush,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            let _ = DeleteObject(brush.into());
            return;
        }
        let Ok(window) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("ClippyStatus"),
            w!(""),
            WS_POPUP,
            0,
            0,
            420,
            82,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        ) else {
            let _ = UnregisterClassW(w!("ClippyStatus"), Some(HINSTANCE(module.0)));
            let _ = DeleteObject(brush.into());
            return;
        };
        while !state.stop.load(Ordering::Relaxed) {
            let (text, until) = state.status.lock().unwrap().clone();
            if !text.is_empty() && Instant::now() < until && state.active.load(Ordering::SeqCst) {
                let mut rect = RECT::default();
                let foreground = GetForegroundWindow();
                if GetWindowRect(foreground, &mut rect).is_ok() {
                    let _ = SetWindowPos(
                        window,
                        Some(HWND_TOPMOST),
                        (rect.right - 440).max(0),
                        (rect.bottom - 120).max(0),
                        420,
                        82,
                        SWP_NOACTIVATE,
                    );
                }
                let text = format!("Clippy\r\n{text}");
                let mut wide = text.encode_utf16().collect::<Vec<_>>();
                wide.push(0);
                let _ = SetWindowTextW(window, PCWSTR(wide.as_ptr()));
                let _ = InvalidateRect(Some(window), None, true);
                let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            } else {
                let _ = ShowWindow(window, SW_HIDE);
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = DestroyWindow(window);
        let _ = UnregisterClassW(w!("ClippyStatus"), Some(HINSTANCE(module.0)));
        let _ = DeleteObject(brush.into());
    }
}
