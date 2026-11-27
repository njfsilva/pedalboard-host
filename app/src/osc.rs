//! OSC over UDP. A small thread decodes messages and bundles, hands the first number of each
//! message to the audio thread (OSC mappings act there) and passes the whole message to the
//! window for the event log and application mappings.

use crate::app::Incoming;
use crate::engine::{osc_hash, OscEvent};
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    F(f32),
    I(i32),
    D(f64),
    H(i64),
    S(String),
    B(Vec<u8>),
    True,
    False,
    Nil,
    Impulse,
}

impl Arg {
    pub fn value(&self) -> Option<f32> {
        match self {
            Arg::F(v) => Some(*v),
            Arg::I(v) => Some(*v as f32),
            Arg::D(v) => Some(*v as f32),
            Arg::H(v) => Some(*v as f32),
            Arg::True | Arg::Impulse => Some(1.0),
            Arg::False => Some(0.0),
            _ => None,
        }
    }
}

pub struct Message {
    pub address: String,
    pub tags: String,
    pub args: Vec<Arg>,
}

pub struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Why the multicast group couldn't be joined; the server still listens on the port.
    pub warning: Option<String>,
}

impl Server {
    /// Listens on every interface at `port`, as the original binds its socket (0x140016fa0), and
    /// with a multicast address also joins that group on any interface, so messages sent to the
    /// group arrive as well as ones sent straight to this computer.
    pub fn start(port: u16, multicast: Option<Ipv4Addr>, to_audio: Arc<Mutex<rtrb::Producer<OscEvent>>>, to_ui: Sender<Incoming>, ctx: egui::Context) -> Result<Server, String> {
        let sock = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| e.to_string())?;
        sock.set_read_timeout(Some(Duration::from_millis(200))).map_err(|e| e.to_string())?;
        let warning = multicast.and_then(|g| sock.join_multicast_v4(&g, &Ipv4Addr::UNSPECIFIED).err().map(|e| format!("could not join multicast group {g}: {e}")));
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("osc".into())
            .spawn(move || {
                let mut buf = vec![0u8; 65536];
                while !s.load(Ordering::Relaxed) {
                    let Ok(n) = sock.recv(&mut buf) else { continue };
                    let mut msgs = Vec::new();
                    parse_packet(&buf[..n], &mut msgs, 0);
                    for m in msgs {
                        if let Some(v) = m.args.iter().find_map(|a| a.value()) {
                            if let Ok(mut p) = to_audio.lock() {
                                let _ = p.push(OscEvent { hash: osc_hash(&m.address), value: v });
                            }
                        }
                        let _ = to_ui.send(Incoming::Osc { address: m.address, tags: m.tags, args: m.args, received: std::time::Instant::now() });
                    }
                    ctx.request_repaint();
                    crate::tray::wake();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Server { stop, thread: Some(thread), warning })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_str(b: &[u8], pos: &mut usize) -> Option<String> {
    let start = *pos;
    let len = b.get(start..)?.iter().position(|&c| c == 0)?;
    let s = String::from_utf8_lossy(&b[start..start + len]).into_owned();
    *pos = (start + len + 4) & !3;
    Some(s)
}

fn read4(b: &[u8], pos: &mut usize) -> Option<[u8; 4]> {
    let v = b.get(*pos..*pos + 4)?.try_into().ok()?;
    *pos += 4;
    Some(v)
}

fn read8(b: &[u8], pos: &mut usize) -> Option<[u8; 8]> {
    let v = b.get(*pos..*pos + 8)?.try_into().ok()?;
    *pos += 8;
    Some(v)
}

/// Decodes a packet: a message, or a bundle of messages and bundles.
pub fn parse_packet(b: &[u8], out: &mut Vec<Message>, depth: u32) {
    if depth > 8 || b.is_empty() {
        return;
    }
    if b.starts_with(b"#bundle\0") {
        let mut pos = 16;
        while let Some(size) = read4(b, &mut pos).map(|s| i32::from_be_bytes(s).max(0) as usize) {
            let Some(inner) = b.get(pos..pos + size) else { break };
            parse_packet(inner, out, depth + 1);
            pos += size;
        }
        return;
    }
    if let Some(m) = parse_message(b) {
        out.push(m);
    }
}

fn parse_message(b: &[u8]) -> Option<Message> {
    let mut pos = 0;
    let address = read_str(b, &mut pos)?;
    if !address.starts_with('/') {
        return None;
    }
    let tags = if b.get(pos) == Some(&b',') { read_str(b, &mut pos)? } else { String::from(",") };
    let mut args = Vec::new();
    for t in tags.chars().skip(1) {
        let a = match t {
            'i' | 'c' | 'r' | 'm' => Arg::I(i32::from_be_bytes(read4(b, &mut pos)?)),
            'f' => Arg::F(f32::from_be_bytes(read4(b, &mut pos)?)),
            'h' | 't' => Arg::H(i64::from_be_bytes(read8(b, &mut pos)?)),
            'd' => Arg::D(f64::from_be_bytes(read8(b, &mut pos)?)),
            's' | 'S' => Arg::S(read_str(b, &mut pos)?),
            'b' => {
                let n = i32::from_be_bytes(read4(b, &mut pos)?).max(0) as usize;
                let data = b.get(pos..pos + n)?.to_vec();
                pos += (n + 3) & !3;
                Arg::B(data)
            }
            'T' => Arg::True,
            'F' => Arg::False,
            'N' => Arg::Nil,
            'I' => Arg::Impulse,
            '[' | ']' => continue,
            _ => break,
        };
        args.push(a);
    }
    Some(Message { address, tags: tags[1..].to_string(), args })
}
