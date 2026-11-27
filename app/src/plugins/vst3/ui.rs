//! The UI-thread half of a VST3 instance: parameters, programs, state and the editor window.

use super::com::*;
use super::host::{new_frame, Frame, MemStream, Shared};
use super::{read_params, Instance, Param};
use crate::plugins::editor::{resize_for_plugin, EditorWindow};
use crate::plugins::PluginUi;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

/// Two length-prefixed state chunks, optionally followed by unprocessed (ParamID, value) edits.
const STATE_TAG: &[u8] = b"PBV3";

pub struct V3Ui {
    inst: Arc<Instance>,
    name: String,
    params: Vec<Param>,
    shared: Arc<Shared>,
    from_audio: rtrb::Consumer<(u32, f64)>,
    view: Option<Com<PlugViewVtbl>>,
    win: Option<EditorWindow>,
    frame: *mut Frame,
}

impl V3Ui {
    pub(super) fn new(inst: Arc<Instance>, name: String, params: Vec<Param>, shared: Arc<Shared>, from_audio: rtrb::Consumer<(u32, f64)>) -> V3Ui {
        V3Ui { inst, name, params, shared, from_audio, view: None, win: None, frame: new_frame() }
    }

    fn controller(&self) -> Option<&Com<EditControllerVtbl>> {
        self.inst.controller.as_ref()
    }

    fn program_param(&self) -> Option<&Param> {
        self.params.iter().find(|p| p.flags & PARAM_PROGRAM_CHANGE != 0 && p.steps > 0)
    }

    fn text_for(&self, id: u32, v: f64) -> String {
        let mut buf = [0u16; 128];
        match self.controller() {
            Some(c) if unsafe { (c.vt().get_param_string_by_value)(c.ptr(), id, v, buf.as_mut_ptr()) } == OK => wstr(&buf),
            _ => String::new(),
        }
    }

    /// Sets a value on the controller (and its window) and sends it on to the processor.
    fn set_normalized(&mut self, id: u32, v: f64) {
        if let Some(c) = self.controller() {
            unsafe { (c.vt().set_param_normalized)(c.ptr(), id, v) };
        }
        let _ = self.shared.to_audio.lock().unwrap_or_else(|e| e.into_inner()).push((id, v));
    }
}

fn stream_out(f: impl FnOnce(*mut c_void) -> TResult) -> Vec<u8> {
    let s = MemStream::new(Vec::new());
    if f(s.ptr()) == OK {
        s.take()
    } else {
        Vec::new()
    }
}

fn chunk(d: &[u8]) -> Option<(&[u8], &[u8])> {
    let n = u32::from_le_bytes(d.get(..4)?.try_into().ok()?) as usize;
    let body = d.get(4..4 + n)?;
    Some((body, &d[4 + n..]))
}

fn split_state(data: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let (a, rest) = chunk(data.strip_prefix(STATE_TAG)?)?;
    let (b, tail) = if rest.is_empty() { (&[][..], &[][..]) } else { chunk(rest)? };
    let edits = if tail.is_empty() { tail } else { tail.strip_prefix(b"EDIT")? };
    (edits.len() % 12 == 0).then_some((a, b, edits))
}

impl PluginUi for V3Ui {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn num_params(&self) -> i32 {
        self.params.len() as i32
    }

    fn param_name(&self, i: i32) -> String {
        self.params.get(i as usize).map(|p| p.title.clone()).unwrap_or_default()
    }

    fn param_value(&self, i: i32) -> f32 {
        match (self.params.get(i as usize), self.controller()) {
            (Some(p), Some(c)) => unsafe { (c.vt().get_param_normalized)(c.ptr(), p.id) as f32 },
            _ => 0.0,
        }
    }

    fn param_text(&self, i: i32) -> String {
        match self.params.get(i as usize) {
            Some(p) => self.text_for(p.id, self.param_value(i) as f64).trim().to_string(),
            None => String::new(),
        }
    }

    fn set_param(&mut self, i: i32, v: f32) {
        if let Some(id) = self.params.get(i as usize).map(|p| p.id) {
            self.set_normalized(id, v.clamp(0.0, 1.0) as f64);
        }
    }

    // VST3 programs are a stepped "program change" parameter.
    fn num_programs(&self) -> i32 {
        self.program_param().map_or(0, |p| p.steps + 1)
    }

    fn program(&self) -> i32 {
        match (self.program_param(), self.controller()) {
            (Some(p), Some(c)) => unsafe { ((c.vt().get_param_normalized)(c.ptr(), p.id) * p.steps as f64).round() as i32 },
            _ => 0,
        }
    }

    fn set_program(&mut self, i: i32) {
        if let Some((id, steps)) = self.program_param().map(|p| (p.id, p.steps)) {
            self.set_normalized(id, i.clamp(0, steps) as f64 / steps as f64);
        }
    }

    fn select_program_after_state(&mut self, i: i32) {
        // The component already has the saved sound. Sending another factory program
        // change on the next audio block would overwrite any edits to that sound.
        if let (Some(p), Some(c)) = (self.program_param(), self.controller()) {
            unsafe { (c.vt().set_param_normalized)(c.ptr(), p.id, i as f64 / p.steps as f64) };
        }
    }

    fn program_name(&self, i: i32) -> String {
        match self.program_param() {
            Some(p) => self.text_for(p.id, i.clamp(0, p.steps) as f64 / p.steps as f64),
            None => String::new(),
        }
    }

    fn get_state(&mut self) -> Vec<u8> {
        let mut pending = self.shared.processing.lock().unwrap_or_else(|e| e.into_inner());
        let comp = &self.inst.component;
        let a = stream_out(|s| unsafe { (comp.vt().get_state)(comp.ptr(), s) });
        let b = match self.controller() {
            Some(c) => stream_out(|s| unsafe { (c.vt().get_state)(c.ptr(), s) }),
            None => Vec::new(),
        };
        let mut out = Vec::with_capacity(12 + a.len() + b.len());
        out.extend_from_slice(STATE_TAG);
        out.extend_from_slice(&(a.len() as u32).to_le_bytes());
        out.extend_from_slice(&a);
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(&b);
        let count = pending.slots();
        if count > 0 {
            out.extend_from_slice(b"EDIT");
            let queued = pending.read_chunk(count).unwrap();
            let (first, second) = queued.as_slices();
            for (id, value) in first.iter().chain(second) {
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        out
    }

    fn set_state(&mut self, data: &[u8]) -> bool {
        let Some((a, b, edits)) = split_state(data) else { return false };
        let shared = Arc::clone(&self.shared);
        let mut pending = shared.processing.lock().unwrap_or_else(|e| e.into_inner());
        let previous_edits = pending.slots();
        unsafe {
            let comp = &self.inst.component;
            let s = MemStream::new(a.to_vec());
            let mut restored = (comp.vt().set_state)(comp.ptr(), s.ptr()) == OK;
            if let Some(c) = self.controller() {
                s.rewind();
                (c.vt().set_component_state)(c.ptr(), s.ptr());
                if !b.is_empty() {
                    let s2 = MemStream::new(b.to_vec());
                    let controller_restored = (c.vt().set_state)(c.ptr(), s2.ptr()) == OK;
                    restored |= a.is_empty() && controller_restored;
                }
            }
            if !restored {
                return false;
            }
            // Keep notifications emitted during restore, discarding only the prior sound's edits.
            pending.read_chunk(previous_edits).unwrap().commit_all();
            while self.from_audio.pop().is_ok() {}
            for edit in edits.chunks_exact(12) {
                let id = u32::from_le_bytes(edit[..4].try_into().unwrap());
                let value = f64::from_le_bytes(edit[4..].try_into().unwrap());
                self.set_normalized(id, value);
            }
            true
        }
    }

    fn has_editor(&self) -> bool {
        self.controller().is_some()
    }

    fn editor_window(&self) -> Option<&EditorWindow> {
        self.win.as_ref()
    }

    fn open_editor(&mut self, owner: isize, on_top: bool, fixed_size: bool) -> Result<(), String> {
        if let Some(w) = &self.win {
            w.show();
            return Ok(());
        }
        let c = self.inst.controller.as_ref().ok_or("This plugin has no editor.")?;
        unsafe {
            let view: Com<PlugViewVtbl> = Com::from_raw((c.vt().create_view)(c.ptr(), c"editor".as_ptr())).ok_or("This plugin has no editor.")?;
            if (view.vt().is_platform_type_supported)(view.ptr(), c"HWND".as_ptr()) != OK {
                return Err("This plugin's editor can't open on Windows.".into());
            }
            let mut r = ViewRect::default();
            (view.vt().get_size)(view.ptr(), &mut r);
            let (w, h) = (r.width().max(50), r.height().max(30));
            let resizable = !fixed_size && (view.vt().can_resize)(view.ptr()) == OK;
            let mut win = EditorWindow::new(&self.name, w, h, owner, on_top, resizable)?;
            let frame = &*self.frame;
            frame.hwnd.store(win.hwnd as isize, Relaxed);
            (view.vt().set_frame)(view.ptr(), self.frame as *mut c_void);
            // The plugin's window goes in the area under the presets bar.
            if (view.vt().attached)(view.ptr(), win.host as *mut c_void, c"HWND".as_ptr()) != OK {
                (view.vt().set_frame)(view.ptr(), null_mut());
                frame.hwnd.store(0, Relaxed);
                return Err("This plugin's editor failed to open.".into());
            }
            // Plugins often settle on their real size only once attached.
            let mut now = ViewRect::default();
            if (view.vt().get_size)(view.ptr(), &mut now) == OK && now.width() > 0 && now.height() > 0 && (now.width(), now.height()) != (w, h) {
                resize_for_plugin(win.hwnd as isize, now.width(), now.height());
            }
            if resizable {
                let v = view.clone();
                let fp = self.frame;
                win.set_on_size(Box::new(move |cw, ch| {
                    if (*fp).resizing.load(Relaxed) || cw <= 0 || ch <= 0 {
                        return;
                    }
                    let mut rc = ViewRect { left: 0, top: 0, right: cw, bottom: ch };
                    (v.vt().check_size_constraint)(v.ptr(), &mut rc);
                    (v.vt().on_size)(v.ptr(), &mut rc);
                }));
            }
            win.show();
            self.view = Some(view);
            self.win = Some(win);
        }
        Ok(())
    }

    fn editor_open(&self) -> bool {
        self.win.is_some()
    }

    fn close_editor(&mut self) {
        if let Some(view) = self.view.take() {
            unsafe {
                (view.vt().removed)(view.ptr());
                (view.vt().set_frame)(view.ptr(), null_mut());
                (*self.frame).hwnd.store(0, Relaxed);
            }
        }
        self.win = None;
    }

    fn set_editor_on_top(&mut self, on: bool) {
        if let Some(w) = &self.win {
            w.set_topmost(on);
        }
    }

    fn idle(&mut self) -> bool {
        // Values the processor reported (meters, MIDI-mapped moves) reach the controller and its window.
        if let Some(c) = &self.inst.controller {
            while let Ok((id, v)) = self.from_audio.pop() {
                unsafe { (c.vt().set_param_normalized)(c.ptr(), id, v) };
            }
        }
        let flags = self.shared.restart.swap(0, Relaxed);
        if flags & RESTART_PARAM_TITLES != 0 {
            let fresh = unsafe { read_params(&self.inst) };
            if fresh.len() == self.params.len() {
                self.params = fresh;
            }
        }
        if self.win.as_ref().is_some_and(|w| w.closed()) {
            self.close_editor();
        }
        if flags & RESTART_LATENCY != 0 {
            let _between_blocks = self.shared.processing.lock().unwrap_or_else(|e| e.into_inner());
            unsafe {
                self.inst.deactivate();
                self.inst.start_processing();
            }
            return true;
        }
        false
    }

    fn latency(&self) -> u32 {
        self.inst.latency_samples.load(Relaxed)
    }
}

impl Drop for V3Ui {
    fn drop(&mut self) {
        self.close_editor();
        unsafe { release(self.frame as *mut c_void) };
    }
}
