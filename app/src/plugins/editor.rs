//! Native windows that hold plugin editors. They are owned by the main window and created on
//! the UI thread, so the main message loop drives them.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering::{Acquire, Release}};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::COLOR_BTNFACE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct WinState {
    closed: bool,
    resizable: bool,
    on_timer: Option<Box<dyn FnMut()>>,
    on_size: Option<Box<dyn FnMut(i32, i32)>>,
    /// Set when a tick is posted to the window, cleared once the window has handled it.
    tick_pending: Arc<AtomicBool>,
}

/// The message a window's ticker posts for each tick of its timer.
const WM_TICK: u32 = WM_APP + 1;
/// A tick the window hasn't handled after this long is posted again in case a modal loop
/// dropped it, as in the original's timer thread (0x1401414f0).
const TICK_REPOST: Duration = Duration::from_millis(300);
/// Longest single wait of the ticker, as in the original's timer thread.
const TICK_MAX_WAIT: Duration = Duration::from_millis(50);
/// How often the ticker looks again while the window hasn't handled its last tick yet (the
/// original's timer thread waits 4 ms between looks).
const TICK_POLL: Duration = Duration::from_millis(4);

/// A window's timer, kept the way the original keeps all its timers (its "Juce Timer" thread,
/// 0x1401414f0): a thread of its own counts the time to the millisecond and posts one tick at a
/// time to the window, the next only once the window has handled the last. Posted messages
/// reach a window ahead of Windows' own timer messages, which come at best every 15.6 ms.
struct Ticker {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Ticker {
    fn start(hwnd: HWND, ms: u32, pending: Arc<AtomicBool>) -> Option<Ticker> {
        let stop = Arc::new(AtomicBool::new(false));
        let halt = stop.clone();
        let hwnd = hwnd as isize;
        let period = Duration::from_millis(u64::from(ms.max(1)));
        let post = move || unsafe { PostMessageW(hwnd as HWND, WM_TICK, 0, 0) != 0 };
        let thread = std::thread::Builder::new()
            .name("plugin window timer".into())
            .spawn(move || {
                let mut next = Instant::now() + period;
                let mut posted = Instant::now();
                while !halt.load(Acquire) {
                    let now = Instant::now();
                    if now < next {
                        std::thread::sleep((next - now).min(TICK_MAX_WAIT));
                        continue;
                    }
                    if pending.load(Acquire) {
                        // The last tick is still waiting: this one goes as soon as the window
                        // has handled it.
                        if now - posted >= TICK_REPOST {
                            posted = now;
                            post();
                        }
                        std::thread::sleep(TICK_POLL);
                        continue;
                    }
                    pending.store(true, Release);
                    posted = now;
                    if !post() {
                        pending.store(false, Release);
                    }
                    // A tick a whole period late starts the count again rather than catching up.
                    next = if now - next >= period { now + period } else { next + period };
                }
            })
            .ok()?;
        Some(Ticker { stop, thread })
    }

    fn stop(self) {
        self.stop.store(true, Release);
        let _ = self.thread.join();
    }
}

/// How far the person can drag a resizable plugin window: the original's setResizeLimits
/// (396, 32, 10000, 10000) in its plugin window (0x140075950).
const DRAG_MIN: (i32, i32) = (396, 32);
const DRAG_MAX: i32 = 10000;

/// Keeps a window being dragged to a new size within the drag limits; `edge` is the WMSZ_*
/// edge or corner being dragged, which is the one that gives way.
fn limit_drag(r: &mut RECT, edge: u32) {
    let w = (r.right - r.left).clamp(DRAG_MIN.0, DRAG_MAX);
    let h = (r.bottom - r.top).clamp(DRAG_MIN.1, DRAG_MAX);
    if matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
        r.left = r.right - w;
    } else {
        r.right = r.left + w;
    }
    if matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
        r.top = r.bottom - h;
    } else {
        r.bottom = r.top + h;
    }
}

pub struct EditorWindow {
    pub hwnd: HWND,
    st: *mut WinState,
    ticker: Option<Ticker>,
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn class() -> *const u16 {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    NAME.get_or_init(|| {
        let n = wide("PedalboardHostPluginWindow");
        unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(null()),
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: (COLOR_BTNFACE + 1) as usize as *mut c_void,
                lpszMenuName: null(),
                lpszClassName: n.as_ptr(),
                hIconSm: null_mut(),
            };
            RegisterClassExW(&wc);
        }
        n
    })
    .as_ptr()
}

unsafe fn run_timer(st: *mut WinState) {
    if let Some(mut f) = (*st).on_timer.take() {
        f();
        if (*st).on_timer.is_none() {
            (*st).on_timer = Some(f);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WinState;
    if !st.is_null() {
        match msg {
            WM_CLOSE => {
                // The app notices and closes the editor properly on its next frame.
                (*st).closed = true;
                ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            WM_TICK => {
                run_timer(st);
                // Cleared after the call, as the original does, so no tick arrives inside it.
                (*st).tick_pending.store(false, Release);
                return 0;
            }
            WM_TIMER if wp == 1 => {
                run_timer(st);
                return 0;
            }
            WM_SIZING if (*st).resizable => {
                limit_drag(&mut *(lp as *mut RECT), wp as u32);
                return 1;
            }
            WM_SIZE if wp != SIZE_MINIMIZED as usize => {
                if let Some(mut f) = (*st).on_size.take() {
                    f((lp & 0xffff) as i32, ((lp >> 16) & 0xffff) as i32);
                    if (*st).on_size.is_none() {
                        (*st).on_size = Some(f);
                    }
                }
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

impl EditorWindow {
    pub fn new(title: &str, w: i32, h: i32, owner: isize, on_top: bool, resizable: bool) -> Result<EditorWindow, String> {
        unsafe {
            let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN | if resizable { WS_THICKFRAME | WS_MAXIMIZEBOX } else { 0 };
            let ex = if on_top { WS_EX_TOPMOST } else { 0 };
            let mut rc = RECT { left: 0, top: 0, right: w.max(32), bottom: h.max(32) };
            AdjustWindowRectEx(&mut rc, style, 0, ex);
            let t = wide(title);
            let hwnd = CreateWindowExW(
                ex,
                class(),
                t.as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rc.right - rc.left,
                rc.bottom - rc.top,
                owner as HWND,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            if hwnd.is_null() {
                return Err("Could not create the plugin window.".into());
            }
            let st = Box::into_raw(Box::new(WinState {
                closed: false,
                resizable,
                on_timer: None,
                on_size: None,
                tick_pending: Arc::new(AtomicBool::new(false)),
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, st as isize);
            Ok(EditorWindow { hwnd, st, ticker: None })
        }
    }

    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(self.hwnd);
        }
    }

    pub fn closed(&self) -> bool {
        unsafe { (*self.st).closed }
    }

    /// Calls `f` on the window's thread every `ms` milliseconds.
    pub fn set_timer(&mut self, ms: u32, f: Box<dyn FnMut()>) {
        if let Some(t) = self.ticker.take() {
            t.stop();
        }
        unsafe {
            (*self.st).on_timer = Some(f);
            (*self.st).tick_pending.store(false, Release);
            self.ticker = Ticker::start(self.hwnd, ms, (*self.st).tick_pending.clone());
            if self.ticker.is_none() {
                // Without a thread of its own, Windows' timer stands in, in 15.6 ms steps.
                SetTimer(self.hwnd, 1, ms, None);
            }
        }
    }

    pub fn set_on_size(&mut self, f: Box<dyn FnMut(i32, i32)>) {
        unsafe { (*self.st).on_size = Some(f) };
    }

    pub fn set_topmost(&self, on: bool) {
        unsafe {
            SetWindowPos(self.hwnd, if on { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// Resizes a window so its client area is `w`×`h`.
pub fn resize_client(hwnd: isize, w: i32, h: i32) {
    unsafe {
        let hwnd = hwnd as HWND;
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut cr = wr;
        // The frame is measured on the window itself so it is right at any display scaling;
        // a minimised window has no client area to measure.
        let (fw, fh) = if IsIconic(hwnd) == 0 && GetWindowRect(hwnd, &mut wr) != 0 && GetClientRect(hwnd, &mut cr) != 0 {
            (wr.right - wr.left - cr.right, wr.bottom - wr.top - cr.bottom)
        } else {
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let mut rc = RECT { left: 0, top: 0, right: w, bottom: h };
            AdjustWindowRectEx(&mut rc, style, 0, ex);
            (rc.right - rc.left - w, rc.bottom - rc.top - h)
        };
        SetWindowPos(hwnd, null_mut(), 0, 0, w + fw, h + fh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// The plugin's own window inside an editor window (0 when it has none): the first child,
/// as the original finds it (GetWindow GW_CHILD in 0x1400fdf40).
pub fn plugin_view(hwnd: isize) -> isize {
    unsafe { GetWindow(hwnd as HWND, GW_CHILD) as isize }
}

/// Outer size of a window.
pub fn window_size(hwnd: isize) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetWindowRect(hwnd as HWND, &mut r) };
    (r.right - r.left, r.bottom - r.top)
}

/// Size of a window's client area.
pub fn client_size(hwnd: isize) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd as HWND, &mut r) };
    (r.right, r.bottom)
}

/// Visible on screen and not minimised.
pub fn is_shown(hwnd: isize) -> bool {
    unsafe { IsWindowVisible(hwnd as HWND) != 0 && IsIconic(hwnd as HWND) == 0 }
}

/// Changes the size of a plugin's window without moving it.
pub fn set_view_size(view: isize, w: i32, h: i32) {
    unsafe { SetWindowPos(view as HWND, null_mut(), 0, 0, w, h, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOOWNERZORDER | SWP_NOZORDER) };
}

/// Lays a plugin's window over the whole client area.
pub fn fit_view(view: isize, w: i32, h: i32) {
    unsafe { MoveWindow(view as HWND, 0, 0, w, h, 1) };
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        // The ticker stops before the window goes, so it never posts to a window that's gone.
        if let Some(t) = self.ticker.take() {
            t.stop();
        }
        unsafe {
            KillTimer(self.hwnd, 1);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            DestroyWindow(self.hwnd);
            drop(Box::from_raw(self.st));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_stops_at_the_original_limits() {
        // The left edge dragged in too far stops; the right edge stays put.
        let mut r = RECT { left: 500, top: 100, right: 700, bottom: 400 };
        limit_drag(&mut r, WMSZ_LEFT);
        assert_eq!((r.left, r.top, r.right, r.bottom), (304, 100, 700, 400));
        // The bottom-right corner dragged up past the smallest height.
        let mut r = RECT { left: 0, top: 0, right: 800, bottom: 10 };
        limit_drag(&mut r, WMSZ_BOTTOMRIGHT);
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 800, 32));
        // The top-left corner dragged out past the largest size.
        let mut r = RECT { left: -20000, top: -20000, right: 100, bottom: 100 };
        limit_drag(&mut r, WMSZ_TOPLEFT);
        assert_eq!((r.left, r.top, r.right, r.bottom), (-9900, -9900, 100, 100));
    }

    /// Handles every message waiting for this thread.
    fn drain() {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    #[test]
    fn ticks_arrive_every_period_and_never_pile_up() {
        use std::cell::Cell;
        use std::rc::Rc;
        let mut win = EditorWindow::new("test", 300, 150, 0, false, false).unwrap();
        let ticks = Rc::new(Cell::new(0u32));
        let t = ticks.clone();
        win.set_timer(10, Box::new(move || t.set(t.get() + 1)));
        // A window too busy to take its ticks for 200 ms finds one waiting, not twenty.
        std::thread::sleep(Duration::from_millis(200));
        drain();
        assert_eq!(ticks.get(), 1);
        // Taken as they come, a tick arrives every 10 ms (Windows' own timer gives about 64 a second).
        ticks.set(0);
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            drain();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!((85..=101).contains(&ticks.get()), "{} ticks in a second", ticks.get());
    }
}
