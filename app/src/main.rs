#![windows_subsystem = "windows"]
//! Pedalboard Host: a live effects host for VST and VST3 plugins with ASIO and Windows audio.
//!
//! Options:
//!   <file.pdl>               open a patch file at startup
//!   --screenshot <file.png>  draw about 120 frames, save the window's picture and exit; with a
//!                            patch file too, the picture shows that patch (not remembered as last file)
//!   --scan-plugin <file>     (internal) describe one plugin file as JSON, used by the scanner

mod app;
mod bars;
mod builtins;
mod colours;
mod dialogs;
mod editors;
mod engine;
mod field;
mod graph;
mod midi;
mod osc;
mod pdl;
mod plugin_list;
mod plugins;
mod settings;
mod tempo;
mod tray;
mod watchdog;
mod widgets;

use egui::{FontData, FontDefinitions, FontFamily, FontTweak, IconData, ViewportBuilder};
use std::path::PathBuf;
use std::sync::Arc;


/// Verdana, the font JUCE uses on Windows. JUCE sizes a font by its ascent + descent and egui
/// by its em square; for Verdana that is 2489 against 2048 font units.
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts");
    // egui lifts scaled glyphs by (1 - scale) / 2 of the ascent-plus-descent span, which is
    // (2059 - 430) / 2048 of the size; shifting back by the same amount keeps JUCE's baseline.
    let scale = 2048.0 / 2489.0;
    let tweak = FontTweak { scale, y_offset_factor: (1.0 - scale) * 0.5 * (2059.0 - 430.0) / 2048.0, ..Default::default() };
    if let Ok(b) = std::fs::read(dir.join("verdana.ttf")) {
        fonts.font_data.insert("verdana".into(), Arc::new(FontData::from_owned(b).tweak(tweak)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "verdana".into());
    }
    let mut bold = Vec::new();
    if let Ok(b) = std::fs::read(dir.join("verdanab.ttf")) {
        fonts.font_data.insert("verdana-bold".into(), Arc::new(FontData::from_owned(b).tweak(tweak)));
        bold.push("verdana-bold".to_string());
    }
    bold.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);
}

/// A plain drawn icon: a pedal in the box colour with a knob and a footswitch.
fn icon() -> IconData {
    let n = 32usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (dx, dy) = ((fx - 16.0).abs() - 7.0, (fy - 16.0).abs() - 10.0);
            let body = dx.max(0.0).hypot(dy.max(0.0)) <= 5.0;
            let knob = (fx - 16.0).hypot(fy - 11.0) < 4.5;
            let switch = (fx - 16.0).hypot(fy - 23.0) < 3.5;
            let c = if knob || switch {
                [0x30, 0x30, 0x48, 0xFF]
            } else if body {
                [0xB0, 0xB0, 0xFF, 0xFF]
            } else {
                [0, 0, 0, 0]
            };
            rgba[(y * n + x) * 4..][..4].copy_from_slice(&c);
        }
    }
    IconData { rgba, width: n as u32, height: n as u32 }
}

/// Like the original (0x140277130), asks Windows for 1 ms timer precision while the app runs.
/// Plugin windows animate on timers and sleeps inside this process, and since Windows 10 (2004)
/// a process only gets the precision it asks for itself, otherwise 15.6 ms steps.
fn fast_timers(on: bool) {
    use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
    };
    unsafe {
        if !on {
            timeEndPeriod(1);
            return;
        }
        timeBeginPeriod(1);
        // Windows 11 slows down apps whose windows are covered or minimised (efficiency cores, coarse
        // timers); a live audio host has to keep its pace, so it opts out of both.
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
            StateMask: 0,
        };
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const PROCESS_POWER_THROTTLING_STATE as *const std::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--scan-plugin") {
        if let Some(p) = args.get(i + 1) {
            plugins::scan_child_main(p);
        }
        return;
    }
    let shot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let file = args.iter().find(|a| a.to_ascii_lowercase().ends_with(".pdl") && !a.starts_with("--")).map(PathBuf::from);

    watchdog::install_crash_notes();
    fast_timers(true);

    let mut vp = ViewportBuilder::default().with_title(app::TITLE).with_inner_size([1010.0, 573.0]).with_min_inner_size([640.0, 400.0]).with_icon(Arc::new(icon()));
    if shot.is_some() {
        // A check run must never take focus or cover what the person is doing.
        vp = vp.with_active(false);
        #[cfg(windows)]
        {
            vp = vp.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    // Plugin windows share this window's thread: waiting for the screen's refresh after each
    // picture would hold them up (Windows composes the window anyway, so nothing tears).
    let options = eframe::NativeOptions { viewport: vp, vsync: false, ..Default::default() };
    let r = eframe::run_native(
        app::TITLE,
        options,
        Box::new(move |cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc, shot, file)))
        }),
    );
    fast_timers(false);
    if let Err(e) = r {
        eprintln!("{e}");
    }
}
