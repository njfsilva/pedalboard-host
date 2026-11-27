//! Listen on the host window and the key-capture prompt. Native key messages also distinguish
//! number-pad keys and reach the host before egui converts clipboard shortcuts into edit events.

use super::Mappings;
use crate::app::{Cmd, Incoming};
use egui::Context;
use std::cell::RefCell;
use std::sync::mpsc::Sender;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_KEYDOWN, WM_NCDESTROY, WM_SYSKEYDOWN};

const MAIN: usize = 0x50424b;
const CAPTURE: usize = MAIN + 1;

struct Router {
    ctx: Context,
    tx: Sender<Incoming>,
    bindings: Vec<(Cmd, String)>,
    enabled: bool,
}

thread_local! { static ROUTER: RefCell<Option<Router>> = const { RefCell::new(None) }; }

pub fn install(hwnd: isize, tx: Sender<Incoming>, ctx: &Context, maps: &Mappings) {
    ROUTER.with(|r| *r.borrow_mut() = Some(Router {
        ctx: ctx.clone(), tx, bindings: Vec::new(), enabled: true,
    }));
    update_keys(maps);
    unsafe { SetWindowSubclass(hwnd as HWND, Some(key_proc), MAIN, 0) };
}

pub fn update_keys(maps: &Mappings) {
    ROUTER.with_borrow_mut(|r| {
        if let Some(r) = r {
            r.bindings = maps.keys().map(|(cmd, key)| (cmd, key.to_owned())).collect();
        }
    });
}

pub fn enable(enabled: bool) {
    ROUTER.with_borrow_mut(|r| {
        if let Some(r) = r {
            r.enabled = enabled;
        }
    });
}

pub fn listen(hwnd: isize) {
    if hwnd != 0 {
        unsafe { SetWindowSubclass(hwnd as HWND, Some(key_proc), CAPTURE, 0) };
    }
}

unsafe extern "system" fn key_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, id: usize, _: usize) -> LRESULT {
    if matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let handled = ROUTER.with_borrow(|r| {
            let Some(r) = r else { return false };
            if id == MAIN && !r.enabled { return false; }
            let Some(key) = chord(wp as u16, lp) else { return false };
            let event = if id == CAPTURE {
                Incoming::CapturedKey(key)
            } else if let Some((cmd, _)) = r.bindings.iter().find(|(_, k)| *k == key) {
                Incoming::Shortcut(super::timed(*cmd, std::time::Instant::now()))
            } else {
                return false;
            };
            // One command for each press and release, including a pedal that emulates a key.
            if lp & (1 << 30) == 0 {
                let _ = r.tx.send(event);
                r.ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            true
        });
        if handled { return 0; }
    } else if msg == WM_NCDESTROY {
        RemoveWindowSubclass(h, Some(key_proc), id);
    }
    DefSubclassProc(h, msg, wp, lp)
}

fn chord(vk: u16, lp: LPARAM) -> Option<String> {
    let down = |key: u16| unsafe { GetKeyState(i32::from(key)) < 0 };
    // Leave Windows shortcuts to the shell.
    if down(VK_LWIN) || down(VK_RWIN) { return None; }
    let name = match vk {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_SHIFT | VK_LSHIFT | VK_RSHIFT |
        VK_MENU | VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN | VK_PROCESSKEY | VK_PACKET => return None,
        VK_RETURN => "return", VK_SPACE => "spacebar", VK_ESCAPE => "escape",
        VK_BACK => "backspace", VK_INSERT => "insert", VK_DELETE if lp & (1 << 24) == 0 => "numpad delete",
        VK_DELETE => "delete", VK_TAB => "tab", VK_HOME => "home", VK_END => "end",
        VK_PRIOR => "page up", VK_NEXT => "page down", VK_LEFT => "cursor left",
        VK_RIGHT => "cursor right", VK_UP => "cursor up", VK_DOWN => "cursor down",
        VK_ADD => "numpad +", VK_SUBTRACT => "numpad -", VK_MULTIPLY => "numpad *",
        VK_DIVIDE => "numpad /", VK_DECIMAL => "numpad .", VK_SEPARATOR => "numpad separator",
        VK_MEDIA_PLAY_PAUSE => "play", VK_MEDIA_STOP => "stop", VK_MEDIA_NEXT_TRACK => "fast forward",
        VK_MEDIA_PREV_TRACK => "rewind",
        _ => "",
    };
    let key = if !name.is_empty() {
        name.to_owned()
    } else if (VK_F1..=VK_F24).contains(&vk) {
        format!("F{}", vk - VK_F1 + 1)
    } else if (VK_NUMPAD0..=VK_NUMPAD9).contains(&vk) {
        format!("numpad {}", vk - VK_NUMPAD0)
    } else {
        let c = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_CHAR) } & 0x7fff_ffff;
        char::from_u32(c).filter(|c| !c.is_control()).map(|c| c.to_string())
            .unwrap_or_else(|| format!("#{:x}", 0x10000 | u32::from(vk)))
    };
    Some(description(down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU), &key))
}

fn description(ctrl: bool, shift: bool, alt: bool, key: &str) -> String {
    let mut s = String::new();
    if ctrl { s.push_str("ctrl + "); }
    if shift { s.push_str("shift + "); }
    if alt { s.push_str("alt + "); }
    s.push_str(key);
    s
}

pub fn normalize(text: &str) -> String {
    let mut key = text.trim();
    let (mut ctrl, mut shift, mut alt) = (false, false, false);
    while let Some((prefix, tail)) = key.split_once('+') {
        match prefix.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "command" | "cmd" => ctrl = true,
            "shift" => shift = true,
            "alt" | "option" => alt = true,
            _ => break,
        }
        key = tail.trim_start();
    }
    let key = if key.chars().count() == 1 || key.strip_prefix(['F','f']).is_some_and(|n| n.parse::<u8>().is_ok()) {
        key.to_uppercase()
    } else {
        key.to_ascii_lowercase()
    };
    description(ctrl, shift, alt, &key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ptr::null_mut, sync::mpsc};
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SendMessageW, WS_POPUP};

    #[test]
    fn native_key_capture_reassignment_and_focus_gating() {
        let (tx, rx) = mpsc::channel();
        let ctx = Context::default();
        let mut maps = Mappings::default();
        unsafe {
            let window = || CreateWindowExW(0, windows_sys::core::w!("STATIC"), std::ptr::null(), WS_POPUP,
                0, 0, 1, 1, null_mut(), null_mut(), null_mut(), std::ptr::null());
            let main = window();
            let prompt = window();
            assert!(!main.is_null() && !prompt.is_null());
            install(main as isize, tx, &ctx, &maps);
            listen(prompt as isize);
            let mut state = [0u8; 256];
            GetKeyboardState(state.as_mut_ptr());
            let saved = state;
            state.fill(0);
            state[VK_CONTROL as usize] = 0x80;
            state[VK_SHIFT as usize] = 0x80;
            SetKeyboardState(state.as_ptr());
            let keydown = |h, vk: u16, lp| SendMessageW(h, WM_KEYDOWN, usize::from(vk), lp);
            keydown(prompt, VK_SHIFT, 1);
            keydown(prompt, VK_F9, 1);
            keydown(prompt, VK_F9, 1 | (1 << 30));
            let Ok(Incoming::CapturedKey(key)) = rx.try_recv() else { panic!("Expected a key from the prompt") };
            assert_eq!(key, "ctrl + shift + F9");
            assert!(rx.try_recv().is_err());
            maps.assign(Cmd::NextPatch, super::super::Trigger::Key(key));
            update_keys(&maps);
            enable(false);
            keydown(main, VK_F9, 1);
            assert!(rx.try_recv().is_err());
            enable(true);
            keydown(main, VK_F9, 1);
            assert!(matches!(rx.try_recv(), Ok(Incoming::Shortcut(Cmd::NextPatch))));
            SetKeyboardState(saved.as_ptr());
            DestroyWindow(prompt);
            DestroyWindow(main);
        }
    }
}
