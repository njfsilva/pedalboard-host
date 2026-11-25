//! Direct ASIO host: drivers come from HKLM\SOFTWARE\ASIO and are created as COM objects.
//! The driver's bufferSwitch callback converts samples in place and runs the patch straight away,
//! with no extra buffering, so the latency is exactly the driver's buffer size.

use super::{AudioRuntime, DeviceInfo};
use crate::settings::AudioConfig;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

#[repr(C)]
struct IAsio {
    vtbl: *const IAsioVtbl,
}

type This = *mut IAsio;

#[repr(C)]
#[allow(dead_code)]
struct IAsioVtbl {
    query_interface: unsafe extern "system" fn(This, *const c_void, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(This) -> u32,
    release: unsafe extern "system" fn(This) -> u32,
    init: unsafe extern "system" fn(This, *mut c_void) -> i32,
    get_driver_name: unsafe extern "system" fn(This, *mut u8),
    get_driver_version: unsafe extern "system" fn(This) -> i32,
    get_error_message: unsafe extern "system" fn(This, *mut u8),
    start: unsafe extern "system" fn(This) -> i32,
    stop: unsafe extern "system" fn(This) -> i32,
    get_channels: unsafe extern "system" fn(This, *mut i32, *mut i32) -> i32,
    get_latencies: unsafe extern "system" fn(This, *mut i32, *mut i32) -> i32,
    get_buffer_size: unsafe extern "system" fn(This, *mut i32, *mut i32, *mut i32, *mut i32) -> i32,
    can_sample_rate: unsafe extern "system" fn(This, f64) -> i32,
    get_sample_rate: unsafe extern "system" fn(This, *mut f64) -> i32,
    set_sample_rate: unsafe extern "system" fn(This, f64) -> i32,
    get_clock_sources: unsafe extern "system" fn(This, *mut c_void, *mut i32) -> i32,
    set_clock_source: unsafe extern "system" fn(This, i32) -> i32,
    get_sample_position: unsafe extern "system" fn(This, *mut i64, *mut i64) -> i32,
    get_channel_info: unsafe extern "system" fn(This, *mut ChannelInfo) -> i32,
    create_buffers: unsafe extern "system" fn(This, *mut BufferInfo, i32, i32, *const Callbacks) -> i32,
    dispose_buffers: unsafe extern "system" fn(This) -> i32,
    control_panel: unsafe extern "system" fn(This) -> i32,
    future: unsafe extern "system" fn(This, i32, *mut c_void) -> i32,
    output_ready: unsafe extern "system" fn(This) -> i32,
}

#[repr(C)]
struct ChannelInfo {
    channel: i32,
    is_input: i32,
    is_active: i32,
    group: i32,
    sample_type: i32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BufferInfo {
    is_input: i32,
    channel: i32,
    buffers: [*mut c_void; 2],
}

#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy, Default)]
struct ClockSource {
    index: i32,
    associated_channel: i32,
    associated_group: i32,
    is_current: i32,
    name: [u8; 32],
}

/// ASE_NoClock: the driver has no clock for the rate it was asked to run at.
const ASE_NO_CLOCK: i32 = -995;

#[repr(C)]
struct Callbacks {
    buffer_switch: extern "C" fn(i32, i32),
    sample_rate_changed: extern "C" fn(f64),
    message: extern "C" fn(i32, i32, *mut c_void, *mut f64) -> i32,
    buffer_switch_time_info: extern "C" fn(*mut c_void, i32, i32) -> *mut c_void,
}

static CALLBACKS: Callbacks = Callbacks {
    buffer_switch,
    sample_rate_changed,
    message: asio_message,
    buffer_switch_time_info,
};

struct CbState {
    driver: This,
    bufs: Vec<BufferInfo>,
    in_types: Vec<i32>,
    out_types: Vec<i32>,
    block: usize,
    scratch_in: Vec<Vec<f32>>,
    scratch_out: Vec<Vec<f32>>,
    in_ptrs: Vec<*const f32>,
    out_ptrs: Vec<*mut f32>,
    runtime: Arc<Mutex<AudioRuntime>>,
    post_output: bool,
}

static STATE: AtomicPtr<CbState> = AtomicPtr::new(null_mut());
static RESET: AtomicBool = AtomicBool::new(false);

pub fn reset_requested() -> bool {
    RESET.swap(false, Ordering::Relaxed)
}

extern "C" fn buffer_switch(index: i32, _direct: i32) {
    let p = STATE.load(Ordering::Acquire);
    if p.is_null() {
        return;
    }
    let st = unsafe { &mut *p };
    let half = (index & 1) as usize;
    let n_in = st.in_types.len();
    let n = st.block;
    for i in 0..n_in {
        unsafe { read_samples(st.in_types[i], st.bufs[i].buffers[half] as *const u8, &mut st.scratch_in[i][..n]) };
    }
    match st.runtime.try_lock() {
        Ok(mut rt) => unsafe { rt.process(&st.in_ptrs, &st.out_ptrs, n) },
        Err(_) => st.scratch_out.iter_mut().for_each(|b| b.fill(0.0)),
    }
    for (k, ty) in st.out_types.iter().enumerate() {
        unsafe { write_samples(*ty, &st.scratch_out[k][..n], st.bufs[n_in + k].buffers[half] as *mut u8) };
    }
    if st.post_output {
        unsafe { ((*(*st.driver).vtbl).output_ready)(st.driver) };
    }
}

extern "C" fn buffer_switch_time_info(params: *mut c_void, index: i32, direct: i32) -> *mut c_void {
    buffer_switch(index, direct);
    params
}

/// The original's sampleRateDidChange callback (0x1401adc20) is an empty function: a driver that
/// changes rate also sends a reset request, and that is what restarts the device.
extern "C" fn sample_rate_changed(_rate: f64) {}

/// The original's asioMessage switch (0x1400a78b0, jump table 0x1400a7900).
extern "C" fn asio_message(selector: i32, value: i32, _msg: *mut c_void, _opt: *mut f64) -> i32 {
    const SELECTOR_SUPPORTED: i32 = 1;
    const ENGINE_VERSION: i32 = 2;
    const RESET_REQUEST: i32 = 3;
    const BUFFER_SIZE_CHANGE: i32 = 4;
    const RESYNC_REQUEST: i32 = 5;
    const LATENCIES_CHANGED: i32 = 6;
    const SUPPORTS_INPUT_MONITOR: i32 = 9;
    match selector {
        SELECTOR_SUPPORTED => matches!(value, ENGINE_VERSION | RESET_REQUEST | RESYNC_REQUEST | LATENCIES_CHANGED | SUPPORTS_INPUT_MONITOR) as i32,
        ENGINE_VERSION => 2,
        // The main window restarts the device half a second after the last of these (0x1400a7920).
        RESET_REQUEST | BUFFER_SIZE_CHANGE | RESYNC_REQUEST => {
            RESET.store(true, Ordering::Relaxed);
            1
        }
        LATENCIES_CHANGED => 1,
        _ => 0,
    }
}

unsafe fn read_samples(ty: i32, src: *const u8, dst: &mut [f32]) {
    let n = dst.len();
    let rd32 = |i: usize, be: bool| {
        let v = (src as *const i32).add(i).read_unaligned();
        if be { i32::from_be(v) } else { v }
    };
    match ty {
        19 => std::ptr::copy_nonoverlapping(src as *const f32, dst.as_mut_ptr(), n),
        3 => (0..n).for_each(|i| dst[i] = f32::from_bits(rd32(i, true) as u32)),
        18 | 2 => (0..n).for_each(|i| dst[i] = rd32(i, ty == 2) as f32 * (1.0 / 2147483648.0)),
        16 | 0 => (0..n).for_each(|i| {
            let v = (src as *const i16).add(i).read_unaligned();
            dst[i] = if ty == 0 { i16::from_be(v) } else { v } as f32 * (1.0 / 32768.0)
        }),
        17 | 1 => (0..n).for_each(|i| {
            let p = src.add(i * 3);
            let (a, b, c) = if ty == 1 { (*p.add(2), *p.add(1), *p) } else { (*p, *p.add(1), *p.add(2)) };
            let v = ((a as i32) | (b as i32) << 8 | (c as i32) << 16) << 8 >> 8;
            dst[i] = v as f32 * (1.0 / 8388608.0)
        }),
        20 | 4 => (0..n).for_each(|i| {
            let v = (src as *const u64).add(i).read_unaligned();
            dst[i] = f64::from_bits(if ty == 4 { u64::from_be(v) } else { v }) as f32
        }),
        24..=27 | 8..=11 => {
            let shift = [15, 17, 19, 23][(ty & 3) as usize];
            let scale = 1.0 / (1u32 << shift) as f32;
            (0..n).for_each(|i| dst[i] = rd32(i, ty < 16) as f32 * scale)
        }
        _ => dst.fill(0.0),
    }
}

unsafe fn write_samples(ty: i32, src: &[f32], dst: *mut u8) {
    let n = src.len();
    let wr32 = |i: usize, v: i32, be: bool| (dst as *mut i32).add(i).write_unaligned(if be { v.to_be() } else { v });
    match ty {
        19 => std::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut f32, n),
        3 => (0..n).for_each(|i| wr32(i, src[i].to_bits() as i32, true)),
        18 | 2 => (0..n).for_each(|i| wr32(i, (src[i].clamp(-1.0, 1.0) as f64 * 2147483647.0) as i32, ty == 2)),
        16 | 0 => (0..n).for_each(|i| {
            let v = (src[i].clamp(-1.0, 1.0) * 32767.0) as i16;
            (dst as *mut i16).add(i).write_unaligned(if ty == 0 { v.to_be() } else { v })
        }),
        17 | 1 => (0..n).for_each(|i| {
            let v = (src[i].clamp(-1.0, 1.0) * 8388607.0) as i32;
            let p = dst.add(i * 3);
            let b = [v as u8, (v >> 8) as u8, (v >> 16) as u8];
            let b = if ty == 1 { [b[2], b[1], b[0]] } else { b };
            std::ptr::copy_nonoverlapping(b.as_ptr(), p, 3);
        }),
        20 | 4 => (0..n).for_each(|i| {
            let v = (src[i] as f64).to_bits();
            (dst as *mut u64).add(i).write_unaligned(if ty == 4 { v.to_be() } else { v })
        }),
        24..=27 | 8..=11 => {
            let shift = [15, 17, 19, 23][(ty & 3) as usize];
            let scale = ((1u32 << shift) - 1) as f32;
            (0..n).for_each(|i| wr32(i, (src[i].clamp(-1.0, 1.0) * scale) as i32, ty < 16))
        }
        _ => {}
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// Installed ASIO drivers: (name, CLSID).
pub fn list_drivers() -> Vec<(String, String)> {
    use windows_sys::Win32::System::Registry::*;
    let mut out = Vec::new();
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide("SOFTWARE\\ASIO").as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return out;
        }
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(key, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) != 0 {
                break;
            }
            let name = String::from_utf16_lossy(&name[..len as usize]);
            let mut buf = [0u16; 128];
            let mut size = (buf.len() * 2) as u32;
            let sub = wide(&name);
            if RegGetValueW(key, sub.as_ptr(), wide("CLSID").as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut size) == 0 {
                let clsid = String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]);
                out.push((name, clsid));
            }
        }
        RegCloseKey(key);
    }
    out
}

pub struct AsioDevice {
    driver: This,
    state: Option<Box<CbState>>,
    running: bool,
}

impl AsioDevice {
    unsafe fn vt(&self) -> &IAsioVtbl {
        &*(*self.driver).vtbl
    }

    pub fn open(cfg: &AudioConfig, hwnd: isize, runtime: Arc<Mutex<AudioRuntime>>) -> Result<(AsioDevice, DeviceInfo), String> {
        use windows_sys::core::GUID;
        use windows_sys::Win32::System::Com::*;
        let drivers = list_drivers();
        let (name, clsid_text) = drivers
            .iter()
            .find(|(n, _)| *n == cfg.device)
            .or(drivers.first())
            .cloned()
            .ok_or("No ASIO drivers are installed.")?;
        unsafe {
            CoInitializeEx(null(), COINIT_APARTMENTTHREADED as _);
            let mut clsid = GUID { data1: 0, data2: 0, data3: 0, data4: [0; 8] };
            if CLSIDFromString(wide(&clsid_text).as_ptr(), &mut clsid) < 0 {
                return Err(format!("The ASIO driver \"{name}\" has a bad CLSID."));
            }
            let mut obj: *mut c_void = null_mut();
            // ASIO drivers use their CLSID as the interface id too.
            let hr = CoCreateInstance(&clsid, null_mut(), CLSCTX_INPROC_SERVER, &clsid, &mut obj);
            if hr < 0 || obj.is_null() {
                return Err(format!("Could not load the ASIO driver \"{name}\" (error {hr:#x})."));
            }
            let mut dev = AsioDevice { driver: obj as This, state: None, running: false };
            let vt = dev.vt();
            if (vt.init)(dev.driver, hwnd as *mut c_void) == 0 {
                let mut msg = [0u8; 128];
                (vt.get_error_message)(dev.driver, msg.as_mut_ptr());
                return Err(format!("The ASIO driver \"{name}\" failed to start: {}", cstr(&msg)));
            }
            let (mut ni, mut no) = (0i32, 0i32);
            (vt.get_channels)(dev.driver, &mut ni, &mut no);
            let (mut bmin, mut bmax, mut bpref, mut gran) = (0, 0, 0, 0);
            (vt.get_buffer_size)(dev.driver, &mut bmin, &mut bmax, &mut bpref, &mut gran);
            let rates: Vec<f64> = [44100.0, 48000.0, 88200.0, 96000.0, 176400.0, 192000.0]
                .into_iter()
                .filter(|&r| (vt.can_sample_rate)(dev.driver, r) == 0)
                .collect();
            // As the original's open (0x1400a4440): with several clock sources and none in use,
            // select the first and give the driver 20 ms.
            let mut clocks = [ClockSource::default(); 32];
            let mut n_clocks = clocks.len() as i32;
            if (vt.get_clock_sources)(dev.driver, clocks.as_mut_ptr() as *mut c_void, &mut n_clocks) != 0 {
                n_clocks = 0;
            }
            let clocks = &clocks[..n_clocks.clamp(0, 32) as usize];
            if clocks.len() > 1 && !clocks.iter().any(|c| c.is_current != 0) {
                (vt.set_clock_source)(dev.driver, clocks[0].index);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let mut sr = 0.0f64;
            (vt.get_sample_rate)(dev.driver, &mut sr);
            // Only ask for a rate the driver isn't already running at (0x1400a4440): some drivers
            // re-clock and send a reset request on every rate call, which would restart the device
            // again and again.
            if cfg.sample_rate > 0.0 && rates.contains(&cfg.sample_rate) && sr != cfg.sample_rate {
                if (vt.set_sample_rate)(dev.driver, cfg.sample_rate) == ASE_NO_CLOCK && !clocks.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    (vt.set_clock_source)(dev.driver, clocks[0].index);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    (vt.set_sample_rate)(dev.driver, cfg.sample_rate);
                }
                (vt.get_sample_rate)(dev.driver, &mut sr);
            }
            if sr <= 0.0 {
                sr = 44100.0;
            }
            let mut blocks: Vec<u32> = Vec::new();
            if gran == -1 {
                let mut b = bmin.max(1);
                while b <= bmax && blocks.len() < 32 {
                    blocks.push(b as u32);
                    b *= 2;
                }
            } else if gran > 0 {
                let mut b = bmin;
                while b <= bmax && blocks.len() < 64 {
                    blocks.push(b as u32);
                    b += gran;
                }
            }
            if !blocks.contains(&(bpref as u32)) {
                blocks.push(bpref as u32);
                blocks.sort();
            }
            let block = if cfg.buffer_size > 0 && blocks.contains(&cfg.buffer_size) { cfg.buffer_size } else { bpref as u32 } as usize;
            let chan_name = |input: bool, ch: i32| {
                let mut ci = ChannelInfo { channel: ch, is_input: input as i32, is_active: 0, group: 0, sample_type: 0, name: [0; 32] };
                (vt.get_channel_info)(dev.driver, &mut ci);
                (cstr(&ci.name), ci.sample_type)
            };
            let in_names: Vec<String> = (0..ni).map(|c| chan_name(true, c).0).collect();
            let out_names: Vec<String> = (0..no).map(|c| chan_name(false, c).0).collect();
            let pick = |want: &[u32], total: i32| -> Vec<u32> {
                let v: Vec<u32> = want.iter().copied().filter(|&c| (c as i32) < total).collect();
                if v.is_empty() && want.is_empty() { Vec::new() } else if v.is_empty() { (0..total.min(2) as u32).collect() } else { v }
            };
            let active_ins = pick(&cfg.input_channels, ni);
            let active_outs = pick(&cfg.output_channels, no);
            let mut bufs: Vec<BufferInfo> = active_ins
                .iter()
                .map(|&c| BufferInfo { is_input: 1, channel: c as i32, buffers: [null_mut(); 2] })
                .chain(active_outs.iter().map(|&c| BufferInfo { is_input: 0, channel: c as i32, buffers: [null_mut(); 2] }))
                .collect();
            if bufs.is_empty() {
                return Err("Choose at least one ASIO channel.".into());
            }
            let in_types: Vec<i32> = active_ins.iter().map(|&c| chan_name(true, c as i32).1).collect();
            let out_types: Vec<i32> = active_outs.iter().map(|&c| chan_name(false, c as i32).1).collect();
            let mut scratch_in: Vec<Vec<f32>> = vec![vec![0.0; block]; active_ins.len()];
            let mut scratch_out: Vec<Vec<f32>> = vec![vec![0.0; block]; active_outs.len()];
            let in_ptrs = scratch_in.iter_mut().map(|b| b.as_ptr()).collect();
            let out_ptrs = scratch_out.iter_mut().map(|b| b.as_mut_ptr()).collect();
            if (vt.create_buffers)(dev.driver, bufs.as_mut_ptr(), bufs.len() as i32, block as i32, &CALLBACKS) != 0 {
                return Err(format!("The ASIO driver \"{name}\" could not create {block}-sample buffers."));
            }
            let (mut il, mut ol) = (0, 0);
            (vt.get_latencies)(dev.driver, &mut il, &mut ol);
            let post_output = (vt.output_ready)(dev.driver) == 0;
            dev.state = Some(Box::new(CbState {
                driver: dev.driver,
                bufs,
                in_types,
                out_types,
                block,
                scratch_in,
                scratch_out,
                in_ptrs,
                out_ptrs,
                runtime,
                post_output,
            }));
            let info = DeviceInfo {
                name,
                backend: "ASIO".into(),
                sample_rate: sr,
                block,
                in_names,
                out_names,
                active_ins,
                active_outs,
                rates,
                blocks,
                in_latency: il.max(0) as u32,
                out_latency: ol.max(0) as u32,
            };
            Ok((dev, info))
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        let Some(st) = self.state.as_mut() else { return Err("ASIO buffers missing".into()) };
        RESET.store(false, Ordering::Relaxed);
        STATE.store(&mut **st as *mut CbState, Ordering::Release);
        let r = unsafe { (self.vt().start)(self.driver) };
        if r != 0 {
            STATE.store(null_mut(), Ordering::Release);
            return Err(format!("The ASIO driver refused to start (error {r})."));
        }
        self.running = true;
        Ok(())
    }

    pub fn control_panel(&self) {
        unsafe { (self.vt().control_panel)(self.driver) };
    }
}

impl Drop for AsioDevice {
    fn drop(&mut self) {
        unsafe {
            let vt = self.vt();
            if self.running {
                (vt.stop)(self.driver);
            }
            STATE.store(null_mut(), Ordering::Release);
            if self.state.is_some() {
                (vt.dispose_buffers)(self.driver);
            }
            (vt.release)(self.driver);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_messages_are_answered_like_the_original() {
        let msg = |sel: i32, val: i32| asio_message(sel, val, null_mut(), null_mut());
        RESET.store(false, Ordering::Relaxed);
        let supported: Vec<i32> = (1..=12).filter(|&v| msg(1, v) == 1).collect();
        assert_eq!(supported, vec![2, 3, 5, 6, 9]);
        assert_eq!(msg(2, 0), 2);
        for sel in [3, 4, 5] {
            assert_eq!(msg(sel, 0), 1);
            assert!(reset_requested(), "selector {sel} asks for a restart");
        }
        assert_eq!(msg(6, 0), 1);
        assert_eq!((msg(7, 0), msg(8, 0)), (0, 0));
        sample_rate_changed(44100.0);
        assert!(!reset_requested(), "a bare rate change notice restarts nothing");
    }
}
