//! Minimal replacement for the `webbrowser` crate API used by egui-winit.

use std::io;

pub type Result<T> = io::Result<T>;

#[cfg(windows)]
pub fn open(url: &str) -> Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let verb = wide("open");
    let target = wide(url);
    // ShellExecuteW reports success with a value greater than 32.
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, format!("could not open {url} (code {code})")))
    }
}

#[cfg(target_os = "macos")]
pub fn open(url: &str) -> Result<()> {
    std::process::Command::new("open").arg(url).spawn().map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn open(url: &str) -> Result<()> {
    std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
}
