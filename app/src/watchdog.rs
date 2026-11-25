//! Notes for finding out why the app crashed or froze on the person's machine. A panic or a
//! crash, and a main window that stops answering for 5 seconds (when Windows calls it "Not
//! Responding"), are written to `crash.log` / `hang.log` in the settings folder, each with a small
//! memory snapshot of that moment (`crash.dmp` / `hang.dmp`) showing what every thread was doing.

use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::ptr::null;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HANDLE, HMODULE, HWND, SYSTEMTIME};
use windows_sys::Win32::System::Diagnostics::Debug::{
    MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump, SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
    MINIDUMP_EXCEPTION_INFORMATION,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_NULL};

/// What the window thread is busy with; a hang note names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Stage {
    Idle,
    Frame,
    AudioDevice,
    PluginLoad,
    PluginEditor,
    PluginIdle,
    PluginWindowIdle,
}

impl Stage {
    fn text(v: u8) -> &'static str {
        match v {
            1 => "building the main window's frame",
            2 => "opening or restarting the audio device",
            3 => "loading a plugin",
            4 => "opening a plugin window",
            5 => "passing values to plugins",
            6 => "letting a VST2 plugin window redraw (its idle call)",
            _ => "drawing the main window or handling window messages (plugin windows draw and tick here)",
        }
    }
}

static STAGE: AtomicU8 = AtomicU8::new(0);

pub fn set_stage(s: Stage) {
    STAGE.store(s as u8, Relaxed);
}

/// Marks a stage until the returned guard is dropped.
pub fn enter(s: Stage) -> StageGuard {
    StageGuard(STAGE.swap(s as u8, Relaxed))
}

pub struct StageGuard(u8);

impl Drop for StageGuard {
    fn drop(&mut self) {
        STAGE.store(self.0, Relaxed);
    }
}

fn dir() -> &'static PathBuf {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(crate::settings::dir)
}

fn now_text() -> String {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Appends a dated line to a note; a note past 256 KB starts over.
fn note(file: &str, text: &str) {
    let d = dir();
    let _ = std::fs::create_dir_all(d);
    let path = d.join(file);
    let append = std::fs::metadata(&path).is_ok_and(|m| m.len() < 256 * 1024);
    let mut o = std::fs::OpenOptions::new();
    o.create(true);
    if append {
        o.append(true);
    } else {
        o.write(true).truncate(true);
    }
    if let Ok(mut f) = o.open(&path) {
        let _ = writeln!(f, "{} {}", now_text(), text);
    }
}

/// Writes a memory snapshot of this process: every thread's stack and the loaded modules.
fn snapshot(file: &str, exc: Option<*const EXCEPTION_POINTERS>) {
    let Ok(f) = std::fs::File::create(dir().join(file)) else { return };
    unsafe {
        let info = exc.map(|p| MINIDUMP_EXCEPTION_INFORMATION { ThreadId: GetCurrentThreadId(), ExceptionPointers: p as *mut _, ClientPointers: 0 });
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            f.as_raw_handle() as HANDLE,
            MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules,
            info.as_ref().map_or(null(), |i| i as *const _),
            null(),
            null(),
        );
    }
}

/// The file an address belongs to, and the offset into it.
fn module_of(addr: usize) -> String {
    unsafe {
        let mut m: HMODULE = std::ptr::null_mut();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        if GetModuleHandleExW(flags, addr as *const u16, &mut m) == 0 || m.is_null() {
            return format!("{addr:#x}");
        }
        let mut buf = [0u16; 520];
        let n = GetModuleFileNameW(m, buf.as_mut_ptr(), buf.len() as u32) as usize;
        format!("{}+{:#x}", String::from_utf16_lossy(&buf[..n]), addr - m as usize)
    }
}

unsafe extern "system" fn on_crash(p: *const EXCEPTION_POINTERS) -> i32 {
    static ONCE: AtomicBool = AtomicBool::new(false);
    if !p.is_null() && !ONCE.swap(true, Relaxed) {
        let rec = &*(*p).ExceptionRecord;
        let code = rec.ExceptionCode as u32;
        let at = module_of(rec.ExceptionAddress as usize);
        note("crash.log", &format!("crash: exception {code:#010x} at {at} on thread {} while {}", GetCurrentThreadId(), Stage::text(STAGE.load(Relaxed))));
        snapshot("crash.dmp", Some(p));
    }
    // Windows still handles the crash as usual afterwards.
    0
}

/// Records panics and crashes from here on.
pub fn install_crash_notes() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        note("crash.log", &format!("panic on thread {thread}: {info}"));
        prev(info);
    }));
    unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
}

/// Watches the main window from a thread of its own: when it stops answering for 5 seconds,
/// notes what it was doing and takes a snapshot; a second line follows if it recovers.
pub fn watch(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let _ = std::thread::Builder::new().name("hang watch".into()).spawn(move || {
        let hwnd = hwnd as HWND;
        let mut hung: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            unsafe {
                if IsWindow(hwnd) == 0 {
                    return;
                }
                let mut r = 0usize;
                // Fails at once for a window Windows already counts as hung, else after 5 s.
                let answered = SendMessageTimeoutW(hwnd, WM_NULL, 0, 0, SMTO_ABORTIFHUNG, 5000, &mut r) != 0;
                if answered {
                    if let Some(since) = hung.take() {
                        note("hang.log", &format!("the window answered again after about {:.0} s", since.elapsed().as_secs_f64() + 5.0));
                    }
                } else if hung.is_none() && IsWindow(hwnd) != 0 {
                    hung = Some(Instant::now());
                    note("hang.log", &format!("the window stopped responding for 5 s while {}", Stage::text(STAGE.load(Relaxed))));
                    snapshot("hang.dmp", None);
                }
            }
        }
    });
}
