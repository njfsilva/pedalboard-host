//! The application-wide command list and Pedalboard2 AppMappings.xml format.

pub mod editor;
pub mod keyboard;

use crate::app::Cmd;
use crate::osc::Arg;
use crate::pdl::El;
use crate::settings::AppMapping;
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::Path;
use std::time::Instant;

// MainPanel.h assigns permanent IDs. JUCE adds the application-level Quit command.
pub const GROUPS: &[(&str, &[(Cmd, u16, &str)])] = &[
    ("File", &[
        (Cmd::New, 1, "New"), (Cmd::Open, 2, "Open..."), (Cmd::Save, 3, "Save"),
        (Cmd::SaveAs, 4, "Save As..."), (Cmd::SaveAsDefault, 5, "Save As Default"),
        (Cmd::ResetDefault, 6, "Reset Default"), (Cmd::Exit, 7, "Exit"),
    ]),
    ("Edit", &[
        (Cmd::DeleteConnections, 8, "Delete selected connection(s)"),
        (Cmd::OrganisePatches, 9, "Organise patches"), (Cmd::UserPresets, 21, "User Preset Management"),
    ]),
    ("Options", &[
        (Cmd::MiscSettings, 12, "Misc Settings"), (Cmd::AudioSettings, 10, "Audio Settings"),
        (Cmd::PluginList, 11, "Plugin List"), (Cmd::ColourSchemes, 13, "Colour Schemes"),
        (Cmd::AppMappings, 14, "Application Mappings"),
    ]),
    ("Help", &[(Cmd::About, 15, "About"), (Cmd::Documentation, 22, "Documentation"), (Cmd::EventLog, 23, "Event Log")]),
    ("Patch", &[(Cmd::NextPatch, 16, "Next Patch"), (Cmd::PrevPatch, 17, "Previous Patch")]),
    ("Main Transport", &[(Cmd::PlayPause, 18, "Play/Pause"), (Cmd::Rtz, 19, "Return to Zero"), (Cmd::TapTempo, 20, "Tap Tempo")]),
    ("Application", &[(Cmd::Quit, 0x1001, "Quit")]),
];

fn commands() -> impl Iterator<Item = &'static (Cmd, u16, &'static str)> {
    GROUPS.iter().flat_map(|(_, entries)| entries.iter())
}

#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    Key(String),
    Midi { cc: u8, channel: u8 },
    Osc { address: String, parameter: usize },
}

impl Trigger {
    fn label(&self) -> String {
        match self {
            Self::Key(key) => key.clone(),
            Self::Midi { cc, channel: 0 } => format!("MIDI CC: {cc}"),
            Self::Midi { cc, channel } => format!("MIDI CC: {cc} (ch {channel})"),
            Self::Osc { address, parameter: 0 } => address.clone(),
            Self::Osc { address, parameter } => format!("{address} [{parameter}]"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mappings(Vec<(Cmd, Trigger)>);

impl Default for Mappings {
    fn default() -> Self {
        Self([
            (Cmd::New, "ctrl + N"), (Cmd::Open, "ctrl + O"), (Cmd::Save, "ctrl + S"),
            (Cmd::SaveAs, "ctrl + shift + S"), (Cmd::DeleteConnections, "delete"),
            (Cmd::DeleteConnections, "backspace"), (Cmd::Documentation, "F1"),
            (Cmd::PlayPause, "spacebar"), (Cmd::Quit, "ctrl + Q"),
        ].into_iter().map(|(cmd, key)| (cmd, Trigger::Key(key.into()))).collect())
    }
}

impl Mappings {
    fn assign(&mut self, cmd: Cmd, trigger: Trigger) {
        if let Trigger::Key(_) = trigger {
            self.0.retain(|(_, t)| *t != trigger);
        }
        if !self.0.iter().any(|(c, t)| *c == cmd && *t == trigger) {
            self.0.push((cmd, trigger));
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = (Cmd, &str)> {
        self.0.iter().filter_map(|(cmd, t)| match t {
            Trigger::Key(key) => Some((*cmd, key.as_str())),
            _ => None,
        })
    }

    pub fn menu_keys(&self, cmd: Cmd) -> String {
        self.keys().filter(|(c, _)| *c == cmd).map(|(_, k)| k).collect::<Vec<_>>().join(", ")
    }

    pub fn midi(&self, cc: u8, channel: u8, value: u8, received: Instant, pending: &mut Vec<Cmd>) {
        if value > 64 {
            let first = pending.len();
            for (cmd, trigger) in &self.0 {
                if matches!(trigger, Trigger::Midi { cc: n, channel: ch } if *n == cc && (*ch == 0 || *ch == channel)) {
                    let command = timed(*cmd, received);
                    if !pending[first..].contains(&command) {
                        pending.push(command);
                    }
                }
            }
        }
    }

    pub fn osc(&self, address: &str, args: &[Arg], received: Instant, pending: &mut Vec<Cmd>) {
        // The original indexes just the float arguments, even in a mixed-type OSC message.
        let floats = args.iter().filter_map(|a| if let Arg::F(v) = a { Some(*v) } else { None });
        for (index, value) in floats.enumerate() {
            if value > 0.5 {
                for (cmd, trigger) in &self.0 {
                    if matches!(trigger, Trigger::Osc { address: a, parameter } if a == address && *parameter == index) {
                        pending.push(timed(*cmd, received));
                    }
                }
            }
        }
    }

    fn from_xml(text: &str) -> io::Result<Self> {
        let root = El::parse(text).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
        if root.name != "Pedalboard2AppMappings" {
            return Err(io::Error::new(ErrorKind::InvalidData, "Expected a Pedalboard2AppMappings document."));
        }
        let mut maps = Self::default();
        if let Some(keys) = root.child("KEYMAPPINGS") {
            if !keys.get_b("basedOnDefaults", true) {
                maps.0.clear();
            }
            for el in &keys.children {
                let Some(cmd) = xml_command(el, 16) else { continue };
                let Some(key) = el.get("key").filter(|k| !k.trim().is_empty()) else { continue };
                let trigger = Trigger::Key(keyboard::normalize(key));
                match el.name.as_str() {
                    "MAPPING" => maps.assign(cmd, trigger),
                    "UNMAPPING" => maps.0.retain(|(c, t)| *c != cmd || *t != trigger),
                    _ => {}
                }
            }
        }
        if let Some(midi) = root.child("MidiMappings") {
            for m in midi.named("MidiAppMapping") {
                let cc = m.get_i("cc", -1);
                let channel = m.get_i("channel", 0);
                if let Some(cmd) = xml_command(m, 10) {
                    if (0..128).contains(&cc) && (0..=16).contains(&channel) {
                        maps.assign(cmd, Trigger::Midi { cc: cc as u8, channel: channel as u8 });
                    }
                }
            }
        }
        if let Some(osc) = root.child("OscMappings") {
            for m in osc.named("OscAppMapping") {
                if let (Some(cmd), Some(address), Ok(parameter)) = (
                    xml_command(m, 10), m.get("address"), usize::try_from(m.get_i("parameterIndex", 0)),
                ) {
                    maps.assign(cmd, Trigger::Osc { address: address.to_owned(), parameter });
                }
            }
        }
        Ok(maps)
    }

    fn xml(&self) -> String {
        let mut keys = El::new("KEYMAPPINGS").attr("basedOnDefaults", 0);
        let mut midi = El::new("MidiMappings");
        let mut osc = El::new("OscMappings");
        for (cmd, trigger) in &self.0 {
            let Some((_, id, name)) = commands().find(|(c, _, _)| c == cmd) else { continue };
            match trigger {
                Trigger::Key(key) => keys.children.push(
                    El::new("MAPPING").attr("commandId", format!("{id:x}")).attr("description", name).attr("key", key)
                ),
                Trigger::Midi { cc, channel } => {
                    let mut m = El::new("MidiAppMapping").attr("commandId", id).attr("cc", cc);
                    if *channel != 0 {
                        m.set("channel", channel);
                    }
                    midi.children.push(m);
                }
                Trigger::Osc { address, parameter } => osc.children.push(
                    El::new("OscAppMapping").attr("commandId", id).attr("address", address).attr("parameterIndex", parameter)
                ),
            }
        }
        El::new("Pedalboard2AppMappings").with_child(keys).with_child(midi).with_child(osc).to_document()
    }

    pub fn load(dir: &Path, legacy: &[AppMapping]) -> io::Result<Self> {
        let read = |dir: &Path| match fs::read_to_string(dir.join("AppMappings.xml")) {
            Ok(text) => Self::from_xml(&text).map(Some),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        };
        if let Some(maps) = read(dir)? {
            return Ok(maps);
        }
        let original = read(&dir.with_file_name("Pedalboard2"))?;
        let mut maps = original.unwrap_or_default();
        for old in legacy {
            let trigger = if old.osc {
                Trigger::Osc { address: old.address.clone(), parameter: 0 }
            } else {
                Trigger::Midi { cc: old.cc, channel: old.channel }
            };
            maps.assign(old.command.into(), trigger);
        }
        maps.save(dir)?;
        Ok(maps)
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let temp = dir.join(format!("AppMappings.{}.tmp", std::process::id()));
        let mut file = fs::File::create(&temp)?;
        let result = file.write_all(self.xml().as_bytes()).and_then(|_| file.sync_all());
        drop(file);
        let result = result.and_then(|_| fs::rename(&temp, dir.join("AppMappings.xml")));
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}

fn xml_command(el: &El, radix: u32) -> Option<Cmd> {
    let id = u16::from_str_radix(el.get("commandId")?, radix).ok()?;
    commands().find(|(_, n, _)| *n == id).map(|(cmd, _, _)| *cmd)
}

fn timed(cmd: Cmd, received: Instant) -> Cmd {
    if cmd == Cmd::TapTempo { Cmd::TimedTap(received) } else { cmd }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AppCommand;

    #[test]
    fn original_xml_restores_shortcuts_and_uses_decimal_ids_for_controllers() {
        let xml = r#"<Pedalboard2AppMappings>
            <KEYMAPPINGS basedOnDefaults="1">
                <UNMAPPING commandId="3" key="ctrl + S"/>
                <MAPPING commandId="10" key="Ctrl + S"/>
            </KEYMAPPINGS>
            <MidiMappings><MidiAppMapping cc="64" commandId="16"/></MidiMappings>
            <OscMappings><OscAppMapping address="/pad" parameterIndex="2" commandId="20"/></OscMappings>
        </Pedalboard2AppMappings>"#;
        let maps = Mappings::from_xml(xml).unwrap();
        assert_eq!(maps.menu_keys(Cmd::Save), "");
        assert_eq!(maps.menu_keys(Cmd::NextPatch), "ctrl + S");
        assert_eq!(maps.menu_keys(Cmd::PlayPause), "spacebar");
        assert_eq!(Mappings::from_xml(&maps.xml()).unwrap(), maps);
        let now = Instant::now();
        let mut hits = Vec::new();
        maps.midi(64, 16, 64, now, &mut hits);
        maps.midi(64, 16, 65, now, &mut hits);
        maps.osc("/pad", &[Arg::I(1), Arg::F(1.0), Arg::F(0.7), Arg::F(0.5)], now, &mut hits);
        maps.osc("/pad", &[Arg::F(0.0), Arg::I(12), Arg::F(0.0), Arg::F(1.0)], now, &mut hits);
        assert_eq!(hits, vec![Cmd::NextPatch, Cmd::TimedTap(now)]);
    }

    #[test]
    fn upgrades_keep_existing_shortcuts_and_channel_filters_then_respect_removal_and_reset() {
        let root = std::env::temp_dir().join(format!("pedalhost-mappings-{}", std::process::id()));
        let dir = root.join("PedalboardHost");
        let original = root.join("Pedalboard2");
        let mut start = Mappings::default();
        start.assign(Cmd::EventLog, Trigger::Key("F11".into()));
        start.save(&original).unwrap();
        let old = [AppMapping { osc: false, channel: 2, cc: 127, address: String::new(), command: AppCommand::PlayPause }];
        let mut maps = Mappings::load(&dir, &old).unwrap();
        assert_eq!(maps.menu_keys(Cmd::EventLog), "F11");
        let mut hits = Vec::new();
        let now = Instant::now();
        maps.midi(127, 1, 127, now, &mut hits);
        maps.midi(127, 2, 127, now, &mut hits);
        assert_eq!(hits, vec![Cmd::PlayPause]);
        maps.assign(Cmd::About, Trigger::Key("F11".into()));
        maps.0.retain(|(_, t)| !matches!(t, Trigger::Midi { .. }));
        maps.save(&dir).unwrap();
        assert_eq!(Mappings::load(&dir, &old).unwrap(), maps);
        assert_eq!(maps.menu_keys(Cmd::EventLog), "");
        Mappings::default().save(&dir).unwrap();
        assert_eq!(Mappings::load(&dir, &old).unwrap(), Mappings::default());
        assert_eq!(Mappings::from_xml(&fs::read_to_string(original.join("AppMappings.xml")).unwrap()).unwrap(), start);
        fs::remove_dir_all(root).unwrap();
    }
}
