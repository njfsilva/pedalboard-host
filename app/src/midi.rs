//! MIDI inputs. Each port's callback runs on the driver's own thread: it hands the event
//! straight to the audio thread (mappings act there without waiting for the screen), then tells
//! the window (event log, patch changes, application mappings).

use crate::app::Incoming;
use crate::engine::MidiEvent;
use midir::{Ignore, MidiInput, MidiInputConnection};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Stored in the settings when every port is switched off (an empty list means all ports).
pub const NONE: &str = "<none>";
const CLIENT: &str = "Pedalboard Host";

#[derive(Default)]
pub struct Inputs {
    conns: Vec<MidiInputConnection<()>>,
}

impl Inputs {
    pub fn count(&self) -> usize {
        self.conns.len()
    }

    pub fn open(enabled: &[String], to_audio: Arc<Mutex<rtrb::Producer<MidiEvent>>>, to_ui: Sender<Incoming>, ctx: egui::Context) -> (Inputs, Vec<String>) {
        let (mut conns, mut errors) = (Vec::new(), Vec::new());
        for name in port_names() {
            if !(enabled.is_empty() || enabled.contains(&name)) {
                continue;
            }
            let mut mi = match MidiInput::new(CLIENT) {
                Ok(m) => m,
                Err(e) => {
                    errors.push(format!("MIDI: {e}"));
                    break;
                }
            };
            mi.ignore(Ignore::TimeAndActiveSense);
            let Some(port) = mi.ports().into_iter().find(|p| mi.port_name(p).ok().as_deref() == Some(name.as_str())) else { continue };
            let (a, u, c) = (to_audio.clone(), to_ui.clone(), ctx.clone());
            let cb = move |_t: u64, bytes: &[u8], _: &mut ()| {
                if let Some(ev) = MidiEvent::new(bytes) {
                    if let Ok(mut p) = a.lock() {
                        let _ = p.push(ev);
                    }
                }
                if bytes.first().is_some_and(|b| *b < 0xF8) {
                    let _ = u.send(Incoming::Midi(bytes.to_vec()));
                    c.request_repaint();
                    crate::tray::wake();
                }
            };
            match mi.connect(&port, "input", cb, ()) {
                Ok(conn) => conns.push(conn),
                Err(e) => errors.push(format!("MIDI: could not open {name}: {e}")),
            }
        }
        (Inputs { conns }, errors)
    }
}

pub fn port_names() -> Vec<String> {
    let Ok(mi) = MidiInput::new(CLIENT) else { return Vec::new() };
    mi.ports().iter().filter_map(|p| mi.port_name(p).ok()).collect()
}
