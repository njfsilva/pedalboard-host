//! Native windows that hold plugin editors, with the "Presets:" bar (the original's PresetBar,
//! 0x1400865f0) along their top and the plugin's own window in the area under it. They are
//! owned by the main window and created on the UI thread, so the main message loop drives them.

use crate::presets::{
    BarEvent, IconPart, BAR_BG, BAR_H, BAR_LINE, BAR_W, COMBO, COMBO_TEXT, DISK, EDIT, FOLDER, FONT, ICON_AT, IMPORT_BUTTON, LABEL, LABEL_TEXT, LINE_Y,
    NO_CHOICES, SAVE_BUTTON, TIP_IMPORT, TIP_SAVE,
};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering::{Acquire, Release}};
use std::sync::{Arc, Once, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontIndirectW, CreatePen, CreateRoundRectRgn, DeleteDC, DeleteObject,
    DrawTextW, EndPaint, FillRect, FrameRgn, GetDC, GetStockObject, GetSysColorBrush, GetTextExtentPoint32W, GetTextMetricsW, InvalidateRect, Polygon, Polyline,
    ReleaseDC, SelectClipRgn, SelectObject, SetBkColor, SetBkMode, SetDCBrushColor, SetTextColor, CLEARTYPE_QUALITY, COLOR_BTNFACE, DC_BRUSH, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HDC, HFONT, LOGFONTW, NULL_PEN, PAINTSTRUCT, PS_SOLID, SRCCOPY, TEXTMETRICW, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{InitCommonControlsEx, ICC_BAR_CLASSES, INITCOMMONCONTROLSEX, TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_RETURN};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

// Edit-control and mouse messages and styles, defined here so their types are plain.
const EM_SETSEL: u32 = 0x00B1;
const EM_SETMARGINS: u32 = 0x00D3;
const EC_LEFTMARGIN: usize = 1;
const EC_RIGHTMARGIN: usize = 2;
const EN_KILLFOCUS: u32 = 0x0200;
const ES_AUTOHSCROLL: u32 = 0x0080;
const WM_CTLCOLOREDIT: u32 = 0x0133;
const WM_MOUSELEAVE: u32 = 0x02A3;

struct WinState {
    closed: bool,
    resizable: bool,
    on_timer: Option<Box<dyn FnMut()>>,
    on_size: Option<Box<dyn FnMut(i32, i32)>>,
    /// Set when a tick is posted to the window, cleared once the window has handled it.
    tick_pending: Arc<AtomicBool>,
    bar: BarState,
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
    /// The area under the presets bar the plugin's own window goes in.
    pub host: HWND,
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
                wake();
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
                // The bar keeps the top; the plugin gets the area under it.
                layout(hwnd);
                let (cw, ch) = ((lp & 0xffff) as i32, ((lp >> 16) & 0xffff) as i32);
                if let Some(mut f) = (*st).on_size.take() {
                    f(cw, (ch - BAR_H).max(0));
                    if (*st).on_size.is_none() {
                        (*st).on_size = Some(f);
                    }
                }
            }
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                if !dc.is_null() {
                    let (cw, ch) = client_size(hwnd as isize);
                    if ch > BAR_H {
                        let r = RECT { left: 0, top: BAR_H, right: cw, bottom: ch };
                        FillRect(dc, &r, GetSysColorBrush(COLOR_BTNFACE));
                    }
                    let b = &(*st).bar;
                    paint_bar(dc, cw, b.hover, b.pressed, b.label_font as HFONT);
                }
                EndPaint(hwnd, &ps);
                return 0;
            }
            WM_CTLCOLOREDIT if lp == (*st).bar.edit => {
                let dc = wp as HDC;
                let look = LOOK.with(Cell::get);
                let bg = over(look.editor_bg, hex(BAR_BG));
                SetTextColor(dc, colorref(over(look.text, bg)));
                SetBkColor(dc, colorref(bg));
                SetDCBrushColor(dc, colorref(bg));
                return GetStockObject(DC_BRUSH) as LRESULT;
            }
            WM_COMMAND if ((wp >> 16) & 0xffff) as u32 == EN_KILLFOCUS && lp == (*st).bar.edit => {
                commit(hwnd);
                return 0;
            }
            WM_LBUTTONDOWN => {
                let (x, y) = mouse_at(lp);
                let edit = (*st).bar.edit as HWND;
                if inside(COMBO_TEXT, x, y) && !edit.is_null() {
                    // A click on the list's text edits it, all of it selected.
                    SetFocus(edit);
                    SendMessageW(edit, EM_SETSEL, 0, -1);
                } else if let Some(k) = hit_button(x, y) {
                    SetFocus(hwnd);
                    (*st).bar.pressed = Some(k);
                    (*st).bar.hover = Some(k);
                    SetCapture(hwnd);
                    redraw_bar(hwnd);
                } else {
                    SetFocus(hwnd);
                    if inside(COMBO, x, y) {
                        popup(hwnd);
                    }
                }
                return 0;
            }
            WM_MOUSEMOVE => {
                let (x, y) = mouse_at(lp);
                if !(*st).bar.tracking {
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    (*st).bar.tracking = TrackMouseEvent(&mut t) != 0;
                }
                let h = hit_button(x, y);
                if (*st).bar.hover != h {
                    (*st).bar.hover = h;
                    redraw_bar(hwnd);
                }
                return 0;
            }
            WM_MOUSELEAVE => {
                (*st).bar.tracking = false;
                if (*st).bar.pressed.is_none() && (*st).bar.hover.is_some() {
                    (*st).bar.hover = None;
                    redraw_bar(hwnd);
                }
                return 0;
            }
            WM_LBUTTONUP => {
                let (x, y) = mouse_at(lp);
                let k = (*st).bar.pressed.take();
                if k.is_some() {
                    ReleaseCapture();
                }
                let h = hit_button(x, y);
                // A button acts when it's let go over, as JUCE's buttons do.
                if let Some(k) = k.filter(|&k| h == Some(k)) {
                    (*st).bar.events.push(if k == 0 { BarEvent::Import } else { BarEvent::Save });
                    wake();
                }
                (*st).bar.hover = h;
                redraw_bar(hwnd);
                return 0;
            }
            WM_CAPTURECHANGED => {
                if (*st).bar.pressed.take().is_some() {
                    redraw_bar(hwnd);
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
            // The bar sits above the plugin's window, and the window is never narrower than it.
            let mut rc = RECT { left: 0, top: 0, right: w.max(BAR_W), bottom: h.max(1) + BAR_H };
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
                bar: BarState::default(),
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, st as isize);
            let (cw, ch) = client_size(hwnd as isize);
            let host = CreateWindowExW(
                0,
                holder_class(),
                null(),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                BAR_H,
                cw,
                (ch - BAR_H).max(0),
                hwnd,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            if host.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DestroyWindow(hwnd);
                drop(Box::from_raw(st));
                return Err("Could not create the plugin window.".into());
            }
            let (font, label_font) = make_fonts(hwnd);
            let edit = make_edit(hwnd, font);
            let tip = make_tips(hwnd);
            let b = &mut (*st).bar;
            b.host = host as isize;
            b.font = font as isize;
            b.label_font = label_font as isize;
            b.edit = edit as isize;
            b.tip = tip as isize;
            BARS.with(|v| v.borrow_mut().push(hwnd as isize));
            Ok(EditorWindow { hwnd, host, st, ticker: None })
        }
    }

    /// Shows a bar's list, its ticked item and its text. Text being typed in the box stays
    /// until the box is left.
    pub fn set_bar(&self, items: &[String], ticked: Option<usize>, text: &str) {
        unsafe {
            let st = self.st;
            (*st).bar.items = items.to_vec();
            (*st).bar.ticked = ticked;
            let edit = (*st).bar.edit as HWND;
            if edit.is_null() {
                (*st).bar.text = text.to_string();
                return;
            }
            if GetFocus() == edit {
                (*st).bar.pending = Some(text.to_string());
                return;
            }
            (*st).bar.pending = None;
            (*st).bar.text = text.to_string();
            if get_text(edit) != text {
                set_text(edit, text);
            }
        }
    }

    /// What the person did in the bar since the last call.
    pub fn take_bar_events(&self) -> Vec<BarEvent> {
        unsafe { std::mem::take(&mut (*self.st).bar.events) }
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

/// Resizes an editor window so the area under its bar is `w`×`h`; the window is never
/// narrower than the bar (EditorWrapper, 0x140076240).
pub fn resize_for_plugin(hwnd: isize, w: i32, h: i32) {
    resize_client(hwnd, w.max(BAR_W), h.max(0) + BAR_H);
}

/// What a window's bar shows and what the person did in it.
#[derive(Default)]
struct BarState {
    items: Vec<String>,
    ticked: Option<usize>,
    /// The text the bar last showed (or the person last committed).
    text: String,
    /// Text the app gave while the person was typing, shown once they leave the box.
    pending: Option<String>,
    events: Vec<BarEvent>,
    /// The button under the pointer and the one held down: 0 import, 1 save.
    hover: Option<usize>,
    pressed: Option<usize>,
    tracking: bool,
    edit: isize,
    tip: isize,
    font: isize,
    label_font: isize,
    host: isize,
}

/// The bar's colours from the app's colour scheme, as premultiplied RGBA.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BarLook {
    pub button: [u8; 4],
    pub highlight: [u8; 4],
    pub editor_bg: [u8; 4],
    pub text: [u8; 4],
}

impl BarLook {
    /// The default colour scheme's.
    pub const DEFAULT: BarLook = BarLook { button: [0xEE, 0xEC, 0xE1, 0xFF], highlight: [121, 121, 176, 176], editor_bg: [255; 4], text: [0, 0, 0, 255] };
}

thread_local! {
    /// Wakes the app when a bar has something for it.
    static WAKER: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    static LOOK: Cell<BarLook> = const { Cell::new(BarLook::DEFAULT) };
    /// The open editor windows, to repaint when the colours change.
    static BARS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    /// Set while a window lays its plugin area out, so the area doesn't take it for the plugin.
    static LAYOUT: Cell<bool> = const { Cell::new(false) };
}

/// Sets what wakes the app when a bar has something for it.
pub fn set_waker(f: Box<dyn Fn()>) {
    WAKER.with(|w| *w.borrow_mut() = Some(f));
}

fn wake() {
    let _ = WAKER.try_with(|w| {
        if let Ok(w) = w.try_borrow() {
            if let Some(f) = w.as_ref() {
                f();
            }
        }
    });
}

/// Sets the bars' colours; the open windows repaint when they change.
pub fn set_bar_look(l: BarLook) {
    if LOOK.with(|c| c.replace(l)) == l {
        return;
    }
    let open = BARS.with(|v| v.borrow().clone());
    for h in open {
        unsafe {
            InvalidateRect(h as HWND, null(), 0);
            let st = state(h as HWND);
            if !st.is_null() && (*st).bar.edit != 0 {
                InvalidateRect((*st).bar.edit as HWND, null(), 1);
            }
        }
    }
}

type Rgb = [u8; 3];
const WHITE: [u8; 4] = [255; 4];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// `t` of the way from `a` to `b`, as JUCE's `Colour::interpolatedWith` (and widgets::lerp).
fn mix(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round().clamp(0.0, 255.0) as u8)
}

/// A premultiplied colour laid over an opaque one.
fn over(c: [u8; 4], bg: Rgb) -> Rgb {
    let k = 1.0 - c[3] as f32 / 255.0;
    std::array::from_fn(|i| (c[i] as f32 + bg[i] as f32 * k).round().clamp(0.0, 255.0) as u8)
}

fn hex(v: u32) -> Rgb {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

fn colorref(c: Rgb) -> u32 {
    c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16
}

/// A button's gradient ends and rim, as widgets::button_face draws them.
fn face(l: &BarLook, hover: bool, down: bool) -> ([u8; 4], [u8; 4], [u8; 4]) {
    let (mut top, mut bot) = (mix(l.button, WHITE, 0.35), mix(l.button, BLACK, 0.035));
    if hover {
        top = mix(top, l.highlight, 0.25);
        bot = mix(bot, l.highlight, 0.25);
    }
    if down {
        std::mem::swap(&mut top, &mut bot);
    }
    (top, bot, mix(l.button, BLACK, 0.46))
}

/// A bar button's icon colours: its holes show the middle of the button's face and the rest
/// is half as bright.
pub fn icon_colours(l: &BarLook, hover: bool) -> (Rgb, Rgb) {
    let (top, bot, _) = face(l, hover, false);
    let mid = over(mix(top, bot, 0.5), hex(BAR_BG));
    (mid, mid.map(|v| v / 2))
}

fn fill(dc: HDC, x: i32, y: i32, w: i32, h: i32, c: Rgb) {
    unsafe {
        SetDCBrushColor(dc, colorref(c));
        let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
        FillRect(dc, &r, GetStockObject(DC_BRUSH));
    }
}

/// Paints the bar into a window `cw` wide (PresetBar::paint, 0x140087250, and the look of the
/// app-drawn bar in presets::bar_ui).
unsafe fn paint_bar(dc: HDC, cw: i32, hover: Option<usize>, pressed: Option<usize>, label_font: HFONT) {
    let w = cw.max(1);
    let mem = CreateCompatibleDC(dc);
    let bmp = CreateCompatibleBitmap(dc, w, BAR_H);
    if mem.is_null() || bmp.is_null() {
        if !bmp.is_null() {
            DeleteObject(bmp);
        }
        if !mem.is_null() {
            DeleteDC(mem);
        }
        return;
    }
    let old_bmp = SelectObject(mem, bmp);
    let look = LOOK.with(Cell::get);
    fill(mem, 0, 0, w, BAR_H, hex(BAR_BG));
    fill(mem, 0, LINE_Y, w, 1, hex(BAR_LINE));

    let old_font = SelectObject(mem, label_font);
    SetBkMode(mem, TRANSPARENT as _);
    SetTextColor(mem, 0);
    let [x, y, lw, lh] = LABEL_TEXT;
    let mut r = RECT { left: x, top: y, right: x + lw, bottom: y + lh };
    let mut t: Vec<u16> = LABEL.encode_utf16().collect();
    DrawTextW(mem, t.as_mut_ptr(), t.len() as i32, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(mem, old_font);

    paint_combo(mem, &look);
    for (k, (a, icon)) in [(IMPORT_BUTTON, FOLDER), (SAVE_BUTTON, DISK)].into_iter().enumerate() {
        let hov = hover == Some(k);
        paint_button(mem, a, &look, hov, hov && pressed == Some(k), icon);
    }
    BitBlt(dc, 0, 0, w, BAR_H, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bmp);
    DeleteObject(bmp);
    DeleteDC(mem);
}

/// The list's face, as widgets::combo_face draws it.
unsafe fn paint_combo(dc: HDC, look: &BarLook) {
    let [x, y, w, h] = COMBO;
    fill(dc, x, y, w, h, over(look.editor_bg, hex(BAR_BG)));
    let (l, t, r, b) = (x, y, x + w, y + h);
    let (al, bl, br, mid) = (r - 20, r - 19, r - 2, (t + b).div_euclid(2));
    let rect = |x0: i32, y0: i32, x1: i32, y1: i32, c: u32| fill(dc, x0, y0, x1 - x0, y1 - y0, hex(c));
    rect(bl, t + 2, br, mid, 0xEEECE1);
    rect(bl, t + 2, br, t + 3, 0xF2F0E5);
    rect(bl, mid, br, b - 2, 0xE6E4D9);
    rect(bl, b - 2, br, b - 1, 0xEAE8DD);
    rect(al, t, al + 1, b, 0xC7C6C3);
    rect(l, t - 1, r, t, 0xF2F0E4);
    rect(l - 1, t, l, b, 0xF2EFE4);
    rect(r, t, r + 1, b, 0xF1EFE4);
    rect(l, b, r, b + 1, 0xE0DED5);
    rect(l, t, r, t + 1, 0xC7C6C0);
    rect(l + 1, t + 1, r - 1, t + 2, 0xCCCCCD);
    rect(l, t, l + 1, b, 0xA9A9A8);
    rect(l + 1, t + 2, l + 2, b - 1, 0xF7F7F7);
    rect(r - 2, t + 2, r - 1, b - 1, 0xC9C8BE);
    rect(r - 1, t + 1, r, b, 0xD1CFC5);
    rect(l, b - 1, r, b, 0xB2B2B3);
    let pen = CreatePen(PS_SOLID, 2, colorref(hex(0x6A6964)));
    let old = SelectObject(dc, pen);
    let (cx, cy) = (al + 10, mid);
    let pts = [POINT { x: cx - 5, y: cy - 3 }, POINT { x: cx, y: cy + 3 }, POINT { x: cx + 5, y: cy - 3 }];
    Polyline(dc, pts.as_ptr(), pts.len() as i32);
    SelectObject(dc, old);
    DeleteObject(pen);
}

/// A bar button: rounded, with a vertical gradient, a dark rim and its icon.
unsafe fn paint_button(dc: HDC, a: [i32; 4], look: &BarLook, hover: bool, down: bool, icon: &[IconPart]) {
    let [x, y, w, h] = a;
    let bg = hex(BAR_BG);
    let (top, bot, rim) = face(look, hover, down);
    // Corners of radius 4; the region leaves out its right and bottom edges.
    let rgn = CreateRoundRectRgn(x, y, x + w + 1, y + h + 1, 8, 8);
    SelectClipRgn(dc, rgn);
    for row in 0..h {
        let t = row as f32 / (h - 1).max(1) as f32;
        fill(dc, x, y + row, w, 1, over(mix(top, bot, t), bg));
    }
    let (hole, solid) = icon_colours(look, hover);
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    let old_brush = SelectObject(dc, GetStockObject(DC_BRUSH));
    for part in icon {
        let pts: Vec<POINT> = part.pts.iter().map(|&(px, py)| POINT { x: x + ICON_AT.0 + px, y: y + ICON_AT.1 + py }).collect();
        SetDCBrushColor(dc, colorref(if part.hole { hole } else { solid }));
        Polygon(dc, pts.as_ptr(), pts.len() as i32);
    }
    SelectObject(dc, old_brush);
    SelectObject(dc, old_pen);
    SelectClipRgn(dc, null_mut());
    SetDCBrushColor(dc, colorref(over(rim, bg)));
    FrameRgn(dc, rgn, GetStockObject(DC_BRUSH), 1, 1);
    DeleteObject(rgn);
}

unsafe fn state(hwnd: HWND) -> *mut WinState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WinState
}

fn mouse_at(lp: LPARAM) -> (i32, i32) {
    ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32)
}

fn inside(a: [i32; 4], x: i32, y: i32) -> bool {
    x >= a[0] && x < a[0] + a[2] && y >= a[1] && y < a[1] + a[3]
}

/// The button at a point: 0 import, 1 save.
fn hit_button(x: i32, y: i32) -> Option<usize> {
    [IMPORT_BUTTON, SAVE_BUTTON].iter().position(|&a| inside(a, x, y))
}

unsafe fn redraw_bar(hwnd: HWND) {
    let (cw, _) = client_size(hwnd as isize);
    let r = RECT { left: 0, top: 0, right: cw, bottom: BAR_H };
    InvalidateRect(hwnd, &r, 0);
}

unsafe fn get_text(h: HWND) -> String {
    let n = GetWindowTextLengthW(h).max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
    String::from_utf16_lossy(&buf[..got.min(n)])
}

unsafe fn set_text(h: HWND, s: &str) {
    let t = wide(s);
    SetWindowTextW(h, t.as_ptr());
}

/// The list of programs and presets, under the list's box (PresetBar's combo box). Picking an
/// item puts its name in the box and tells the app.
unsafe fn popup(hwnd: HWND) {
    let st = state(hwnd);
    if st.is_null() {
        return;
    }
    let items = (*st).bar.items.clone();
    let ticked = (*st).bar.ticked;
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    if items.is_empty() {
        let t = wide(NO_CHOICES);
        AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, t.as_ptr());
    } else {
        for (i, n) in items.iter().enumerate() {
            // A menu takes '&' for a shortcut mark, and an empty item would be a separator.
            let t = wide(&if n.is_empty() { " ".to_string() } else { n.replace('&', "&&") });
            AppendMenuW(menu, MF_STRING | if ticked == Some(i) { MF_CHECKED } else { 0 }, i + 1, t.as_ptr());
        }
    }
    let mut at = POINT { x: COMBO[0], y: COMBO[1] + COMBO[3] };
    ClientToScreen(hwnd, &mut at);
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_NONOTIFY, at.x, at.y, 0, hwnd, null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
    // The app may have closed the window or changed its list while the menu was open.
    if cmd <= 0 || IsWindow(hwnd) == 0 {
        return;
    }
    let st = state(hwnd);
    if st.is_null() || (*st).bar.items != items {
        return;
    }
    let k = cmd as usize - 1;
    (*st).bar.text = items[k].clone();
    (*st).bar.pending = None;
    let edit = (*st).bar.edit as HWND;
    if !edit.is_null() {
        set_text(edit, &items[k]);
    }
    (*st).bar.events.push(BarEvent::Pick(k));
    wake();
}

/// The box was left or Return pressed: new text goes to the app.
unsafe fn commit(hwnd: HWND) {
    let st = state(hwnd);
    if st.is_null() || (*st).bar.edit == 0 {
        return;
    }
    let edit = (*st).bar.edit as HWND;
    let now = get_text(edit);
    let pending = (*st).bar.pending.take();
    if now != (*st).bar.text {
        (*st).bar.text = now.clone();
        (*st).bar.events.push(BarEvent::Typed(now));
        wake();
    } else if let Some(p) = pending {
        // Nothing typed: what the app gave meanwhile shows now.
        (*st).bar.text = p.clone();
        if p != now {
            set_text(edit, &p);
        }
    }
}

/// The list's text box: Return keeps the text, Escape puts the old text back, and a click
/// into it selects all of it, as the original's label editor does.
unsafe extern "system" fn edit_proc(edit: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let old = GetWindowLongPtrW(edit, GWLP_USERDATA);
    let parent = GetParent(edit);
    match msg {
        WM_KEYDOWN if wp == VK_RETURN as usize => {
            commit(parent);
            SetFocus(parent);
            return 0;
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            let st = state(parent);
            if !st.is_null() {
                let back = (*st).bar.pending.take().unwrap_or_else(|| (*st).bar.text.clone());
                (*st).bar.text = back.clone();
                set_text(edit, &back);
            }
            SetFocus(parent);
            return 0;
        }
        // No beep for the keys handled above.
        WM_CHAR if wp == 0x0D || wp == 0x1B => return 0,
        WM_LBUTTONDOWN if GetFocus() != edit => {
            SetFocus(edit);
            SendMessageW(edit, EM_SETSEL, 0, -1);
            return 0;
        }
        _ => {}
    }
    if old == 0 {
        return DefWindowProcW(edit, msg, wp, lp);
    }
    CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(old), edit, msg, wp, lp)
}

unsafe fn make_edit(hwnd: HWND, font: HFONT) -> HWND {
    let class = wide("EDIT");
    let [x, y, w, h] = EDIT;
    let edit = CreateWindowExW(0, class.as_ptr(), null(), WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL, x, y, w, h, hwnd, null_mut(), GetModuleHandleW(null()), null());
    if edit.is_null() {
        return edit;
    }
    SendMessageW(edit, WM_SETFONT, font as WPARAM, 0);
    SendMessageW(edit, EM_SETMARGINS, EC_LEFTMARGIN | EC_RIGHTMARGIN, 0);
    let old = GetWindowLongPtrW(edit, GWLP_WNDPROC);
    SetWindowLongPtrW(edit, GWLP_USERDATA, old);
    SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize);
    edit
}

unsafe fn verdana(width: i32) -> HFONT {
    let mut lf: LOGFONTW = zeroed();
    lf.lfHeight = FONT as i32;
    lf.lfWidth = width;
    lf.lfWeight = 400;
    lf.lfQuality = CLEARTYPE_QUALITY as _;
    for (d, s) in lf.lfFaceName.iter_mut().zip("Verdana".encode_utf16()) {
        *d = s;
    }
    CreateFontIndirectW(&lf)
}

/// The bar's fonts: the box's, and the label's, squeezed sideways to fit its place as JUCE's
/// drawFittedText does (down to 70%).
unsafe fn make_fonts(hwnd: HWND) -> (HFONT, HFONT) {
    let font = verdana(0);
    let mut label = font;
    let dc = GetDC(hwnd);
    if dc.is_null() || font.is_null() {
        if !dc.is_null() {
            ReleaseDC(hwnd, dc);
        }
        return (font, label);
    }
    let old = SelectObject(dc, font);
    let t: Vec<u16> = LABEL.encode_utf16().collect();
    let mut size = SIZE { cx: 0, cy: 0 };
    let mut tm: TEXTMETRICW = zeroed();
    if GetTextExtentPoint32W(dc, t.as_ptr(), t.len() as i32, &mut size) != 0 && GetTextMetricsW(dc, &mut tm) != 0 && size.cx > LABEL_TEXT[2] {
        let k = (LABEL_TEXT[2] as f32 / size.cx as f32).max(0.7);
        let squeezed = verdana((tm.tmAveCharWidth as f32 * k).floor().max(1.0) as i32);
        if !squeezed.is_null() {
            label = squeezed;
        }
    }
    SelectObject(dc, old);
    ReleaseDC(hwnd, dc);
    (font, label)
}

fn init_controls() {
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_BAR_CLASSES };
        InitCommonControlsEx(&icc);
    });
}

/// The buttons' tooltips (PresetBar's constructor).
unsafe fn make_tips(hwnd: HWND) -> HWND {
    init_controls();
    let tip = CreateWindowExW(
        WS_EX_TOPMOST,
        TOOLTIPS_CLASSW,
        null(),
        WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        hwnd,
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    if tip.is_null() {
        return tip;
    }
    for (id, a, text) in [(1usize, IMPORT_BUTTON, TIP_IMPORT), (2, SAVE_BUTTON, TIP_SAVE)] {
        let mut t = wide(text);
        let mut ti: TTTOOLINFOW = zeroed();
        // The size without the last field, which older common controls don't know.
        ti.cbSize = (size_of::<TTTOOLINFOW>() - size_of::<*mut c_void>()) as u32;
        ti.uFlags = TTF_SUBCLASS;
        ti.hwnd = hwnd;
        ti.uId = id;
        ti.rect = RECT { left: a[0], top: a[1], right: a[0] + a[2], bottom: a[1] + a[3] };
        ti.lpszText = t.as_mut_ptr();
        SendMessageW(tip, TTM_ADDTOOLW, 0, &ti as *const TTTOOLINFOW as LPARAM);
    }
    tip
}

/// Puts the plugin area under the bar, over the rest of the window.
unsafe fn layout(hwnd: HWND) {
    let st = state(hwnd);
    if st.is_null() || (*st).bar.host == 0 {
        return;
    }
    let host = (*st).bar.host as HWND;
    let (cw, ch) = client_size(hwnd as isize);
    let was = LAYOUT.with(|l| l.replace(true));
    SetWindowPos(host, null_mut(), 0, BAR_H, cw, (ch - BAR_H).max(0), SWP_NOZORDER | SWP_NOACTIVATE);
    LAYOUT.with(|l| l.set(was));
}

fn holder_class() -> *const u16 {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    NAME.get_or_init(|| {
        let n = wide("PedalboardHostPluginArea");
        unsafe {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(holder_proc),
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

/// The plugin area. A plugin that resizes the window it was given resizes the editor window
/// around it, bar included.
unsafe extern "system" fn holder_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_WINDOWPOSCHANGED if !LAYOUT.with(Cell::get) => {
            let pos = &*(lp as *const WINDOWPOS);
            let (flags, cx, cy) = (pos.flags, pos.cx, pos.cy);
            let parent = GetParent(h);
            if !parent.is_null() {
                if flags & SWP_NOSIZE == 0 {
                    let (cw, ch) = client_size(parent as isize);
                    if (cx, cy) != (cw, (ch - BAR_H).max(0)) {
                        resize_for_plugin(parent as isize, cx, cy);
                    }
                }
                if flags & (SWP_NOSIZE | SWP_NOMOVE) != (SWP_NOSIZE | SWP_NOMOVE) {
                    layout(parent);
                }
            }
        }
        WM_LBUTTONDOWN => {
            SetFocus(GetParent(h));
        }
        _ => {}
    }
    DefWindowProcW(h, msg, wp, lp)
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        // The ticker stops before the window goes, so it never posts to a window that's gone.
        if let Some(t) = self.ticker.take() {
            t.stop();
        }
        let me = self.hwnd as isize;
        let _ = BARS.try_with(|v| {
            if let Ok(mut v) = v.try_borrow_mut() {
                v.retain(|&h| h != me);
            }
        });
        unsafe {
            KillTimer(self.hwnd, 1);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            // Its text box, plugin area and tooltips go with it.
            DestroyWindow(self.hwnd);
            let st = Box::from_raw(self.st);
            if st.bar.label_font != st.bar.font {
                DeleteObject(st.bar.label_font as HFONT);
            }
            DeleteObject(st.bar.font as HFONT);
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

    #[test]
    fn the_bar_shows_its_text_passes_on_what_the_person_does_and_follows_the_plugin() {
        let win = EditorWindow::new("test", 300, 150, 0, false, false).unwrap();
        // Never narrower than the bar, which sits above the plugin's area.
        assert_eq!((client_size(win.hwnd as isize), client_size(win.host as isize)), ((396, 182), (396, 150)));
        win.set_bar(&["One".to_string(), "Two".to_string()], Some(1), "Two");
        let edit = unsafe { (*win.st).bar.edit } as HWND;
        assert_eq!(unsafe { get_text(edit) }, "Two");
        // A click on the save button.
        let at = (16 << 16 | 380) as LPARAM;
        unsafe {
            SendMessageW(win.hwnd, WM_LBUTTONDOWN, 1, at);
            SendMessageW(win.hwnd, WM_LBUTTONUP, 0, at);
        }
        assert_eq!(win.take_bar_events(), vec![BarEvent::Save]);
        assert!(win.take_bar_events().is_empty());
        // Pressed on the import button but let go elsewhere: nothing.
        unsafe {
            SendMessageW(win.hwnd, WM_LBUTTONDOWN, 1, (16 << 16 | 350) as LPARAM);
            SendMessageW(win.hwnd, WM_LBUTTONUP, 0, (16 << 16 | 200) as LPARAM);
        }
        assert!(win.take_bar_events().is_empty());
        // Typed text goes on Return, once.
        unsafe {
            set_text(edit, "Lead");
            SendMessageW(edit, WM_KEYDOWN, VK_RETURN as WPARAM, 0);
            SendMessageW(edit, WM_KEYDOWN, VK_RETURN as WPARAM, 0);
        }
        assert_eq!(win.take_bar_events(), vec![BarEvent::Typed("Lead".into())]);
        // Escape puts the text back.
        unsafe {
            set_text(edit, "Oops");
            SendMessageW(edit, WM_KEYDOWN, VK_ESCAPE as WPARAM, 0);
            assert_eq!(get_text(edit), "Lead");
        }
        assert!(win.take_bar_events().is_empty());
        // A plugin that resizes the area it was given resizes the window around it.
        unsafe { SetWindowPos(win.host, null_mut(), 0, 0, 500, 300, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        assert_eq!((client_size(win.hwnd as isize), client_size(win.host as isize)), ((500, 332), (500, 300)));
    }
}
