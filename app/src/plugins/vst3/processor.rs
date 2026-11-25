//! The audio-thread half of a VST3 instance. Everything it touches per block is allocated up
//! front: bus tables, parameter queues, the note list and the transport context.

use super::com::*;
use super::host::{EventList, ParamChanges};
use super::{Instance, MIDI_CTRLS};
use crate::engine::{MidiEvent, ProcessArgs, Processor, MIDI_CAP};
use std::ptr::null_mut;
use std::sync::Arc;

pub struct V3Proc {
    inst: Arc<Instance>,
    in_ch: Vec<i32>,
    out_ch: Vec<i32>,
    in_bufs: Vec<Vec<f32>>,
    out_bufs: Vec<Vec<f32>>,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    in_buses: Vec<AudioBusBuffers>,
    out_buses: Vec<AudioBusBuffers>,
    params_in: Box<ParamChanges>,
    params_out: Box<ParamChanges>,
    events: Box<EventList>,
    ctx: ProcessContext,
    from_ui: rtrb::Consumer<(u32, f64)>,
    to_ui: rtrb::Producer<(u32, f64)>,
    param_ids: Vec<u32>,
    midi_map: Vec<u32>,
    cap: usize,
    clock: i64,
}

// The pointer tables only ever point into buffers this struct owns or the graph lends per call.
unsafe impl Send for V3Proc {}

/// One `AudioBusBuffers` per bus, each pointing at its run of channel pointers.
fn bus_table(chans: &[i32], ptrs: &mut [*mut f32]) -> Vec<AudioBusBuffers> {
    let base = ptrs.as_mut_ptr();
    let mut at = 0;
    chans
        .iter()
        .map(|&n| {
            let b = AudioBusBuffers { num_channels: n, silence_flags: 0, buffers: unsafe { base.add(at) } };
            at += n as usize;
            b
        })
        .collect()
}

impl V3Proc {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        inst: Arc<Instance>,
        in_ch: Vec<i32>,
        out_ch: Vec<i32>,
        param_ids: Vec<u32>,
        midi_map: Vec<u32>,
        from_ui: rtrb::Consumer<(u32, f64)>,
        to_ui: rtrb::Producer<(u32, f64)>,
        block: usize,
    ) -> V3Proc {
        let queues = param_ids.len().clamp(16, 512);
        let mut p = V3Proc {
            inst,
            in_ch,
            out_ch,
            in_bufs: Vec::new(),
            out_bufs: Vec::new(),
            in_ptrs: Vec::new(),
            out_ptrs: Vec::new(),
            in_buses: Vec::new(),
            out_buses: Vec::new(),
            params_in: ParamChanges::new(queues),
            params_out: ParamChanges::new(queues),
            events: EventList::new(MIDI_CAP),
            ctx: ProcessContext::default(),
            from_ui,
            to_ui,
            param_ids,
            midi_map,
            cap: 0,
            clock: 0,
        };
        p.alloc(block);
        p
    }

    fn alloc(&mut self, block: usize) {
        self.cap = block.max(16);
        let total = |ch: &[i32]| ch.iter().map(|&n| n as usize).sum::<usize>();
        self.in_bufs = vec![vec![0.0; self.cap]; total(&self.in_ch)];
        self.out_bufs = vec![vec![0.0; self.cap]; total(&self.out_ch)];
        self.in_ptrs = self.in_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        self.out_ptrs = self.out_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        self.in_buses = bus_table(&self.in_ch, &mut self.in_ptrs);
        self.out_buses = bus_table(&self.out_ch, &mut self.out_ptrs);
    }

    fn midi(&mut self, ev: &MidiEvent, frames: usize) {
        let offset = (ev.frame as usize).min(frames - 1) as i32;
        let ch = (ev.data[0] & 0x0F) as i16;
        let (d1, d2) = (ev.data[1] & 0x7F, ev.data[2] & 0x7F);
        let mut e = Event { bus_index: 0, sample_offset: offset, ppq_position: 0.0, flags: EVENT_IS_LIVE, kind: 0, body: EventBody { raw: [0; 3] } };
        match ev.status() {
            0x90 if d2 > 0 => {
                e.kind = EVENT_NOTE_ON;
                e.body.note_on = NoteOn { channel: ch, pitch: d1 as i16, tuning: 0.0, velocity: d2 as f32 / 127.0, length: 0, note_id: -1 };
            }
            0x80 | 0x90 => {
                e.kind = EVENT_NOTE_OFF;
                e.body.note_off = NoteOff { channel: ch, pitch: d1 as i16, velocity: d2 as f32 / 127.0, note_id: -1, tuning: 0.0 };
            }
            0xA0 => {
                e.kind = EVENT_POLY_PRESSURE;
                e.body.poly = PolyPressure { channel: ch, pitch: d1 as i16, pressure: d2 as f32 / 127.0, note_id: -1 };
            }
            // Controllers reach VST3 plugins as parameters, through the plugin's own MIDI mapping.
            0xB0 => return self.midi_param(ch, d1 as usize, d2 as f64 / 127.0, offset),
            0xD0 => return self.midi_param(ch, CTRL_AFTERTOUCH, d1 as f64 / 127.0, offset),
            0xE0 => return self.midi_param(ch, CTRL_PITCH_BEND, ((d2 as u32) << 7 | d1 as u32) as f64 / 16383.0, offset),
            _ => return,
        }
        self.events.push(e);
    }

    fn midi_param(&mut self, ch: i16, ctrl: usize, value: f64, offset: i32) {
        let id = self.midi_map[ch as usize * MIDI_CTRLS + ctrl];
        if id != u32::MAX {
            self.params_in.add(id, offset, value);
            let _ = self.to_ui.push((id, value));
        }
    }
}

impl Processor for V3Proc {
    fn prepare(&mut self, sample_rate: f64, max_block: usize) {
        unsafe { self.inst.activate(sample_rate, max_block.max(16)) };
        self.alloc(max_block);
        self.clock = 0;
    }

    fn process(&mut self, a: &ProcessArgs) {
        let n = a.frames;
        if n == 0 || n > self.cap {
            for i in 0..a.outputs.len() {
                a.output(i).fill(0.0);
            }
            return;
        }
        // Private input copies (plugins may process in place); the first bus takes the graph's inputs.
        let main_in = self.in_ch.first().map_or(0, |&c| c as usize);
        for i in 0..main_in {
            let d = &mut self.in_bufs[i][..n];
            if i < a.inputs.len() {
                d.copy_from_slice(a.input(i));
            } else {
                d.fill(0.0);
            }
        }
        // The first output bus renders straight into the graph's buffers.
        let main_out = self.out_ch.first().map_or(0, |&c| c as usize);
        for i in 0..main_out {
            self.out_ptrs[i] = if i < a.outputs.len() { a.outputs[i] } else { self.out_bufs[i].as_mut_ptr() };
        }
        while let Ok((id, v)) = self.from_ui.pop() {
            self.params_in.add(id, 0, v);
        }
        self.events.clear();
        for ev in a.midi_in {
            self.midi(ev, n);
        }
        let t = a.transport;
        let ppq = t.ppq();
        let bar = t.num.max(1) as f64 * 4.0 / t.den.max(1) as f64;
        let c = &mut self.ctx;
        c.state = CTX_TEMPO_VALID | CTX_TIME_SIG_VALID | CTX_PROJECT_MUSIC_VALID | CTX_BAR_VALID | CTX_CONT_TIME_VALID | if t.playing { CTX_PLAYING } else { 0 };
        c.sample_rate = t.sample_rate;
        c.project_time_samples = t.sample_pos;
        c.continuous_time_samples = self.clock;
        c.project_time_music = ppq;
        c.bar_position_music = (ppq / bar).floor() * bar;
        c.tempo = t.tempo;
        c.time_sig_numerator = t.num;
        c.time_sig_denominator = t.den;
        self.clock += n as i64;
        for b in &mut self.out_buses {
            b.silence_flags = 0;
        }
        let mut data = ProcessData {
            process_mode: MODE_REALTIME,
            sample_size: SAMPLE_32,
            num_samples: n as i32,
            num_inputs: self.in_buses.len() as i32,
            num_outputs: self.out_buses.len() as i32,
            inputs: self.in_buses.as_mut_ptr(),
            outputs: self.out_buses.as_mut_ptr(),
            in_params: self.params_in.ptr(),
            out_params: self.params_out.ptr(),
            in_events: self.events.ptr(),
            out_events: null_mut(),
            context: &mut self.ctx,
        };
        let p = &self.inst.processor;
        unsafe { (p.vt().process)(p.ptr(), &mut data) };
        self.params_in.clear();
        for (id, v) in self.params_out.last_values() {
            let _ = self.to_ui.push((id, v));
        }
        self.params_out.clear();
        for i in main_out..a.outputs.len() {
            a.output(i).fill(0.0);
        }
    }

    fn set_parameter(&mut self, index: i32, value: f32) {
        if let Some(&id) = usize::try_from(index).ok().and_then(|i| self.param_ids.get(i)) {
            self.params_in.add(id, 0, value as f64);
            let _ = self.to_ui.push((id, value as f64));
        }
    }
}
