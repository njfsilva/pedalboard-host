//! The notification-area icon (Misc Settings: "Display tray icon"), as the original's TrayIcon
//! (constructor 0x1400891a0). It runs on its own thread with a hidden window. A double-click
//! shows or hides the main window (mouseDoubleClick, 0x140089830) and a right click offers
//! Show or Hide, then Quit (mouseDown, 0x140089360). While the icon is on, the main window's
//! close button and File > Exit hide the window to it (closeButtonPressed, 0x14001a970).
//!
//! The original keeps working while its window is hidden: its timers and messages don't need
//! the window. Here a frame of the app is what handles MIDI program changes, mappings and the
//! plugins' chores, and Windows sends no paint to a hidden window, so the repaint a frame waits
//! for would never come. While the window is away the icon's thread posts the paints itself, at
//! the pace the window keeps while audio runs, and MIDI, OSC and the plugin windows' bars post
//! one at once.

use crate::app::{App, LIVE_FRAME, TITLE};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
/// The main window went to the tray: the icon's thread starts posting its frames.
const WM_AWAY: u32 = WM_APP + 2;
const AWAY_TIMER: usize = 1;
const REGISTER_TIMER: usize = 2;
const REGISTER_RETRY_MS: u32 = 2000;
const REGISTERING: u8 = 0;
const REGISTERED: u8 = 1;
const UNAVAILABLE: u8 = 2;
// Delivered to the main window, including while it is hidden or minimised.
const WM_TRAY_ACTION: u32 = WM_APP + 0x587;
/// Timer ticks after which a posted frame that never ran (a nested message loop can swallow
/// it) is posted again: about a second.
const AWAY_LOST_TICKS: u32 = 30;
const ID_SHOW: i32 = 1;
const ID_QUIT: i32 = 2;
const ID_HIDE: i32 = 3;
const ID_TOGGLE: i32 = 4;
static MAIN: AtomicIsize = AtomicIsize::new(0);
/// The icon's window while it exists.
static ICON: AtomicIsize = AtomicIsize::new(0);
/// The icon's Quit asked the main window to close: it quits instead of going to the tray.
static QUIT: AtomicBool = AtomicBool::new(false);
/// A frame posted to the hidden window hasn't run yet: a burst of MIDI posts only one.
static POSTED: AtomicBool = AtomicBool::new(false);
/// Timer ticks since that frame was posted.
static WAITED: AtomicU32 = AtomicU32::new(0);

pub struct Tray {
    hwnd: isize,
    main: isize,
    registration: Arc<AtomicU8>,
    last_reported: u8,
    thread: Option<JoinHandle<()>>,
}

// Lives on the notification window's thread until that window has been destroyed.
struct ShellIcon {
    data: NOTIFYICONDATAW,
    taskbar_created: u32,
    registration: Arc<AtomicU8>,
}

impl ShellIcon {
    unsafe fn register(&self) {
        // The taskbar can also announce its creation after a display change, when the
        // icon still exists. Modify in that case. Both operations use the same HWND/ID.
        let added = Shell_NotifyIconW(NIM_ADD, &self.data) != 0 || Shell_NotifyIconW(NIM_MODIFY, &self.data) != 0;
        let previous = self.registration.swap(if added { REGISTERED } else { UNAVAILABLE }, Ordering::AcqRel);
        let h = self.data.hWnd;
        if added {
            KillTimer(h, REGISTER_TIMER);
        } else {
            SetTimer(h, REGISTER_TIMER, REGISTER_RETRY_MS, None);
            let main = MAIN.load(Ordering::Relaxed);
            if previous == REGISTERED && main_hidden(main) {
                PostMessageW(main as HWND, WM_TRAY_ACTION, ID_SHOW as usize, h as isize);
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn show_main(main: isize) {
    let h = main as HWND;
    unsafe {
        ShowWindow(h, SW_SHOW);
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        crate::subwin::show_owned(main);
        SetForegroundWindow(h);
    }
}

pub fn hide_main(main: isize) {
    unsafe {
        ShowWindow(main as HWND, SW_HIDE);
    }
    crate::subwin::hide_owned(main);
    let icon = ICON.load(Ordering::SeqCst);
    if icon != 0 {
        unsafe {
            PostMessageW(icon as HWND, WM_AWAY, 0, 0);
        }
    }
}

pub fn main_hidden(main: isize) -> bool {
    unsafe { IsWindowVisible(main as HWND) == 0 }
}

/// Posts a paint to the hidden main window, which runs the app's next frame, unless one is
/// still waiting.
fn post_frame(main: isize) {
    if !POSTED.swap(true, Ordering::SeqCst) {
        WAITED.store(0, Ordering::SeqCst);
        unsafe {
            PostMessageW(main as HWND, WM_PAINT, 0, 0);
        }
    }
}

/// Runs the app's next frame at once while its window is in the tray, for what just came in
/// (MIDI, OSC, a plugin window's bar). Safe from any thread.
pub fn wake() {
    let main = MAIN.load(Ordering::SeqCst);
    if main != 0 && ICON.load(Ordering::SeqCst) != 0 && main_hidden(main) {
        post_frame(main);
    }
}

/// Called as each frame starts: whatever was posted has run.
pub fn frame_started() {
    POSTED.store(false, Ordering::SeqCst);
}

/// Whether the icon's Quit is what asked the main window to close (and forget it).
pub fn take_quit() -> bool {
    QUIT.swap(false, Ordering::Relaxed)
}

/// Hidden, or down on the taskbar: Show brings it up.
fn main_away(main: isize) -> bool {
    main_hidden(main) || unsafe { IsIconic(main as HWND) != 0 }
}

// Window visibility and the save-before-quit flow belong to the main window's thread.
unsafe extern "system" fn main_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, id: usize, _: usize) -> LRESULT {
    if msg == WM_TRAY_ACTION && lp != 0 && lp == ICON.load(Ordering::Acquire) {
        let main = h as isize;
        match wp as i32 {
            ID_SHOW => show_main(main),
            ID_HIDE => hide_main(main),
            ID_TOGGLE if main_away(main) => show_main(main),
            ID_TOGGLE => hide_main(main),
            ID_QUIT => {
                QUIT.store(true, Ordering::Relaxed);
                show_main(main);
                PostMessageW(h, WM_CLOSE, 0, 0);
            }
            _ => {}
        }
        return 0;
    }
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(h, Some(main_proc), id);
    }
    DefSubclassProc(h, msg, wp, lp)
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let main = MAIN.load(Ordering::Relaxed);
    // The worker installs its ShellIcon before registration and clears it during destruction.
    if let Some(icon) = (GetWindowLongPtrW(h, GWLP_USERDATA) as *const ShellIcon).as_ref() {
        if (msg != 0 && msg == icon.taskbar_created) || (msg == WM_TIMER && wp == REGISTER_TIMER) {
            icon.register();
            return 0;
        }
        if msg == WM_DESTROY {
            SetWindowLongPtrW(h, GWLP_USERDATA, 0);
            Shell_NotifyIconW(NIM_DELETE, &icon.data);
        }
    }
    match msg {
        WM_TRAY => {
            match (lp as u32) & 0xFFFF {
                WM_LBUTTONDBLCLK => {
                    PostMessageW(main as HWND, WM_TRAY_ACTION, ID_TOGGLE as usize, h as isize);
                }
                WM_RBUTTONUP => {
                    let away = main_away(main);
                    let menu = CreatePopupMenu();
                    let first = wide(&format!("{} {TITLE}", if away { "Show" } else { "Hide" }));
                    let quit = wide("Quit");
                    AppendMenuW(menu, MF_STRING, ID_SHOW as usize, first.as_ptr());
                    AppendMenuW(menu, MF_SEPARATOR, 0, null());
                    AppendMenuW(menu, MF_STRING, ID_QUIT as usize, quit.as_ptr());
                    let mut pt = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut pt);
                    SetForegroundWindow(h);
                    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, null());
                    DestroyMenu(menu);
                    PostMessageW(h, WM_NULL, 0, 0);
                    if cmd == ID_SHOW {
                        let action = if away { ID_SHOW } else { ID_HIDE };
                        PostMessageW(main as HWND, WM_TRAY_ACTION, action as usize, h as isize);
                    } else if cmd == ID_QUIT {
                        // The original's Quit runs the close button's code, which only hides a
                        // window that's showing; here Quit always quits (the rebuild's own).
                        PostMessageW(main as HWND, WM_TRAY_ACTION, ID_QUIT as usize, h as isize);
                    }
                }
                _ => {}
            }
            0
        }
        WM_AWAY => {
            SetTimer(h, AWAY_TIMER, LIVE_FRAME.as_millis() as u32, None);
            0
        }
        WM_TIMER if wp == AWAY_TIMER => {
            if main != 0 && main_hidden(main) {
                if POSTED.load(Ordering::SeqCst) && WAITED.fetch_add(1, Ordering::SeqCst) + 1 >= AWAY_LOST_TICKS {
                    POSTED.store(false, Ordering::SeqCst);
                }
                post_frame(main);
            } else {
                KillTimer(h, AWAY_TIMER);
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

impl Tray {
    pub fn start(main: isize) -> Option<Tray> {
        // Ask for the app icon on the thread that owns the window. A worker sending
        // WM_GETICON while this thread waits for its startup reply deadlocks both.
        let icon = unsafe {
            if SetWindowSubclass(main as HWND, Some(main_proc), WM_TRAY_ACTION as usize, 0) == 0 {
                return None;
            }
            let icon = SendMessageW(main as HWND, WM_GETICON, ICON_SMALL as usize, 0) as HICON;
            if icon.is_null() { LoadIconW(null_mut(), IDI_APPLICATION) as isize } else { icon as isize }
        };
        MAIN.store(main, Ordering::Relaxed);
        let registration = Arc::new(AtomicU8::new(REGISTERING));
        let worker_registration = registration.clone();
        let (ready, wait) = mpsc::channel();
        let thread = std::thread::spawn(move || unsafe {
            let inst = GetModuleHandleW(null());
            let class = wide("PedalboardHostTray");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = inst;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            // A hidden top-level window receives Explorer's TaskbarCreated broadcast.
            let h = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), inst, null());
            if h.is_null() {
                return;
            }
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = h;
            nid.uID = 1;
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY;
            nid.hIcon = icon as HICON;
            for (d, s) in nid.szTip.iter_mut().zip(TITLE.encode_utf16().take(127)) {
                *d = s;
            }
            let taskbar_created = RegisterWindowMessageW(windows_sys::core::w!("TaskbarCreated"));
            let shell = ShellIcon { data: nid, taskbar_created, registration: worker_registration };
            SetWindowLongPtrW(h, GWLP_USERDATA, &shell as *const ShellIcon as isize);
            ICON.store(h as isize, Ordering::SeqCst);
            let _ = ready.send(h as isize);
            shell.register();
            let mut m: MSG = std::mem::zeroed();
            while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
            ICON.store(0, Ordering::SeqCst);
            if IsWindow(h) != 0 {
                DestroyWindow(h);
            }
        });
        match wait.recv() {
            Ok(hwnd) => Some(Tray { hwnd, main, registration, last_reported: REGISTERING, thread: Some(thread) }),
            Err(_) => {
                unsafe { RemoveWindowSubclass(main as HWND, Some(main_proc), WM_TRAY_ACTION as usize) };
                MAIN.store(0, Ordering::Relaxed);
                let _ = thread.join();
                None
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            RemoveWindowSubclass(self.main as HWND, Some(main_proc), WM_TRAY_ACTION as usize);
            PostMessageW(self.hwnd as HWND, WM_CANCELMODE, 0, 0);
            PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        MAIN.store(0, Ordering::Relaxed);
        QUIT.store(false, Ordering::Relaxed);
    }
}

impl App {
    /// Adds or removes the icon to match the setting.
    pub fn update_tray(&mut self) {
        let want = self.settings.use_tray_icon && self.shot.is_none() && self.hwnd != 0;
        if want && self.tray.is_none() {
            self.tray = Tray::start(self.hwnd);
            if self.tray.is_none() {
                self.log("Could not add the notification-area icon.".into());
            }
        } else if !want && self.tray.take().is_some() && main_hidden(self.hwnd) {
            show_main(self.hwnd);
        }
    }

    /// Called every frame: "start in tray" hides the window once it has first appeared. As in the
    /// original, minimising only minimises.
    pub fn tray_frame(&mut self, ctx: &egui::Context) {
        let Some(tray) = self.tray.as_mut() else {
            self.tray_hide = 0;
            return;
        };
        let state = tray.registration.load(Ordering::Acquire);
        let previous = std::mem::replace(&mut tray.last_reported, state);
        if previous != state && state == UNAVAILABLE {
            self.log("Windows could not display the tray icon. Retrying automatically.".into());
        } else if previous == UNAVAILABLE && state == REGISTERED {
            self.log("Windows restored the tray icon.".into());
        }
        if self.tray_hide > 0 {
            match state {
                REGISTERING => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
                REGISTERED => {
                    self.tray_hide -= 1;
                    if self.tray_hide == 0 {
                        hide_main(self.hwnd);
                    }
                    ctx.request_repaint();
                }
                _ => self.tray_hide = 0,
            }
        }
    }

    /// What the original's closeButtonPressed (0x14001a970) does first: with the tray icon on and
    /// the window showing, it hides the window and nothing closes. Says whether it did.
    pub fn hide_to_tray(&mut self) -> bool {
        if !self.tray.as_ref().is_some_and(|tray| tray.registration.load(Ordering::Acquire) == REGISTERED) || main_hidden(self.hwnd) {
            return false;
        }
        hide_main(self.hwnd);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static PAINTS: AtomicU32 = AtomicU32::new(0);
    static TRAY_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    unsafe extern "system" fn count_paints(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_PAINT {
            PAINTS.fetch_add(1, Ordering::SeqCst);
        }
        DefWindowProcW(h, msg, wp, lp)
    }

    /// Handles what is waiting for the window; says how many paints (frames) ran.
    fn frames_run(h: HWND) -> u32 {
        let before = PAINTS.load(Ordering::SeqCst);
        unsafe {
            let mut m: MSG = std::mem::zeroed();
            while PeekMessageW(&mut m, h, 0, 0, PM_REMOVE) != 0 {
                DispatchMessageW(&m);
            }
        }
        PAINTS.load(Ordering::SeqCst) - before
    }

    #[test]
    fn a_window_in_the_tray_still_runs_frames_and_a_burst_runs_one() {
        let _guard = TRAY_TEST.lock().unwrap();
        unsafe {
            let inst = GetModuleHandleW(null());
            let class = wide("PedalboardHostTrayTest");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(count_paints);
            wc.hInstance = inst;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            // Never shown: the state the main window is in while it's in the tray.
            let h = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), WS_OVERLAPPEDWINDOW, 0, 0, 100, 100, null_mut(), null_mut(), inst, null());
            assert!(!h.is_null() && main_hidden(h as isize));
            MAIN.store(h as isize, Ordering::SeqCst);
            ICON.store(1, Ordering::SeqCst);
            frame_started();
            for _ in 0..50 {
                wake();
            }
            assert_eq!(frames_run(h), 1, "a burst of messages runs one frame");
            wake();
            assert_eq!(frames_run(h), 0, "nothing more is posted until that frame starts");
            frame_started();
            wake();
            assert_eq!(frames_run(h), 1);
            ICON.store(0, Ordering::SeqCst);
            MAIN.store(0, Ordering::SeqCst);
            frame_started();
            DestroyWindow(h);
        }
    }

    unsafe fn hidden_test_window() -> HWND {
        CreateWindowExW(
            WS_EX_TOOLWINDOW, windows_sys::core::w!("STATIC"), null(), WS_POPUP, 0, 0, 1, 1,
            null_mut(), null_mut(), GetModuleHandleW(null()), null(),
        )
    }

    #[test]
    fn tray_startup_and_shutdown_return_while_the_window_is_between_frames() {
        let _guard = TRAY_TEST.lock().unwrap();
        let (done, wait) = mpsc::channel();
        std::thread::spawn(move || unsafe {
            let h = hidden_test_window();
            assert!(!h.is_null());
            // No message pump here. The old worker's WM_GETICON used to hang at start.
            let tray = Tray::start(h as isize).unwrap();
            drop(tray);
            DestroyWindow(h);
            done.send(()).unwrap();
        });
        wait.recv_timeout(std::time::Duration::from_secs(10)).expect("tray setup and teardown must return");
    }

    // Run explicitly on a Windows desktop with Explorer. The host window stays hidden.
    #[test]
    #[ignore = "requires the Windows notification area"]
    fn explorer_registers_restores_and_removes_the_icon() {
        use std::time::{Duration, Instant};
        use windows_sys::Win32::UI::Shell::{Shell_NotifyIconGetRect, NOTIFYICONIDENTIFIER};

        let _guard = TRAY_TEST.lock().unwrap();
        unsafe {
            assert!(!GetShellWindow().is_null(), "this test requires a desktop with Windows Explorer");
            let h = hidden_test_window();
            assert!(!h.is_null());
            let tray = Tray::start(h as isize).expect("start without the main window pumping messages");
            let mut id: NOTIFYICONIDENTIFIER = std::mem::zeroed();
            id.cbSize = std::mem::size_of_val(&id) as u32;
            id.hWnd = tray.hwnd as HWND;
            id.uID = 1;
            let present = || Shell_NotifyIconGetRect(&id, &mut std::mem::zeroed()) >= 0;
            let wait_for = |want: bool| {
                let deadline = Instant::now() + Duration::from_secs(6);
                while present() != want && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(25));
                }
                assert_eq!(present(), want, "Explorer icon lookup");
            };
            wait_for(true);
            let mut data: NOTIFYICONDATAW = std::mem::zeroed();
            data.cbSize = std::mem::size_of_val(&data) as u32;
            data.hWnd = id.hWnd;
            data.uID = id.uID;
            assert_ne!(Shell_NotifyIconW(NIM_DELETE, &data), 0);
            wait_for(false);
            let restart = RegisterWindowMessageW(windows_sys::core::w!("TaskbarCreated"));
            assert_ne!(PostMessageW(id.hWnd, restart, 0, 0), 0);
            wait_for(true);
            drop(tray);
            wait_for(false);
            DestroyWindow(h);
        }
    }
}
