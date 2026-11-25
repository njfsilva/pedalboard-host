//! The notification-area icon (Misc Settings: "Show an icon in the notification area"). It
//! runs on its own thread with a hidden window: a left click shows or hides the main window,
//! a right click offers Show and Exit. With the icon on, minimising hides the main window.

use crate::app::{App, TITLE};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const ID_SHOW: i32 = 1;
const ID_EXIT: i32 = 2;
static MAIN: AtomicIsize = AtomicIsize::new(0);

pub struct Tray {
    hwnd: Arc<AtomicIsize>,
    thread: Option<JoinHandle<()>>,
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
        SetForegroundWindow(h);
    }
}

fn hide_main(main: isize) {
    unsafe {
        ShowWindow(main as HWND, SW_HIDE);
    }
}

fn main_hidden(main: isize) -> bool {
    unsafe { IsWindowVisible(main as HWND) == 0 }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let main = MAIN.load(Ordering::Relaxed);
    match msg {
        WM_TRAY => {
            match (lp as u32) & 0xFFFF {
                WM_LBUTTONUP => {
                    if main_hidden(main) || IsIconic(main as HWND) != 0 {
                        show_main(main);
                    } else {
                        hide_main(main);
                    }
                }
                WM_RBUTTONUP => {
                    let menu = CreatePopupMenu();
                    let (show, exit) = (wide("Show window"), wide("Exit"));
                    AppendMenuW(menu, MF_STRING, ID_SHOW as usize, show.as_ptr());
                    AppendMenuW(menu, MF_STRING, ID_EXIT as usize, exit.as_ptr());
                    let mut pt = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut pt);
                    SetForegroundWindow(h);
                    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, null());
                    DestroyMenu(menu);
                    PostMessageW(h, WM_NULL, 0, 0);
                    if cmd == ID_SHOW {
                        show_main(main);
                    } else if cmd == ID_EXIT {
                        show_main(main);
                        PostMessageW(main as HWND, WM_CLOSE, 0, 0);
                    }
                }
                _ => {}
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
        MAIN.store(main, Ordering::Relaxed);
        let hwnd = Arc::new(AtomicIsize::new(0));
        let slot = hwnd.clone();
        let (ready, wait) = mpsc::channel();
        let thread = std::thread::spawn(move || unsafe {
            let inst = GetModuleHandleW(null());
            let class = wide("PedalboardHostTray");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = inst;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let h = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, null_mut(), null_mut(), inst, null());
            if h.is_null() {
                let _ = ready.send(false);
                return;
            }
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = h;
            nid.uID = 1;
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY;
            // The main window's own icon, or the standard one.
            let mut icon = SendMessageW(main as HWND, WM_GETICON, ICON_SMALL as usize, 0) as HICON;
            if icon.is_null() {
                icon = LoadIconW(null_mut(), IDI_APPLICATION);
            }
            nid.hIcon = icon;
            for (d, s) in nid.szTip.iter_mut().zip(TITLE.encode_utf16().take(127)) {
                *d = s;
            }
            let ok = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
            slot.store(h as isize, Ordering::Relaxed);
            let _ = ready.send(ok);
            let mut m: MSG = std::mem::zeroed();
            while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
            Shell_NotifyIconW(NIM_DELETE, &nid);
        });
        let tray = Tray { hwnd, thread: Some(thread) };
        wait.recv().unwrap_or(false).then_some(tray)
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let h = self.hwnd.load(Ordering::Relaxed);
        if h != 0 {
            unsafe {
                PostMessageW(h as HWND, WM_CLOSE, 0, 0);
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
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

    /// Called every frame: minimising hides the window to the icon, and "start hidden" hides it
    /// once the window has first appeared.
    pub fn tray_frame(&mut self, ctx: &egui::Context) {
        if self.tray.is_none() {
            return;
        }
        if self.tray_hide > 0 {
            self.tray_hide -= 1;
            if self.tray_hide == 0 {
                hide_main(self.hwnd);
            }
            ctx.request_repaint();
        } else if ctx.input(|i| i.viewport().minimized == Some(true)) && !main_hidden(self.hwnd) {
            hide_main(self.hwnd);
        }
    }
}
