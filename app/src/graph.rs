//! The patch model: processor boxes, their connections and parameter mappings, conversion to and
//! from the .pdl patch XML, and the description the audio engine builds its plan from.

use crate::builtins::{self, Builtin, Kind};
use crate::engine::{GraphDesc, MidiMap, NodeCell, NodeDesc, OscMap, StepKind, MIDI_PIN};
use crate::pdl::{b64_decode, b64_encode, xml_from_binary, xml_to_binary, El};
use crate::plugins::{self, PluginInfo, PluginUi};
use egui::{pos2, Pos2, Vec2};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub const AUDIO_IN: &str = "Audio Input";
pub const AUDIO_OUT: &str = "Audio Output";
pub const MIDI_IN: &str = "Midi Input";
pub const OSC_IN: &str = "OSC Input";

/// Default box positions are written for a 1024×580 window and scaled (truncated) to the real
/// window size (0x14002cc30, 0x14007b120).
pub fn default_pos(name: &str, window: Vec2) -> Pos2 {
    let (x, y) = match name {
        AUDIO_IN => (10.0, 10.0),
        MIDI_IN => (10.0, 120.0),
        OSC_IN => (10.0, 215.0),
        AUDIO_OUT => (892.0, 10.0),
        _ => (10.0, 10.0),
    };
    pos2((x * window.x / 1024.0).floor(), (y * window.y / 580.0).floor())
}

pub enum NodeKind {
    AudioIn,
    AudioOut,
    MidiIn,
    OscIn,
    Builtin(Builtin),
    Plugin(Box<dyn PluginUi>),
    /// A plugin that couldn't be loaded; its saved state is kept so saving doesn't lose it.
    Missing(String),
}

pub struct Node {
    pub id: u32,
    pub kind: NodeKind,
    pub info: PluginInfo,
    pub pos: Pos2,
    pub cell: Arc<NodeCell>,
    pub ins: usize,
    pub outs: usize,
    /// The name the person gave the box (the patch's `UserNames`); empty for none.
    pub user_name: String,
}

impl Node {
    pub fn name(&self) -> &str {
        &self.info.name
    }
    /// What the box's title shows: the person's name for it, or the processor's own.
    pub fn title(&self) -> &str {
        if self.user_name.is_empty() { &self.info.name } else { &self.user_name }
    }
    pub fn is_io(&self) -> bool {
        matches!(self.kind, NodeKind::AudioIn | NodeKind::AudioOut | NodeKind::MidiIn | NodeKind::OscIn)
    }
    /// Every processor has a "param" input pin: MIDI and parameter mappings arrive there.
    pub fn param_in(&self) -> bool {
        !self.is_io()
    }
    pub fn param_out(&self) -> bool {
        matches!(self.kind, NodeKind::MidiIn | NodeKind::OscIn)
    }
    pub fn in_pins(&self) -> usize {
        self.ins + self.param_in() as usize
    }
    pub fn out_pins(&self) -> usize {
        self.outs + self.param_out() as usize
    }
    pub fn in_channel(&self, pin: usize) -> u32 {
        if pin < self.ins { pin as u32 } else { MIDI_PIN }
    }
    pub fn out_channel(&self, pin: usize) -> u32 {
        if pin < self.outs { pin as u32 } else { MIDI_PIN }
    }
    pub fn in_pin(&self, ch: u32) -> Option<usize> {
        if ch == MIDI_PIN { self.param_in().then_some(self.ins) } else { ((ch as usize) < self.ins).then_some(ch as usize) }
    }
    pub fn out_pin(&self, ch: u32) -> Option<usize> {
        if ch == MIDI_PIN { self.param_out().then_some(self.outs) } else { ((ch as usize) < self.outs).then_some(ch as usize) }
    }
    /// Channel names as the original's graph I/O processors report them.
    pub fn in_name(&self, pin: usize) -> String {
        if pin >= self.ins {
            return "param".into();
        }
        match self.kind {
            NodeKind::AudioOut => format!("Output {}", pin + 1),
            _ => format!("Input {}", pin + 1),
        }
    }
    pub fn out_name(&self, pin: usize) -> String {
        if pin >= self.outs {
            return "param".into();
        }
        match self.kind {
            NodeKind::AudioIn => format!("Input {}", pin + 1),
            _ => format!("Output {}", pin + 1),
        }
    }
    pub fn bypassed(&self) -> bool {
        self.cell.bypass.load(Ordering::Relaxed)
    }
    pub fn set_bypass(&self, on: bool) {
        self.cell.bypass.store(on, Ordering::Relaxed);
    }
    pub fn num_params(&self) -> i32 {
        match &self.kind {
            NodeKind::Builtin(b) => b.kind.params().len() as i32,
            NodeKind::Plugin(p) => p.num_params(),
            _ => 0,
        }
    }
    pub fn param_name(&self, i: i32) -> String {
        match &self.kind {
            NodeKind::Builtin(b) => b.kind.params().get(i as usize).map(|s| s.to_string()).unwrap_or_default(),
            NodeKind::Plugin(p) => p.param_name(i),
            _ => String::new(),
        }
    }
    pub fn param_value(&self, i: i32) -> f32 {
        match &self.kind {
            NodeKind::Builtin(b) => b.param_value(i),
            NodeKind::Plugin(p) => p.param_value(i),
            _ => 0.0,
        }
    }
    pub fn set_param(&mut self, i: i32, v: f32) {
        match &mut self.kind {
            NodeKind::Builtin(b) => b.set_param(i, v),
            NodeKind::Plugin(p) => p.set_param(i, v),
            _ => {}
        }
    }
    pub fn plugin(&mut self) -> Option<&mut Box<dyn PluginUi>> {
        match &mut self.kind {
            NodeKind::Plugin(p) => Some(p),
            _ => None,
        }
    }
    pub fn program(&self) -> i32 {
        match &self.kind {
            NodeKind::Plugin(p) => p.program(),
            _ => 0,
        }
    }
    fn step_kind(&self) -> StepKind {
        match self.kind {
            NodeKind::AudioIn => StepKind::AudioIn,
            NodeKind::AudioOut => StepKind::AudioOut,
            NodeKind::MidiIn => StepKind::MidiIn,
            NodeKind::OscIn => StepKind::OscIn,
            _ => StepKind::Proc,
        }
    }
    /// The processor's state as the base64 text stored in a FILTER's STATE element.
    pub fn state_b64(&mut self) -> Option<String> {
        match &mut self.kind {
            NodeKind::Builtin(b) => b.get_state().map(|e| b64_encode(&xml_to_binary(&e))),
            NodeKind::Plugin(p) => {
                let s = p.get_state();
                (!s.is_empty()).then(|| b64_encode(&s))
            }
            NodeKind::Missing(s) => (!s.is_empty()).then(|| s.clone()),
            _ => None,
        }
    }
    pub fn set_state_b64(&mut self, text: &str) -> bool {
        let Some(data) = b64_decode(text.trim()) else { return false };
        match &mut self.kind {
            NodeKind::Builtin(b) => match xml_from_binary(&data) {
                Some(e) => {
                    b.set_state(&e);
                    true
                }
                None => false,
            },
            NodeKind::Plugin(p) => p.set_state(&data),
            _ => false,
        }
    }
}

/// What node creation needs to know about the running device and the window.
pub struct Ctx {
    pub sample_rate: f64,
    pub block: usize,
    pub dev_ins: usize,
    pub dev_outs: usize,
    pub window: Vec2,
    /// Audio Input, Midi Input and OSC Input boxes enabled in Misc Settings.
    pub audio_in: bool,
    pub midi_in: bool,
    pub osc_in: bool,
}

pub fn internal_info(name: &str, ins: usize, outs: usize) -> PluginInfo {
    PluginInfo {
        name: name.into(),
        format: "Internal".into(),
        manufacturer: "Internal".into(),
        version: "1.00".into(),
        num_inputs: ins as u32,
        num_outputs: outs as u32,
        ..Default::default()
    }
}

fn info_el(i: &PluginInfo) -> El {
    El::new("PLUGIN")
        .attr("name", &i.name)
        .attr("descriptiveName", &i.name)
        .attr("format", &i.format)
        .attr("category", &i.category)
        .attr("manufacturer", &i.manufacturer)
        .attr("version", &i.version)
        .attr("file", &i.file)
        .attr("uid", &i.uid)
        .attr("isInstrument", i.is_instrument as i32)
        .attr("fileTime", 0)
        .attr("numInputs", i.num_inputs)
        .attr("numOutputs", i.num_outputs)
}

fn info_from(e: &El) -> PluginInfo {
    let s = |k: &str| e.get(k).unwrap_or("").to_string();
    PluginInfo {
        name: s("name"),
        format: s("format"),
        category: s("category"),
        manufacturer: s("manufacturer"),
        version: s("version"),
        file: s("file"),
        uid: s("uid"),
        is_instrument: e.get_b("isInstrument", false),
        num_inputs: e.get_i("numInputs", 0).max(0) as u32,
        num_outputs: e.get_i("numOutputs", 0).max(0) as u32,
    }
}

#[derive(Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub conns: Vec<(u32, u32, u32, u32)>,
    pub midi_maps: Vec<MidiMap>,
    pub osc_maps: Vec<OscMap>,
    pub next_id: u32,
}

impl Graph {
    /// An empty patch: just the enabled I/O boxes, unconnected.
    pub fn new_default(ctx: &Ctx) -> Graph {
        let mut g = Graph { next_id: 1, ..Default::default() };
        g.ensure_io(ctx);
        g
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: u32) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    fn alloc_id(&mut self) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn io_node(id: u32, name: &str, ctx: &Ctx, pos: Pos2) -> Node {
        let (kind, ins, outs) = match name {
            AUDIO_IN => (NodeKind::AudioIn, 0, ctx.dev_ins),
            AUDIO_OUT => (NodeKind::AudioOut, ctx.dev_outs, 0),
            MIDI_IN => (NodeKind::MidiIn, 0, 0),
            _ => (NodeKind::OscIn, 0, 0),
        };
        Node { id, kind, info: internal_info(name, ins, outs), pos, cell: NodeCell::new(id, None), ins, outs, user_name: String::new() }
    }

    /// Adds or removes the I/O boxes to match the Misc Settings toggles. Returns removed nodes.
    pub fn ensure_io(&mut self, ctx: &Ctx) -> Vec<Node> {
        let mut removed = Vec::new();
        for (name, on) in [(AUDIO_IN, ctx.audio_in), (AUDIO_OUT, true), (MIDI_IN, ctx.midi_in), (OSC_IN, ctx.osc_in)] {
            let id = self.nodes.iter().find(|n| n.is_io() && n.name() == name).map(|n| n.id);
            match (id, on) {
                (None, true) => {
                    let id = self.alloc_id();
                    self.nodes.push(Self::io_node(id, name, ctx, default_pos(name, ctx.window)));
                }
                (Some(id), false) => removed.extend(self.remove(id)),
                _ => {}
            }
        }
        removed
    }

    fn builtin_node(id: u32, kind: Kind, pos: Pos2, ctx: &Ctx) -> Node {
        let (b, mut p) = builtins::create(kind);
        p.prepare(ctx.sample_rate, ctx.block);
        let (ins, outs) = kind.io();
        Node { id, kind: NodeKind::Builtin(b), info: internal_info(kind.name(), ins, outs), pos, cell: NodeCell::new(id, Some(p)), ins, outs, user_name: String::new() }
    }

    fn plugin_node(id: u32, info: &PluginInfo, pos: Pos2, ctx: &Ctx) -> Result<Node, String> {
        let _stage = crate::watchdog::enter(crate::watchdog::Stage::PluginLoad);
        let l = plugins::instantiate(info, ctx.sample_rate, ctx.block)?;
        let mut info = info.clone();
        info.num_inputs = l.ins as u32;
        info.num_outputs = l.outs as u32;
        Ok(Node { id, kind: NodeKind::Plugin(l.ui), info, pos, cell: NodeCell::new(id, Some(l.proc_)), ins: l.ins, outs: l.outs, user_name: String::new() })
    }

    pub fn add_builtin(&mut self, kind: Kind, pos: Pos2, ctx: &Ctx) -> u32 {
        let id = self.alloc_id();
        self.nodes.push(Self::builtin_node(id, kind, pos, ctx));
        id
    }

    pub fn add_plugin(&mut self, info: &PluginInfo, pos: Pos2, ctx: &Ctx) -> Result<u32, String> {
        let id = self.next_id.max(1);
        let n = Self::plugin_node(id, info, pos, ctx)?;
        self.next_id = id + 1;
        self.nodes.push(n);
        Ok(id)
    }

    /// Removes a box with its connections and mappings.
    pub fn remove(&mut self, id: u32) -> Option<Node> {
        let i = self.nodes.iter().position(|n| n.id == id)?;
        self.conns.retain(|c| c.0 != id && c.2 != id);
        self.midi_maps.retain(|m| m.node != id);
        self.osc_maps.retain(|m| m.node != id);
        Some(self.nodes.remove(i))
    }

    /// Audio pins only join audio pins and param pins only param pins; no duplicates, no
    /// connection to the same box and no feedback loops.
    pub fn can_connect(&self, s: u32, sc: u32, d: u32, dc: u32) -> bool {
        if s == d || (sc == MIDI_PIN) != (dc == MIDI_PIN) || self.conns.contains(&(s, sc, d, dc)) {
            return false;
        }
        let (Some(a), Some(b)) = (self.node(s), self.node(d)) else { return false };
        a.out_pin(sc).is_some() && b.in_pin(dc).is_some() && !self.reaches(d, s)
    }

    fn reaches(&self, from: u32, to: u32) -> bool {
        let mut stack = vec![from];
        let mut seen = Vec::new();
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !seen.contains(&n) {
                seen.push(n);
                stack.extend(self.conns.iter().filter(|c| c.0 == n).map(|c| c.2));
            }
        }
        false
    }

    pub fn connect(&mut self, s: u32, sc: u32, d: u32, dc: u32) -> bool {
        let ok = self.can_connect(s, sc, d, dc);
        if ok {
            self.conns.push((s, sc, d, dc));
        }
        ok
    }

    /// Resizes the audio I/O boxes to the device's active channels and drops dangling cables.
    pub fn set_device_channels(&mut self, ins: usize, outs: usize) {
        for n in &mut self.nodes {
            match n.kind {
                NodeKind::AudioIn => n.outs = ins,
                NodeKind::AudioOut => n.ins = outs,
                _ => {}
            }
        }
        let nodes = &self.nodes;
        let pin_ok = |id: u32, ch: u32, out: bool| {
            nodes.iter().find(|n| n.id == id).is_some_and(|n| if out { n.out_pin(ch).is_some() } else { n.in_pin(ch).is_some() })
        };
        self.conns.retain(|c| pin_ok(c.0, c.1, true) && pin_ok(c.2, c.3, false));
    }

    pub fn desc(&self) -> GraphDesc {
        GraphDesc {
            nodes: self.nodes.iter().map(|n| NodeDesc { cell: n.cell.clone(), kind: n.step_kind(), ins: n.ins, outs: n.outs }).collect(),
            conns: self.conns.clone(),
            midi_maps: self.midi_maps.clone(),
            osc_maps: self.osc_maps.clone(),
        }
    }

    /// Re-prepares every processor for a new rate or block size.
    ///
    /// # Safety
    /// The audio device must be stopped.
    pub unsafe fn prepare_all(&self, sample_rate: f64, block: usize) {
        for n in &self.nodes {
            if let Some(p) = n.cell.processor() {
                p.prepare(sample_rate, block);
            }
        }
    }

    /// Writes one `<Patch>` element as the original does (0x14007d3f0): the FILTERGRAPH, then
    /// `<Mappings>` holding the MIDI and OSC mappings, then `<UserNames>` with one
    /// `<Name id=".." val=".."/>` per box the person renamed.
    pub fn to_patch_el(&mut self, name: &str, tempo: f64) -> El {
        let mut fg = El::new("FILTERGRAPH");
        for n in &mut self.nodes {
            let mut f = El::new("FILTER").attr("uid", n.id).attr("x", n.pos.x.round()).attr("y", n.pos.y.round());
            f.set("program", n.program());
            if n.bypassed() {
                f.set("bypass", 1);
            }
            let state = n.state_b64();
            f = f.with_child(info_el(&n.info));
            if let Some(s) = state {
                let mut st = El::new("STATE");
                st.text = s;
                f = f.with_child(st);
            }
            fg = fg.with_child(f);
        }
        for c in &self.conns {
            fg = fg.with_child(El::new("CONNECTION").attr("srcFilter", c.0).attr("srcChannel", c.1).attr("dstFilter", c.2).attr("dstChannel", c.3));
        }
        let p = El::new("Patch").attr("name", name).attr("tempo", format!("{tempo:.2}")).with_child(fg);
        let mut maps = El::new("Mappings");
        for m in &self.midi_maps {
            maps = maps.with_child(
                El::new("MidiMapping")
                    .attr("pluginId", m.node)
                    .attr("parameterIndex", m.param)
                    .attr("cc", m.cc)
                    .attr("latch", m.latch as i32)
                    .attr("channel", m.channel)
                    .attr("lowerBound", m.lower)
                    .attr("upperBound", m.upper),
            );
        }
        for m in &self.osc_maps {
            maps = maps.with_child(El::new("OscMapping").attr("pluginId", m.node).attr("parameterIndex", m.param).attr("address", &m.address));
        }
        let mut names = El::new("UserNames");
        for n in self.nodes.iter().filter(|n| !n.user_name.is_empty()) {
            names = names.with_child(El::new("Name").attr("id", n.id).attr("val", &n.user_name));
        }
        p.with_child(maps).with_child(names)
    }

    /// Builds a patch from its `<Patch>` element (0x14007dd40). Plugins that fail to load are
    /// kept as placeholders and reported in the returned messages.
    pub fn from_patch_el(patch: &El, ctx: &Ctx) -> (Graph, Vec<String>) {
        let mut g = Graph { next_id: 1, ..Default::default() };
        let mut errors = Vec::new();
        let Some(fg) = patch.child("FILTERGRAPH") else {
            g.ensure_io(ctx);
            return (g, errors);
        };
        let mut alias: Vec<(u32, u32)> = Vec::new();
        let mut interceptor = None;
        for f in fg.named("FILTER") {
            let Some(pe) = f.child("PLUGIN") else { continue };
            let id = f.get_i("uid", 0).max(0) as u32;
            if g.node(id).is_some() {
                continue;
            }
            let info = info_from(pe);
            let mut pos = pos2(f.get_f("x", 10.0) as f32, f.get_f("y", 10.0) as f32);
            if pos.x <= 1.0 && pos.y <= 1.0 && (pos.x.fract() != 0.0 || pos.y.fract() != 0.0) {
                pos = pos2(pos.x * ctx.window.x, pos.y * ctx.window.y);
            }
            let state = f.child("STATE").map(|s| s.text.trim().to_string()).unwrap_or_default();
            g.next_id = g.next_id.max(id + 1);
            let mut node = if info.format.eq_ignore_ascii_case("Internal") {
                match info.name.as_str() {
                    "Midi Interceptor" => {
                        interceptor = Some(id);
                        continue;
                    }
                    n @ (AUDIO_IN | AUDIO_OUT | MIDI_IN | OSC_IN) => {
                        let on = match n {
                            AUDIO_IN => ctx.audio_in,
                            MIDI_IN => ctx.midi_in,
                            OSC_IN => ctx.osc_in,
                            _ => true,
                        };
                        if !on || g.nodes.iter().any(|x| x.name() == n && x.is_io()) {
                            continue;
                        }
                        Self::io_node(id, n, ctx, pos)
                    }
                    n => match Kind::from_name(n) {
                        Some(k) => Self::builtin_node(id, k, pos, ctx),
                        None => {
                            errors.push(format!("Unknown built-in processor: {n}"));
                            continue;
                        }
                    },
                }
            } else {
                match Self::plugin_node(id, &info, pos, ctx) {
                    Ok(n) => n,
                    Err(e) => {
                        errors.push(format!("{}: {e}", info.name));
                        let (ins, outs) = (info.num_inputs as usize, info.num_outputs as usize);
                        Node { id, kind: NodeKind::Missing(state.clone()), info, pos, cell: NodeCell::new(id, None), ins, outs, user_name: String::new() }
                    }
                }
            };
            if !state.is_empty() && !matches!(node.kind, NodeKind::Missing(_)) {
                node.set_state_b64(&state);
            }
            let prog = f.get_i("program", -1);
            if let (true, Some(p)) = (prog >= 0, node.plugin()) {
                if p.num_programs() > prog as i32 && p.program() != prog as i32 {
                    p.set_program(prog as i32);
                }
            }
            node.set_bypass(f.get_b("bypass", false));
            g.nodes.push(node);
        }
        if let Some(un) = patch.child("UserNames") {
            for e in un.named("Name") {
                let id = e.get_i("id", -1);
                let val = e.get("val").unwrap_or("").trim();
                if let Some(n) = g.nodes.iter_mut().find(|n| id >= 0 && n.id == id as u32) {
                    n.user_name = val.to_string();
                }
            }
        }
        // The original routes MIDI through a hidden interceptor box; join its cables to Midi Input.
        if let (Some(i), Some(m)) = (interceptor, g.nodes.iter().find(|n| matches!(n.kind, NodeKind::MidiIn)).map(|n| n.id)) {
            alias.push((i, m));
        }
        g.ensure_io(ctx);
        let map = |id: u32| alias.iter().find(|a| a.0 == id).map(|a| a.1).unwrap_or(id);
        for c in fg.named("CONNECTION") {
            let geti = |k: &str| c.get_i(k, -1).max(0) as u32;
            let (s, sc, d, dc) = (map(geti("srcFilter")), geti("srcChannel"), map(geti("dstFilter")), geti("dstChannel"));
            g.connect(s, sc, d, dc);
        }
        let mut found = Vec::new();
        patch.find_all("MidiMapping", &mut found);
        for m in found {
            let node = m.get_i("pluginId", -1).max(0) as u32;
            if g.node(node).is_some() {
                g.midi_maps.push(MidiMap {
                    node,
                    param: m.get_i("parameterIndex", 0) as i32,
                    cc: m.get_i("cc", 0).clamp(0, 127) as u8,
                    channel: m.get_i("channel", 0).clamp(0, 16) as u8,
                    latch: m.get_b("latch", false),
                    lower: m.get_f("lowerBound", 0.0) as f32,
                    upper: m.get_f("upperBound", 1.0) as f32,
                });
            }
        }
        let mut found = Vec::new();
        patch.find_all("OscMapping", &mut found);
        for m in found {
            let node = m.get_i("pluginId", -1).max(0) as u32;
            if g.node(node).is_some() {
                g.osc_maps.push(OscMap { node, param: m.get_i("parameterIndex", 0) as i32, address: m.get("address").unwrap_or("").to_string() });
            }
        }
        (g, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_keeps_box_names_and_nests_mappings_like_the_original() {
        let ctx = Ctx { sample_rate: 44100.0, block: 256, dev_ins: 2, dev_outs: 2, window: Vec2::new(1024.0, 580.0), audio_in: true, midi_in: true, osc_in: true };
        let mut g = Graph::new_default(&ctx);
        let out = g.nodes.iter_mut().find(|n| matches!(n.kind, NodeKind::AudioOut)).unwrap();
        out.user_name = "Amp".into();
        let id = out.id;
        g.midi_maps.push(MidiMap { node: id, param: -1, cc: 7, channel: 1, latch: false, lower: 0.0, upper: 1.0 });
        g.osc_maps.push(OscMap { node: id, param: -1, address: "/amp".into() });

        let el = g.to_patch_el("Song", 120.0);
        let maps = el.child("Mappings").expect("Mappings element");
        assert_eq!((maps.named("MidiMapping").count(), maps.named("OscMapping").count()), (1, 1));
        let names: Vec<&El> = el.child("UserNames").expect("UserNames element").named("Name").collect();
        assert_eq!(names.len(), 1);
        assert_eq!((names[0].get_i("id", -1), names[0].get("val")), (id as i64, Some("Amp")));

        let (back, errors) = Graph::from_patch_el(&El::parse(&el.to_document()).unwrap(), &ctx);
        assert!(errors.is_empty());
        assert_eq!(back.node(id).map(|n| (n.title(), n.name())), Some(("Amp", AUDIO_OUT)));
        assert!(back.nodes.iter().filter(|n| n.id != id).all(|n| n.user_name.is_empty() && n.title() == n.name()));
        assert_eq!((back.midi_maps.len(), back.osc_maps.len()), (1, 1));
    }
}
