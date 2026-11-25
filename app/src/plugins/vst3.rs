//! VST3 hosting: module loading, class scanning, and one plugin instance split into a processor
//! (audio thread) and a UI handle (parameters, programs, state, editor window).

mod com;
mod host;
mod processor;
mod ui;

use self::com::*;
use self::host::*;
use super::{c_text, Loaded, PluginInfo};
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::zeroed;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};

const AUDIO_CLASS: &str = "Audio Module Class";
/// MIDI controllers looked up through `IMidiMapping`: 128 CCs, channel pressure, pitch bend.
const MIDI_CTRLS: usize = 130;

/// The binary inside a bundle (`X.vst3/Contents/x86_64-win/X.vst3`) or the file itself.
fn binary_path(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_path_buf();
    }
    let dir = path.join("Contents").join("x86_64-win");
    let inner = dir.join(path.file_name().unwrap_or_default());
    if inner.is_file() {
        return inner;
    }
    std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vst3")))
        .unwrap_or(inner)
}

/// Loads a module once and keeps it for the life of the program (modules are never unloaded,
/// as on the VST2 side), returning a new reference to its factory.
fn factory(path: &Path) -> Result<Com<FactoryVtbl>, String> {
    static LOADED: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
    let bin = binary_path(path);
    let mut loaded = LOADED.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&p) = loaded.get(&bin) {
        return unsafe { Com::from_borrowed(p as *mut c_void) }.ok_or_else(|| "The plugin has no factory.".to_string());
    }
    if !bin.is_file() {
        return Err(format!("Could not locate file: {}", bin.display()));
    }
    unsafe {
        use libloading::os::windows::{Library, LOAD_WITH_ALTERED_SEARCH_PATH};
        let lib = Library::load_with_flags(&bin, LOAD_WITH_ALTERED_SEARCH_PATH).map_err(|e| format!("Could not load the plugin: {e}"))?;
        if let Ok(init) = lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") {
            if !init() {
                return Err("The plugin failed to initialise.".into());
            }
        }
        let get = *lib
            .get::<unsafe extern "system" fn() -> *mut c_void>(b"GetPluginFactory\0")
            .map_err(|_| "This file is not a VST3 plugin.".to_string())?;
        let f: Com<FactoryVtbl> = Com::from_raw(get()).ok_or("The plugin has no factory.")?;
        if let Some(f3) = f.query::<Factory3Vtbl>(&IID_PLUGIN_FACTORY3) {
            (f3.vt().set_host_context)(f3.ptr(), host_context());
        }
        std::mem::forget(lib);
        add_ref(f.ptr()); // the table's own reference
        loaded.insert(bin, f.ptr() as usize);
        Ok(f)
    }
}

#[derive(Default)]
struct ClassEntry {
    cid: Tuid,
    name: String,
    category: String,
    sub: String,
    vendor: String,
    version: String,
}

unsafe fn classes(f: &Com<FactoryVtbl>) -> Vec<ClassEntry> {
    let mut fi: FactoryInfo = zeroed();
    let vendor = if (f.vt().get_factory_info)(f.ptr(), &mut fi) == OK { c_text(&fi.vendor) } else { String::new() };
    let f2 = f.query::<Factory2Vtbl>(&IID_PLUGIN_FACTORY2);
    let f3 = f.query::<Factory3Vtbl>(&IID_PLUGIN_FACTORY3);
    let mut out = Vec::new();
    for i in 0..(f.vt().count_classes)(f.ptr()).clamp(0, 1000) {
        let mut w: ClassInfoW = zeroed();
        let mut c2: ClassInfo2 = zeroed();
        let mut c1: ClassInfo = zeroed();
        let mut e = if f3.as_ref().is_some_and(|f3| (f3.vt().get_class_info_unicode)(f3.ptr(), i, &mut w) == OK) {
            ClassEntry {
                cid: w.cid,
                name: wstr(&w.name),
                category: c_text(&w.category),
                sub: c_text(&w.sub_categories),
                vendor: wstr(&w.vendor),
                version: wstr(&w.version),
            }
        } else if f2.as_ref().is_some_and(|f2| (f2.vt().get_class_info2)(f2.ptr(), i, &mut c2) == OK) {
            ClassEntry {
                cid: c2.cid,
                name: c_text(&c2.name),
                category: c_text(&c2.category),
                sub: c_text(&c2.sub_categories),
                vendor: c_text(&c2.vendor),
                version: c_text(&c2.version),
            }
        } else if (f.vt().get_class_info)(f.ptr(), i, &mut c1) == OK {
            ClassEntry { cid: c1.cid, name: c_text(&c1.name), category: c_text(&c1.category), ..Default::default() }
        } else {
            continue;
        };
        if e.vendor.is_empty() {
            e.vendor = vendor.clone();
        }
        out.push(e);
    }
    out
}

/// One created plugin. It is torn down when both the processor and the UI handle are gone,
/// which always happens on the UI thread (the engine returns retired processors there).
struct Instance {
    component: Com<ComponentVtbl>,
    processor: Com<AudioProcessorVtbl>,
    controller: Option<Com<EditControllerVtbl>>,
    /// The controller is its own object rather than the component itself.
    separate: bool,
    links: Option<(Com<ConnectionPointVtbl>, Com<ConnectionPointVtbl>)>,
    handler: *mut c_void,
    active: AtomicBool,
}

unsafe impl Send for Instance {}
unsafe impl Sync for Instance {}

impl Instance {
    unsafe fn create(f: &Com<FactoryVtbl>, cid: &Tuid) -> Result<Instance, String> {
        let mut raw = null_mut();
        (f.vt().create_instance)(f.ptr(), cid, &IID_COMPONENT, &mut raw);
        let component: Com<ComponentVtbl> = Com::from_raw(raw).ok_or("The plugin could not be created.")?;
        if (component.vt().initialize)(component.ptr(), host_context()) != OK {
            return Err("The plugin failed to initialise.".into());
        }
        let Some(processor) = component.query::<AudioProcessorVtbl>(&IID_AUDIO_PROCESSOR) else {
            (component.vt().terminate)(component.ptr());
            return Err("The plugin has no audio processor.".into());
        };
        Ok(Instance { component, processor, controller: None, separate: false, links: None, handler: null_mut(), active: AtomicBool::new(false) })
    }

    /// Finds or creates the edit controller, connects the two halves and gives the controller
    /// the processor's current state.
    unsafe fn attach_controller(&mut self, f: &Com<FactoryVtbl>) {
        let c = &self.component;
        if let Some(ctrl) = c.query::<EditControllerVtbl>(&IID_EDIT_CONTROLLER) {
            self.controller = Some(ctrl);
        } else {
            let mut ccid: Tuid = [0; 16];
            let mut raw = null_mut();
            if (c.vt().get_controller_class_id)(c.ptr(), &mut ccid) == OK
                && ccid != [0; 16]
                && (f.vt().create_instance)(f.ptr(), &ccid, &IID_EDIT_CONTROLLER, &mut raw) == OK
            {
                if let Some(ctrl) = Com::<EditControllerVtbl>::from_raw(raw) {
                    if (ctrl.vt().initialize)(ctrl.ptr(), host_context()) == OK {
                        self.separate = true;
                        self.controller = Some(ctrl);
                    }
                }
            }
        }
        let Some(ctrl) = &self.controller else { return };
        if self.separate {
            let a = c.query::<ConnectionPointVtbl>(&IID_CONNECTION_POINT);
            let b = ctrl.query::<ConnectionPointVtbl>(&IID_CONNECTION_POINT);
            if let (Some(a), Some(b)) = (a, b) {
                (a.vt().connect)(a.ptr(), b.ptr());
                (b.vt().connect)(b.ptr(), a.ptr());
                self.links = Some((a, b));
            }
        }
        let s = MemStream::new(Vec::new());
        if (c.vt().get_state)(c.ptr(), s.ptr()) == OK {
            s.rewind();
            (ctrl.vt().set_component_state)(ctrl.ptr(), s.ptr());
        }
    }

    /// setupProcessing → setActive(true) → setProcessing(true), stopping first if running.
    unsafe fn activate(&self, sample_rate: f64, block: usize) {
        self.deactivate();
        let mut setup = ProcessSetup { process_mode: MODE_REALTIME, sample_size: SAMPLE_32, max_samples: block as i32, sample_rate };
        (self.processor.vt().setup_processing)(self.processor.ptr(), &mut setup);
        (self.component.vt().set_active)(self.component.ptr(), 1);
        (self.processor.vt().set_processing)(self.processor.ptr(), 1);
        self.active.store(true, Relaxed);
    }

    unsafe fn deactivate(&self) {
        if self.active.swap(false, Relaxed) {
            (self.processor.vt().set_processing)(self.processor.ptr(), 0);
            (self.component.vt().set_active)(self.component.ptr(), 0);
        }
    }

    /// Turns on the first audio bus each way and the first event input, turns other audio
    /// buses off, and returns every audio bus's channel count (inputs, outputs).
    unsafe fn setup_buses(&self) -> (Vec<i32>, Vec<i32>) {
        let c = &self.component;
        let mut dirs = [Vec::new(), Vec::new()];
        for (dir, chans) in [DIR_IN, DIR_OUT].into_iter().zip(dirs.iter_mut()) {
            for b in 0..(c.vt().get_bus_count)(c.ptr(), MEDIA_AUDIO, dir).clamp(0, 16) {
                let mut info: BusInfo = zeroed();
                let n = if (c.vt().get_bus_info)(c.ptr(), MEDIA_AUDIO, dir, b, &mut info) == OK { info.channel_count.clamp(0, 64) } else { 0 };
                (c.vt().activate_bus)(c.ptr(), MEDIA_AUDIO, dir, b, (b == 0) as u8);
                chans.push(n);
            }
        }
        if (c.vt().get_bus_count)(c.ptr(), MEDIA_EVENT, DIR_IN) > 0 {
            (c.vt().activate_bus)(c.ptr(), MEDIA_EVENT, DIR_IN, 0, 1);
        }
        let [ins, outs] = dirs;
        (ins, outs)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            self.deactivate();
            if let Some((a, b)) = self.links.take() {
                (a.vt().disconnect)(a.ptr(), b.ptr());
                (b.vt().disconnect)(b.ptr(), a.ptr());
            }
            if let Some(ctrl) = self.controller.take() {
                (ctrl.vt().set_component_handler)(ctrl.ptr(), null_mut());
                if self.separate {
                    (ctrl.vt().terminate)(ctrl.ptr());
                }
            }
            (self.component.vt().terminate)(self.component.ptr());
            if !self.handler.is_null() {
                release(self.handler);
            }
        }
    }
}

#[derive(Clone)]
struct Param {
    id: u32,
    title: String,
    flags: i32,
    steps: i32,
}

unsafe fn read_params(inst: &Instance) -> Vec<Param> {
    let mut out = Vec::new();
    if let Some(c) = &inst.controller {
        for i in 0..(c.vt().get_parameter_count)(c.ptr()).clamp(0, 65536) {
            let mut p: ParameterInfo = zeroed();
            if (c.vt().get_parameter_info)(c.ptr(), i, &mut p) == OK {
                out.push(Param { id: p.id, title: wstr(&p.title), flags: p.flags, steps: p.step_count });
            }
        }
    }
    out
}

/// Channel × controller → parameter id (`u32::MAX` where the plugin maps nothing).
unsafe fn read_midi_map(inst: &Instance) -> Vec<u32> {
    let mut map = vec![u32::MAX; 16 * MIDI_CTRLS];
    if let Some(mm) = inst.controller.as_ref().and_then(|c| c.query::<MidiMappingVtbl>(&IID_MIDI_MAPPING)) {
        for ch in 0..16 {
            for cc in 0..MIDI_CTRLS {
                let mut id = 0u32;
                if (mm.vt().get_midi_controller_assignment)(mm.ptr(), 0, ch as i16, cc as i16, &mut id) == OK {
                    map[ch * MIDI_CTRLS + cc] = id;
                }
            }
        }
    }
    map
}

/// Lists the effects and instruments in a `.vst3` file or bundle (run in the scan child process).
pub fn scan(path: &Path) -> Result<Vec<PluginInfo>, String> {
    unsafe {
        use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let f = factory(path)?;
        let mut out = Vec::new();
        for c in classes(&f) {
            if c.category != AUDIO_CLASS {
                continue;
            }
            // Channel counts need a live instance; one that refuses is listed as stereo.
            let (ins, outs) = match Instance::create(&f, &c.cid) {
                Ok(inst) => {
                    let (i, o) = inst.setup_buses();
                    (i.first().copied().unwrap_or(0), o.first().copied().unwrap_or(0))
                }
                Err(_) => (2, 2),
            };
            out.push(PluginInfo {
                is_instrument: c.sub.contains("Instrument"),
                name: c.name,
                format: "VST3".into(),
                category: c.sub,
                manufacturer: c.vendor,
                version: c.version,
                file: path.to_string_lossy().into_owned(),
                uid: hex(&c.cid),
                num_inputs: ins as u32,
                num_outputs: outs as u32,
            });
        }
        if out.is_empty() {
            Err("No effects or instruments were found in this file.".into())
        } else {
            Ok(out)
        }
    }
}

pub fn load(info: &PluginInfo, sample_rate: f64, block: usize) -> Result<Loaded, String> {
    let f = factory(Path::new(&info.file))?;
    unsafe {
        let list = classes(&f);
        let audio: Vec<&ClassEntry> = list.iter().filter(|c| c.category == AUDIO_CLASS).collect();
        let want = parse_hex(&info.uid);
        let class = audio
            .iter()
            .find(|c| Some(c.cid) == want)
            .or(if audio.len() == 1 { audio.first() } else { None })
            .ok_or_else(|| format!("{} was not found in {}.", info.name, super::file_name(&info.file)))?;
        let mut inst = Instance::create(&f, &class.cid)?;
        if (inst.processor.vt().can_process_sample_size)(inst.processor.ptr(), SAMPLE_32) != OK {
            return Err("The plugin can't process 32-bit audio.".into());
        }
        inst.attach_controller(&f);
        let (in_ch, out_ch) = inst.setup_buses();
        let (to_audio, from_ui) = rtrb::RingBuffer::new(1024);
        let (to_ui, from_audio) = rtrb::RingBuffer::new(1024);
        let shared = Arc::new(Shared { to_audio: Mutex::new(to_audio), restart: AtomicI32::new(0) });
        inst.handler = new_handler(shared.clone());
        if let Some(c) = &inst.controller {
            (c.vt().set_component_handler)(c.ptr(), inst.handler);
        }
        let params = read_params(&inst);
        let midi_map = read_midi_map(&inst);
        let block = block.max(16);
        inst.activate(sample_rate, block);
        let latency = (inst.processor.vt().get_latency_samples)(inst.processor.ptr());
        let inst = Arc::new(inst);
        let ins = in_ch.first().copied().unwrap_or(0) as usize;
        let outs = out_ch.first().copied().unwrap_or(0) as usize;
        let name = if info.name.is_empty() { class.name.clone() } else { info.name.clone() };
        let ids = params.iter().map(|p| p.id).collect();
        let proc_ = processor::V3Proc::new(inst.clone(), in_ch, out_ch, ids, midi_map, from_ui, to_ui, block);
        let ui = ui::V3Ui::new(inst, name, params, shared, from_audio, latency);
        Ok(Loaded { ui: Box::new(ui), proc_: Box::new(proc_), ins, outs })
    }
}
