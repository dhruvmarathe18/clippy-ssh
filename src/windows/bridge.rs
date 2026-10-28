use super::{
    clipboard::{self, Item},
    console,
};
use anyhow::{Context, Result, bail};
use std::{
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, Threading::GetCurrentThreadId},
        UI::{Accessibility::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

pub enum Event {
    Paste { items: Vec<Item> },
    Cancel,
    Error(String),
}
enum Trigger {
    Paste { sequence: u32, window: isize },
    Cancel,
}
pub struct State {
    pub busy: AtomicBool,
    pub cancel: AtomicBool,
    active: AtomicBool,
    window: AtomicIsize,
    swallow_v: AtomicBool,
    stop: AtomicBool,
    trigger: SyncSender<Trigger>,
    pub status: Mutex<(String, Instant)>,
}
impl State {
    pub fn show(&self, text: impl Into<String>, seconds: u64) {
        *self.status.lock().unwrap() = (text.into(), Instant::now() + Duration::from_secs(seconds));
    }
    pub fn finish(&self) {
        self.busy.store(false, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
    }
}
pub struct Bridge {
    pub state: Arc<State>,
    pub events: Receiver<Event>,
    hook_thread: u32,
}
static STATE: OnceLock<Arc<State>> = OnceLock::new();
impl Bridge {
    pub fn start() -> Result<Self> {
        let (trigger_tx, trigger_rx) = mpsc::sync_channel(8);
        let (event_tx, events) = mpsc::channel();
        let state = Arc::new(State {
            busy: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            active: AtomicBool::new(false),
            window: AtomicIsize::new(0),
            swallow_v: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            trigger: trigger_tx,
            status: Mutex::new((String::new(), Instant::now())),
        });
        STATE
            .set(state.clone())
            .map_err(|_| anyhow::anyhow!("paste bridge already running"))?;
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
                Ok(v) => {
                    let _ = ready_tx.send(Ok(()));
                    v
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("{e:#}")));
                    return;
                }
            };
            while !worker_state.stop.load(Ordering::Relaxed) {
                let is_current = || unsafe {
                    automation
                        .GetFocusedElement()
                        .and_then(|focus| automation.CompareElements(&registered, &focus))
                        .map(|v| v.as_bool())
                        .unwrap_or(false)
                };
                let active = is_current();
                worker_state.window.store(
                    if active {
                        unsafe { GetForegroundWindow().0 as isize }
                    } else {
                        0
                    },
                    Ordering::SeqCst,
                );
                worker_state.active.store(active, Ordering::SeqCst);
                match trigger_rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(Trigger::Cancel) => {
                        let _ = event_tx.send(Event::Cancel);
                    }
                    Ok(Trigger::Paste { sequence, window }) => {
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
                            Err(e) => {
                                worker_state.finish();
                                let _ = event_tx.send(Event::Error(format!("{e:#}")));
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
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
            Ok(Err(e)) => {
                state.stop.store(true, Ordering::Relaxed);
                bail!("{e}");
            }
            Err(e) => {
                state.stop.store(true, Ordering::Relaxed);
                return Err(e);
            }
        }
        let (hook_tx, hook_rx) = mpsc::channel();
        thread::spawn(move || unsafe {
            match install_hook() {
                Ok(hook) => {
                    let _ = hook_tx.send(Ok(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    let _ = UnhookWindowsHookEx(hook);
                }
                Err(e) => {
                    let _ = hook_tx.send(Err(e.to_string()));
                }
            }
        });
        let hook_thread = hook_rx.recv()?.map_err(|e| anyhow::anyhow!(e))?;
        let popup_state = state.clone();
        thread::spawn(move || popup(popup_state));
        Ok(Self {
            state,
            events,
            hook_thread,
        })
    }
}
unsafe fn install_hook() -> windows::core::Result<HHOOK> {
    let module = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None)? };
    unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook),
            Some(HINSTANCE(module.0)),
            0,
        )
    }
}
pub fn check_hook_support() -> Result<()> {
    // Installation/removal only: this diagnostic never reads or generates keyboard input.
    unsafe {
        let hook = install_hook()?;
        UnhookWindowsHookEx(hook)?;
    }
    Ok(())
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        unsafe {
            let _ = PostThreadMessageW(self.hook_thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(state) = STATE.get() {
            // Never perform clipboard reads, COM calls, or network operations inside this callback.
            let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            // Accessibility/remapping software can synthesize legitimate paste keys.
            // Clippy never injects keystrokes itself, so there is no recursion to filter.
            {
                let down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
                let up = wparam.0 as u32 == WM_KEYUP || wparam.0 as u32 == WM_SYSKEYUP;
                if key.vkCode == VK_V.0 as u32
                    && up
                    && state.swallow_v.swap(false, Ordering::SeqCst)
                {
                    return LRESULT(1);
                }
                let foreground = unsafe { GetForegroundWindow().0 as isize };
                if down
                    && state.active.load(Ordering::SeqCst)
                    && foreground == state.window.load(Ordering::SeqCst)
                {
                    let ctrl = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
                    let alt = unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } < 0;
                    if ctrl
                        && !alt
                        && key.vkCode == VK_C.0 as u32
                        && state.busy.load(Ordering::SeqCst)
                    {
                        state.cancel.store(true, Ordering::SeqCst);
                        let _ = state.trigger.try_send(Trigger::Cancel);
                        return LRESULT(1);
                    }
                    if ctrl && !alt && key.vkCode == VK_V.0 as u32 && clipboard::has_rich_content()
                    {
                        state.swallow_v.store(true, Ordering::SeqCst);
                        if !state.busy.swap(true, Ordering::SeqCst) {
                            let sequence = unsafe {
                                windows::Win32::System::DataExchange::GetClipboardSequenceNumber()
                            };
                            if state
                                .trigger
                                .try_send(Trigger::Paste {
                                    sequence,
                                    window: foreground,
                                })
                                .is_err()
                            {
                                state.finish();
                            }
                        }
                        return LRESULT(1);
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
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
