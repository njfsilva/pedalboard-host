//! Windows of their own. The dialogs, editors, prompts and drop-down lists the app draws itself
//! open as separate desktop windows (egui's immediate viewports), so a small main window never
//! cuts one off. Each one belongs to the main window (or to the dialog that opened it): it stays
//! in front of its owner and hides and comes back with it. The original's dialogs each get a
//! taskbar button as well (DocumentWindow::getDesktopWindowStyleFlags, 0x14021c5a0); leaving
//! that out is the rebuild's own, so a dialog can't end up behind other programs. With
//! `--screenshot` they are drawn inside the main window instead, so a picture of the window
//! shows them.

use crate::bars::{self, Row};
use crate::colours::{self, Scheme};
use crate::widgets::{combo_face, font, text_fit};
use egui::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetActiveWindow, IsWindowEnabled};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, EnumThreadWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SetWindowLongPtrW, SetWindowPos, ShowWindowAsync, GWLP_HWNDPARENT, GWLP_WNDPROC, GWL_EXSTYLE, GW_OWNER, STYLESTRUCT, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, WM_NCDESTROY, WM_STYLECHANGING, WNDPROC, WS_EX_APPWINDOW,
};

/// Room for a title bar and borders around a window's inside until its real frame is known.
const FRAME_GUESS: Vec2 = vec2(16.0, 39.0);
/// How far inside the screen's usable area a dialog opens. The original centres each dialog on
/// the window that opened it, then moves it, and shrinks it if it must, into the monitor's area
/// reduced by 12 px (centreAroundComponent at 0x14021f700, constrainedWithin at 0x14022da20).
const DIALOG_GAP: f32 = 12.0;
/// The smallest a resizable self-sized window gets (the original sets no limit; Windows' own
/// smallest window applies there).
const FIT_MIN: Vec2 = vec2(120.0, 60.0);
/// How soon after a list closed by losing the focus a click on its box still counts as the
/// click that closed it.
const RECLICK: Duration = Duration::from_millis(300);
/// The class winit gives its windows, to tell them from the app's other windows.
const WINIT_CLASS: &str = "Window Class";

static MAIN: AtomicIsize = AtomicIsize::new(0);
/// Windows whose procedure was replaced to keep them off the taskbar: (window, old procedure).
static PROCS: Mutex<Vec<(isize, isize)>> = Mutex::new(Vec::new());
/// Windows hidden with the main window when it went to the tray.
static HIDDEN: Mutex<Vec<isize>> = Mutex::new(Vec::new());

thread_local! {
    static SLOTS: RefCell<HashMap<ViewportId, Slot>> = RefCell::new(HashMap::new());
    /// Windows disabled behind an open prompt.
    static BLOCKED: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    static MODAL: Cell<bool> = const { Cell::new(false) };
    static ROOT_FOCUSED: Cell<Option<bool>> = const { Cell::new(None) };
}

/// What the app knows about one open window.
#[derive(Clone, Copy, Default)]
struct Slot {
    hwnd: isize,
    /// The main window's pass it was last shown in.
    pass: u64,
    /// 0 new, 1 made (hidden and owned), 2 sized and placed, 3 on screen.
    stage: u8,
    content: Option<Vec2>,
    outer: Option<Rect>,
    inner: Option<Rect>,
    /// The inside size asked for, and where the window was put (points on the screen).
    size: Vec2,
    pos: Option<Pos2>,
    ppp: f32,
    focused: Option<bool>,
    focused_once: bool,
    top: bool,
    /// A text box had the keys, or a list was open, when its last pass ended: Escape is theirs.
    busy: bool,
}

pub fn set_main(h: isize) {
    MAIN.store(h, Ordering::Relaxed);
}

pub fn main_hwnd() -> isize {
    MAIN.load(Ordering::Relaxed)
}

/// The window id for something the app shows as a window of its own.
pub fn id(src: impl Hash) -> ViewportId {
    ViewportId::from_hash_of(("subwin", src))
}

/// Whether windows are drawn inside the main window (for `--screenshot`).
pub fn embedded(ctx: &Context) -> bool {
    ctx.embed_viewports()
}

/// Set while a prompt is open, so the main window ignores keys and clicks meant for it.
pub fn set_modal(on: bool) {
    MODAL.with(|m| m.set(on));
}

pub fn modal() -> bool {
    MODAL.with(|m| m.get())
}

/// Whether a window is on screen.
pub fn shown(vid: ViewportId) -> bool {
    SLOTS.with(|s| s.borrow().get(&vid).is_some_and(|v| v.stage >= 3))
}

/// A window going to the back lets go of the keyboard, so a text box in it finishes its typing
/// as a JUCE text box does when its window loses the focus.
fn let_go(ctx: &Context, was: Option<bool>, now: Option<bool>) {
    if was == Some(true) && now == Some(false) {
        if let Some(id) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
    }
}

/// The same for the main window: call it in each of its passes before anything is drawn.
pub fn root_focus(ctx: &Context) {
    let now = ctx.input(|i| i.viewport().focused);
    let was = ROOT_FOCUSED.with(|f| f.replace(now));
    let_go(ctx, was, now);
}

/// The native handle of a window (the main window for ROOT), or 0.
pub fn hwnd_of(vid: ViewportId) -> isize {
    if vid == ViewportId::ROOT {
        return main_hwnd();
    }
    SLOTS.with(|s| s.borrow().get(&vid).map_or(0, |v| v.hwnd))
}

#[derive(Clone, Copy)]
pub enum Kind {
    /// As big as what's in it: up to `width` wide (at least `min_width`) and as tall as it
    /// needs, scrolled where the screen is too small. When `resizable`, the person may drag its
    /// edges too, as every dialog of the original allows (setResizable(true, false), 0x14021d340).
    Fit { width: f32, min_width: f32, resizable: bool },
    /// Starts at `size`, never past the screen; the person may resize it when `resizable`.
    /// `scroll` scrolls what's in it.
    Fill { size: Vec2, min: Vec2, resizable: bool, scroll: bool },
    /// No title bar or border, exactly `size`: prompts and lists draw their own.
    Bare { size: Vec2 },
}

impl Kind {
    pub fn fit(width: f32) -> Self {
        Kind::Fit { width, min_width: 0.0, resizable: false }
    }

    pub fn fit_between(min_width: f32, width: f32) -> Self {
        Kind::Fit { width, min_width, resizable: false }
    }

    /// A self-sized window the person may resize as well.
    pub fn sizable(self) -> Self {
        match self {
            Kind::Fit { width, min_width, .. } => Kind::Fit { width, min_width, resizable: true },
            k => k,
        }
    }

    pub fn fill(size: Vec2, min: Vec2, resizable: bool) -> Self {
        Kind::Fill { size, min, resizable, scroll: false }
    }

    pub fn scrolled(self) -> Self {
        match self {
            Kind::Fill { size, min, resizable, .. } => Kind::Fill { size, min, resizable, scroll: true },
            k => k,
        }
    }
}

#[derive(Clone, Copy)]
enum Place {
    /// In the middle of its owner, `DIALOG_GAP` inside the screen's usable area.
    Centre,
    /// Its top left at `pos`, or ending at `above` when there's no room below.
    At { pos: Pos2, above: f32 },
    /// Where Windows puts a new window, kept on the screen.
    Auto,
}

pub struct Spec<'a> {
    id: ViewportId,
    title: &'a str,
    kind: Kind,
    margin: Margin,
    fill: Option<Color32>,
    on_top: bool,
    owner: ViewportId,
    place: Place,
    escape: bool,
}

impl<'a> Spec<'a> {
    pub fn new(id: ViewportId, title: &'a str, kind: Kind) -> Self {
        Spec { id, title, kind, margin: Margin::same(10), fill: None, on_top: false, owner: ViewportId::ROOT, place: Place::Centre, escape: false }
    }

    /// Escape closes it, as it does the original's dialogs opened with escapeKeyTriggersCloseButton
    /// (DialogWindow::showModalDialog at 0x140030940, the non-modal helper at 0x140030b30), unless
    /// a text box has the keys or a list is open: in JUCE those take the key first.
    pub fn escape(mut self) -> Self {
        self.escape = true;
        self
    }

    pub fn margin(mut self, m: Margin) -> Self {
        self.margin = m;
        self
    }

    pub fn fill(mut self, c: Color32) -> Self {
        self.fill = Some(c);
        self
    }

    pub fn on_top(mut self, on: bool) -> Self {
        self.on_top = on;
        self
    }

    pub fn owner(mut self, o: ViewportId) -> Self {
        self.owner = o;
        self
    }

    pub fn at(mut self, pos: Pos2, above: f32) -> Self {
        self.place = Place::At { pos, above };
        self
    }

    pub fn auto(mut self) -> Self {
        self.place = Place::Auto;
        self
    }
}

/// Shows a window this pass and returns whether the person asked to close it (its close button
/// or Alt+F4). Call it every pass while the window is open, from the main window's update and
/// never from inside another window. `add` can run more than once a pass, with the clicks and
/// keys only in the first run, so what it reports must stick (`x |= clicked`).
pub fn show(ctx: &Context, spec: Spec, mut add: impl FnMut(&mut Ui)) -> bool {
    if embedded(ctx) {
        return show_inside(ctx, &spec, &mut add);
    }
    let vid = spec.id;
    let now = ctx.cumulative_pass_nr_for(ViewportId::ROOT);
    let mut slot = SLOTS.with(|s| s.borrow_mut().remove(&vid)).unwrap_or_default();
    if slot.stage > 0 && slot.pass + 1 < now {
        slot = Slot::default();
    }
    slot.top = spec.on_top;
    let owner = match hwnd_of(spec.owner) {
        0 => main_hwnd(),
        h => h,
    };
    let on_screen = slot.stage >= 3 && slot.hwnd != 0;
    let ppp = if on_screen && slot.ppp > 0.0 { slot.ppp } else { owner_ppp(ctx, spec.owner) };
    let screen = work_area(if on_screen { slot.hwnd } else { owner }, ppp)
        .unwrap_or_else(|| Rect::from_min_size(Pos2::ZERO, ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1920.0, 1040.0))));
    let work = open_area(spec.place, screen);
    let bare = matches!(spec.kind, Kind::Bare { .. });
    let extra = match (slot.outer, slot.inner) {
        _ if bare => Vec2::ZERO,
        (Some(o), Some(i)) => (o.size() - i.size()).max(Vec2::ZERO),
        _ => FRAME_GUESS,
    };
    let room = (work.size() - extra).max(vec2(120.0, 80.0));
    // A self-sized window lays out as if it were as tall as the screen allows, so lists in it
    // with a height limit get their full height instead of the window's height so far.
    let tall = (room.y - spec.margin.sum().y).max(40.0);
    let target = match spec.kind {
        Kind::Fit { width, min_width, .. } => {
            let c = slot.content.unwrap_or(vec2(width, 200.0));
            let mut s = vec2(c.x.max(min_width), c.y) + spec.margin.sum();
            if s.y > room.y {
                s.x += ctx.style().spacing.scroll.allocated_width() + 4.0;
            }
            // A point to spare, so rounding to pixels never brings in a scroll bar.
            (s + Vec2::splat(1.0)).min(room).ceil()
        }
        Kind::Fill { size, .. } if slot.stage < 3 => size.min(room).round(),
        Kind::Fill { .. } => slot.size,
        Kind::Bare { size } => size.ceil(),
    };

    let main = main_hwnd();
    let ready = (main == 0 || unsafe { IsWindowVisible(main as HWND) != 0 && IsIconic(main as HWND) == 0 })
        && (spec.owner == ViewportId::ROOT || shown(spec.owner));
    if slot.stage == 0 {
        slot.size = target;
    } else if slot.stage == 2 || (slot.stage == 1 && ready) {
        // Hidden until it has its size and place, so it never shows up in the wrong spot.
        slot.size = target;
        let outer = target + extra;
        let pos = match spec.place {
            Place::Centre => owner_rect(ctx, spec.owner).unwrap_or(work).center() - outer / 2.0,
            Place::At { pos, above } if pos.y + outer.y > work.bottom() => pos2(pos.x, above - outer.y),
            Place::At { pos, .. } => pos,
            Place::Auto => slot.outer.map_or(work.center() - outer / 2.0, |o| o.min),
        };
        slot.pos = Some(clamp_pos(pos, outer, work));
        slot.stage += 1;
    } else if slot.stage >= 3 && (target - slot.size).abs().max_elem() > 0.5 {
        // What's in it changed size: follow it and keep the window on the screen, without pulling
        // one the person moved to the edge back in by the gap.
        slot.size = target;
        if let Some(o) = slot.outer {
            let p = clamp_pos(o.min, target + extra, screen);
            if (p - o.min).length() > 0.5 {
                slot.pos = Some(p);
                ctx.send_viewport_cmd_to(vid, ViewportCommand::OuterPosition(p));
            }
        }
    }
    if slot.stage < 3 {
        if slot.stage == 1 && !ready {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint();
        }
    }

    // Everything that would make egui build the window again stays the same every pass.
    let mut b = ViewportBuilder::default()
        .with_title(spec.title)
        .with_visible(slot.stage >= 3)
        .with_active(true)
        .with_taskbar(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_inner_size(slot.size)
        .with_window_level(if spec.on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
    if let Some(p) = slot.pos {
        b = b.with_position(p);
    }
    b = match spec.kind {
        Kind::Fit { resizable: true, .. } => b.with_resizable(true).with_min_inner_size(FIT_MIN.min(slot.size)),
        Kind::Fit { .. } => b.with_resizable(false),
        Kind::Fill { min, resizable, .. } => b.with_resizable(resizable).with_min_inner_size(min.min(slot.size)),
        Kind::Bare { .. } => b.with_decorations(false).with_resizable(false),
    };

    let fill = spec.fill.unwrap_or(ctx.style().visuals.window_fill);
    let (kind, margin) = (spec.kind, spec.margin);
    let mut rec = Slot::default();
    let mut close = false;
    let (escape, busy, mut esc) = (spec.escape, slot.busy, false);
    let was_focused = slot.focused;
    let thread = unsafe { GetCurrentThreadId() };
    let before = (slot.stage == 0).then(|| thread_windows(thread));
    ctx.show_viewport_immediate(vid, b, |ctx, class| {
        if matches!(class, ViewportClass::Embedded) {
            close |= show_inside(ctx, &spec, &mut add);
            return;
        }
        close |= ctx.input(|i| i.viewport().close_requested());
        esc |= ctx.input(|i| i.key_pressed(Key::Escape) && i.modifiers.is_none());
        ctx.input(|i| {
            let v = i.viewport();
            rec.outer = v.outer_rect;
            rec.inner = v.inner_rect;
            rec.focused = v.focused;
        });
        let_go(ctx, was_focused, rec.focused);
        rec.ppp = ctx.pixels_per_point();
        CentralPanel::default().frame(Frame::NONE.fill(fill).inner_margin(margin)).show(ctx, |ui| {
            ui.push_id(vid, |ui| match kind {
                Kind::Fit { width, .. } => {
                    ScrollArea::both().auto_shrink([false, false]).drag_to_scroll(false).show(ui, |ui| {
                        let r = ui.scope(|ui| {
                            ui.set_max_width(width);
                            ui.set_max_height(tall);
                            add(ui);
                        });
                        rec.content = Some(r.response.rect.size());
                    });
                }
                Kind::Fill { scroll: true, .. } => {
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| add(ui));
                }
                _ => add(ui),
            });
        });
        rec.busy |= typing(ctx) || ctx.memory(|m| m.any_popup_open());
    });
    if let Some(before) = before {
        let h = thread_windows(thread).into_iter().find(|h| !before.contains(h)).unwrap_or(0);
        if h != 0 {
            adopt(h, owner);
        }
        slot.hwnd = h;
        slot.stage = 1;
    }
    close |= ctx.input(|i| i.raw.viewports.get(&vid).is_some_and(|v| v.close_requested()));
    // egui takes the focus off a text box as Escape arrives, so whether one had the key is what
    // the last pass left.
    close |= escape && esc && !busy;
    slot.busy = rec.busy;
    slot.outer = rec.outer.or(slot.outer);
    slot.inner = rec.inner.or(slot.inner);
    if rec.ppp > 0.0 {
        slot.ppp = rec.ppp;
    }
    if let Some(f) = rec.focused {
        slot.focused = Some(f);
        slot.focused_once |= f && slot.stage >= 3;
    }
    if rec.content.is_some() {
        slot.content = rec.content;
    }
    slot.pass = now;
    SLOTS.with(|s| s.borrow_mut().insert(vid, slot));
    close
}

/// Whether a text box has the keyboard in this window.
pub fn typing(ctx: &Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|id| TextEdit::load_state(ctx, id).is_some())
}

/// The same window drawn inside the main window.
fn show_inside(ctx: &Context, spec: &Spec, add: &mut dyn FnMut(&mut Ui)) -> bool {
    let fill = spec.fill.unwrap_or(ctx.style().visuals.window_fill);
    let frame = Frame::window(&ctx.style()).fill(fill).inner_margin(spec.margin);
    let mut open = true;
    match spec.kind {
        Kind::Fit { width, .. } => {
            Window::new(spec.title).id(spec.id.0).open(&mut open).collapsible(false).resizable(false).frame(frame).show(ctx, |ui| {
                ui.set_max_width(width);
                add(ui);
            });
        }
        Kind::Fill { size, min, resizable, scroll } => {
            Window::new(spec.title)
                .id(spec.id.0)
                .open(&mut open)
                .collapsible(false)
                .resizable(resizable)
                .default_size(size)
                .min_size(min)
                .frame(frame)
                .show(ctx, |ui| {
                    if scroll {
                        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| add(ui));
                    } else {
                        add(ui);
                    }
                });
        }
        Kind::Bare { size } => {
            let inner = Frame::NONE.fill(fill).inner_margin(spec.margin);
            match spec.place {
                Place::At { pos, above } => {
                    let p = if pos.y + size.y > ctx.screen_rect().bottom() { pos2(pos.x, above - size.y) } else { pos };
                    Area::new(spec.id.0).order(Order::Foreground).fixed_pos(p).show(ctx, |ui| {
                        inner.show(ui, |ui| add(ui));
                    });
                }
                _ => {
                    Modal::new(spec.id.0).frame(inner).backdrop_color(Color32::TRANSPARENT).show(ctx, |ui| add(ui));
                }
            }
        }
    }
    !open
}

/// Ends the windows' pass: forgets the ones not shown in it and disables the windows behind an
/// open prompt (`blocked`), turning them back on once it's gone. Call it once a pass, after the
/// last window.
pub fn end_frame(ctx: &Context, blocked: &[ViewportId]) {
    if embedded(ctx) {
        return;
    }
    let now = ctx.cumulative_pass_nr_for(ViewportId::ROOT);
    let gone: Vec<isize> = SLOTS.with(|s| {
        let mut s = s.borrow_mut();
        let gone = s.values().filter(|v| v.pass != now && v.hwnd != 0).map(|v| v.hwnd).collect();
        s.retain(|_, v| v.pass == now);
        gone
    });
    let main = main_hwnd();
    if !gone.is_empty() && main != 0 {
        // A window closing takes the windows it owns with it, so those move to the main window.
        SLOTS.with(|s| {
            for v in s.borrow().values().filter(|v| v.hwnd != 0) {
                let o = unsafe { GetWindow(v.hwnd as HWND, GW_OWNER) } as isize;
                if gone.contains(&o) {
                    unsafe { SetWindowLongPtrW(v.hwnd as HWND, GWLP_HWNDPARENT, main) };
                }
            }
        });
    }
    let want: Vec<isize> = blocked.iter().map(|&v| hwnd_of(v)).filter(|&h| h != 0).collect();
    BLOCKED.with(|b| {
        let mut b = b.borrow_mut();
        for &h in b.iter().filter(|h| !want.contains(h)) {
            unsafe { EnableWindow(h as HWND, 1) };
        }
        for &h in want.iter().filter(|h| !b.contains(h)) {
            unsafe { EnableWindow(h as HWND, 0) };
        }
        *b = want;
    });
}

/// Brings a window to the front.
pub fn focus(ctx: &Context, vid: ViewportId) {
    if embedded(ctx) {
        ctx.move_to_top(LayerId::new(Order::Middle, vid.0));
    } else if shown(vid) {
        ctx.send_viewport_cmd_to(vid, ViewportCommand::Focus);
    }
}

/// Where a rectangle in `ui`'s window is on the screen.
pub fn to_screen(ui: &Ui, r: Rect) -> Rect {
    if embedded(ui.ctx()) {
        return r;
    }
    let at = ui.ctx().input(|i| i.viewport().inner_rect).map_or(Vec2::ZERO, |ir| ir.min.to_vec2());
    r.translate(at)
}

/// The room a window has: the main window's screen without the taskbar.
pub fn work_size(ctx: &Context) -> Vec2 {
    if embedded(ctx) {
        return ctx.screen_rect().size();
    }
    work_area(main_hwnd(), ctx.pixels_per_point()).map_or(ctx.screen_rect().size(), |w| w.size())
}

fn owner_ppp(ctx: &Context, owner: ViewportId) -> f32 {
    let p = if owner == ViewportId::ROOT { 0.0 } else { SLOTS.with(|s| s.borrow().get(&owner).map_or(0.0, |v| v.ppp)) };
    if p > 0.0 {
        p
    } else {
        ctx.pixels_per_point()
    }
}

fn owner_rect(ctx: &Context, owner: ViewportId) -> Option<Rect> {
    if owner == ViewportId::ROOT {
        ctx.input(|i| i.viewport().outer_rect)
    } else {
        SLOTS.with(|s| s.borrow().get(&owner).and_then(|v| v.outer))
    }
}

fn clamp_pos(p: Pos2, outer: Vec2, work: Rect) -> Pos2 {
    pos2(p.x.min(work.right() - outer.x).max(work.left()), p.y.min(work.bottom() - outer.y).max(work.top())).round()
}

/// The area a window opens in and is sized to: a dialog keeps `DIALOG_GAP` clear of the screen's
/// edges, while a list under its box or a window Windows places may use all of it.
fn open_area(place: Place, screen: Rect) -> Rect {
    match place {
        Place::Centre => screen.shrink(DIALOG_GAP),
        Place::At { .. } | Place::Auto => screen,
    }
}

/// The screen area without the taskbar around a window, in points.
fn work_area(h: isize, ppp: f32) -> Option<Rect> {
    if h == 0 || ppp <= 0.0 {
        return None;
    }
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let ok = unsafe { GetMonitorInfoW(MonitorFromWindow(h as HWND, MONITOR_DEFAULTTONEAREST), &mut mi) };
    if ok == 0 {
        return None;
    }
    let r = mi.rcWork;
    Some(Rect::from_min_max(pos2(r.left as f32 / ppp, r.top as f32 / ppp), pos2(r.right as f32 / ppp, r.bottom as f32 / ppp)))
}

/// winit's windows on a thread.
fn thread_windows(thread: u32) -> Vec<isize> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        let mut buf = [0u16; 32];
        let n = GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32);
        if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == WINIT_CLASS {
            (*(lp as *mut Vec<isize>)).push(h as isize);
        }
        1
    }
    let mut v: Vec<isize> = Vec::new();
    unsafe { EnumThreadWindows(thread, Some(each), &mut v as *mut Vec<isize> as LPARAM) };
    v
}

/// Makes a new window belong to `owner` and keeps it off the taskbar: winit marks its windows
/// as app windows whenever it sets their style, so the flag is taken out as it's set.
fn adopt(h: isize, owner: isize) {
    let hw = h as HWND;
    unsafe {
        let ours = no_taskbar_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT as usize as isize;
        let old = GetWindowLongPtrW(hw, GWLP_WNDPROC);
        if old != 0 && old != ours {
            PROCS.lock().unwrap_or_else(|e| e.into_inner()).push((h, old));
            SetWindowLongPtrW(hw, GWLP_WNDPROC, ours);
        }
        let ex = GetWindowLongPtrW(hw, GWL_EXSTYLE);
        SetWindowLongPtrW(hw, GWL_EXSTYLE, ex & !(WS_EX_APPWINDOW as isize));
        if owner != 0 && owner != h {
            SetWindowLongPtrW(hw, GWLP_HWNDPARENT, owner);
        }
        SetWindowPos(hw, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

unsafe extern "system" fn no_taskbar_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_STYLECHANGING && wp as i32 == GWL_EXSTYLE && lp != 0 {
        (*(lp as *mut STYLESTRUCT)).styleNew &= !WS_EX_APPWINDOW;
    }
    let old = PROCS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|e| e.0 == h as isize).map_or(0, |e| e.1);
    let r = if old != 0 { CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(old), h, msg, wp, lp) } else { DefWindowProcW(h, msg, wp, lp) };
    if msg == WM_NCDESTROY {
        PROCS.lock().unwrap_or_else(|e| e.into_inner()).retain(|e| e.0 != h as isize);
    }
    r
}

fn owned_by(h: isize, main: isize) -> bool {
    let mut o = h;
    for _ in 0..16 {
        o = unsafe { GetWindow(o as HWND, GW_OWNER) } as isize;
        if o == 0 {
            return false;
        }
        if o == main {
            return true;
        }
    }
    false
}

/// Hides the windows that belong to the main window as it goes to the tray (Windows leaves an
/// owned window on screen when its owner is only hidden), for show_owned to bring back.
pub fn hide_owned(main: isize) {
    if main == 0 {
        return;
    }
    let thread = unsafe { GetWindowThreadProcessId(main as HWND, std::ptr::null_mut()) };
    let mut hidden = HIDDEN.lock().unwrap_or_else(|e| e.into_inner());
    for h in thread_windows(thread) {
        if h != main && unsafe { IsWindowVisible(h as HWND) } != 0 && owned_by(h, main) {
            unsafe { ShowWindowAsync(h as HWND, SW_HIDE) };
            hidden.push(h);
        }
    }
}

/// Shows again the windows hide_owned hid, without taking the focus.
pub fn show_owned(main: isize) {
    let list = std::mem::take(&mut *HIDDEN.lock().unwrap_or_else(|e| e.into_inner()));
    for h in list {
        if unsafe { IsWindow(h as HWND) } != 0 && owned_by(h, main) {
            unsafe { ShowWindowAsync(h as HWND, SW_SHOWNOACTIVATE) };
        }
    }
}

/// A window to put a file chooser in front of.
pub struct Parent(pub isize);

impl raw_window_handle::HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let h = std::num::NonZeroIsize::new(self.0).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let raw = raw_window_handle::RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(h));
        // The handle belongs to a window that outlives the chooser, which is modal.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

impl raw_window_handle::HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::windows())
    }
}

/// Runs a file chooser in front of `parent` (0: the window in front) with the app's other
/// windows turned off until it closes, so nothing changes under it.
pub fn chooser<R>(parent: isize, d: rfd::FileDialog, run: impl FnOnce(rfd::FileDialog) -> R) -> R {
    let parent = match parent {
        0 => match unsafe { GetActiveWindow() } as isize {
            0 => main_hwnd(),
            a => a,
        },
        p => p,
    };
    let d = if parent != 0 { d.set_parent(&Parent(parent)) } else { d };
    let mut ours: Vec<isize> = SLOTS.with(|s| s.borrow().values().map(|v| v.hwnd).collect());
    ours.push(main_hwnd());
    let off: Vec<isize> = ours.into_iter().filter(|&h| h != 0 && h != parent && unsafe { IsWindowEnabled(h as HWND) } != 0).collect();
    for &h in &off {
        unsafe { EnableWindow(h as HWND, 0) };
    }
    let r = run(d);
    for &h in &off {
        unsafe { EnableWindow(h as HWND, 1) };
    }
    r
}

pub fn pick_file(d: rfd::FileDialog) -> Option<PathBuf> {
    chooser(0, d, |d| d.pick_file())
}

pub fn pick_files(d: rfd::FileDialog) -> Option<Vec<PathBuf>> {
    chooser(0, d, |d| d.pick_files())
}

pub fn pick_folder(d: rfd::FileDialog) -> Option<PathBuf> {
    chooser(0, d, |d| d.pick_folder())
}

pub fn save_file(d: rfd::FileDialog) -> Option<PathBuf> {
    chooser(0, d, |d| d.save_file())
}

/// A drop-down box's list: where the box is on the screen while the list is open.
#[derive(Default)]
pub struct DropDown {
    pub open: Option<Rect>,
    hot: Option<usize>,
    closed_at: Option<Instant>,
}

impl DropDown {
    /// Opens or closes the list under `r`, a box in `ui`, as a click on the box does.
    pub fn toggle(&mut self, ui: &Ui, r: Rect) {
        if self.open.take().is_some() {
            return;
        }
        // The press that took the focus from the open list already closed it.
        if self.closed_at.take().is_some_and(|t| t.elapsed() < RECLICK) {
            return;
        }
        self.open = Some(to_screen(ui, r));
        self.hot = None;
    }

    pub fn close(&mut self) {
        self.open = None;
        self.hot = None;
    }
}

/// A drop-down box in the original's combo style showing `text`. A click opens or closes its
/// list, which drop_list shows.
pub fn combo(ui: &mut Ui, r: Rect, id: Id, text: &str, dd: &mut DropDown, c: &Scheme) -> Response {
    let p = ui.painter();
    let tr = combo_face(p, r, c);
    text_fit(p, tr.left() + 5.0, tr.width() - 7.0, bars::baseline(tr, 15.0), Align::Min, text, font(15.0), c.get(colours::TEXT));
    let resp = ui.interact(r, id, Sense::click());
    if resp.clicked() {
        dd.toggle(ui, r);
    }
    resp
}

/// A box's open list as a window of its own under the box (above it where the screen ends),
/// at least `min_w` wide and as wide as the box. Returns the row picked; the list closes on a
/// pick, Escape, or a click anywhere else.
pub fn drop_list(ctx: &Context, vid: ViewportId, owner: ViewportId, rows: &[Row], dd: &mut DropDown, c: &Scheme, min_w: f32) -> Option<usize> {
    let r = dd.open?;
    let full = bars::popup_size(ctx, rows, min_w.max(r.width()));
    let room = if embedded(ctx) {
        ctx.screen_rect().height()
    } else {
        let h = match hwnd_of(owner) {
            0 => main_hwnd(),
            h => h,
        };
        work_area(h, owner_ppp(ctx, owner)).map_or(full.y, |w| w.height())
    };
    let size = vec2(full.x, full.y.min(room));
    let top = SLOTS.with(|s| s.borrow().get(&owner).is_some_and(|v| v.top));
    let (mut pick, mut hot, mut esc) = (None, dd.hot, false);
    let spec = Spec::new(vid, "", Kind::Bare { size }).owner(owner).margin(Margin::ZERO).fill(c.get(colours::DIALOG_INNER)).at(r.left_bottom(), r.top()).on_top(top);
    let close = show(ctx, spec, |ui| {
        let (up, down, enter) = ui.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::ArrowUp), i.consume_key(Modifiers::NONE, Key::ArrowDown), i.consume_key(Modifiers::NONE, Key::Enter)));
        esc |= ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if up || down {
            hot = step(rows, hot, down);
        }
        if enter {
            if let Some(k) = hot.filter(|&k| rows[k].2 && !rows[k].0.is_empty()) {
                pick = Some(k);
            }
        }
        let mut list = |ui: &mut Ui| {
            let (lr, _) = ui.allocate_exact_size(full, Sense::hover());
            let (clicked, hovered) = bars::popup_rows(ui, lr, vid.0, rows, c, hot);
            if hovered.is_some() {
                hot = hovered;
            }
            if clicked.is_some() {
                pick = clicked;
            }
        };
        if full.y > size.y {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| list(ui));
        } else {
            list(ui);
        }
    });
    if pick.is_some() || close || esc {
        dd.close();
        return pick;
    }
    let lost = if embedded(ctx) {
        let list = Rect::from_min_size(r.left_bottom(), size);
        ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|m| !list.contains(m) && !r.contains(m)))
    } else {
        SLOTS.with(|s| s.borrow().get(&vid).is_some_and(|v| v.focused_once && v.focused == Some(false)))
    };
    if lost {
        dd.close();
        dd.closed_at = Some(Instant::now());
        return None;
    }
    dd.hot = hot;
    None
}

/// The next row that can be picked going down or up from `hot`.
fn step(rows: &[Row], hot: Option<usize>, down: bool) -> Option<usize> {
    let ok = |k: &usize| !rows[*k].0.is_empty() && rows[*k].2;
    let next = if down { (hot.map_or(0, |h| h + 1)..rows.len()).find(ok) } else { (0..hot.unwrap_or(rows.len())).rev().find(ok) };
    next.or(hot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_stay_on_the_screen() {
        let work = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 700.0));
        assert_eq!(clamp_pos(pos2(900.0, 650.0), vec2(300.0, 200.0), work), pos2(700.0, 500.0));
        assert_eq!(clamp_pos(pos2(-40.0, -10.0), vec2(300.0, 200.0), work), pos2(0.0, 0.0));
        // Too big for the screen: its top left stays on it.
        assert_eq!(clamp_pos(pos2(50.0, 50.0), vec2(1200.0, 900.0), work), pos2(0.0, 0.0));
    }

    #[test]
    fn dialogs_open_12_px_inside_the_screen() {
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 700.0));
        let size = vec2(300.0, 200.0);
        // Centred on a main window in the screen's bottom right corner: 12 px in from both edges.
        let owner = Rect::from_min_max(pos2(700.0, 500.0), pos2(1000.0, 700.0));
        assert_eq!(clamp_pos(owner.center() - size / 2.0, size, open_area(Place::Centre, screen)), pos2(688.0, 488.0));
        // A drop-down list may still reach the edge.
        assert_eq!(open_area(Place::At { pos: Pos2::ZERO, above: 0.0 }, screen), screen);
    }

    #[test]
    fn arrow_keys_skip_separators_and_greyed_rows() {
        let row = |s: &str, on: bool| (s.to_string(), String::new(), on, false);
        let rows = vec![row("a", true), row("", true), row("b", false), row("c", true)];
        assert_eq!(step(&rows, None, true), Some(0));
        assert_eq!(step(&rows, Some(0), true), Some(3));
        assert_eq!(step(&rows, Some(3), true), Some(3));
        assert_eq!(step(&rows, Some(3), false), Some(0));
        assert_eq!(step(&rows, None, false), Some(3));
    }
}
