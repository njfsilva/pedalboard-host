//! Application settings (Misc Settings keys from 0x140081dc0 / 0x140085980), stored as JSON.

use crate::plugins::PluginInfo;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCommand {
    NextPatch,
    PreviousPatch,
    PlayPause,
    ReturnToZero,
    TapTempo,
}

/// Older rebuilds stored five application commands in settings.json.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AppMapping {
    pub osc: bool,
    pub cc: u8,
    pub channel: u8,
    pub address: String,
    pub command: AppCommand,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AudioConfig {
    /// Output system. Older settings use it for both directions.
    pub backend: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub input_backend: String,
    pub device: String,
    pub input_device: String,
    pub sample_rate: f64,
    pub buffer_size: u32,
    pub input_channels: Vec<u32>,
    pub output_channels: Vec<u32>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig {
            backend: "Windows Audio".into(),
            input_backend: String::new(),
            device: String::new(),
            input_device: String::new(),
            sample_rate: 0.0,
            buffer_size: 0,
            input_channels: vec![0, 1],
            output_channels: vec![0, 1],
        }
    }
}

impl AudioConfig {
    pub fn output_asio(&self) -> bool {
        self.backend == "ASIO"
    }

    pub fn input_asio(&self) -> bool {
        if self.input_backend.is_empty() { self.output_asio() } else { self.input_backend == "ASIO" }
    }

    pub fn set_systems(&mut self, input_asio: bool, output_asio: bool) {
        let old_input = self.input_asio();
        let old_output = self.output_asio();
        if old_input && old_output && input_asio && !output_asio {
            self.input_device = self.device.clone();
        } else if old_input != input_asio {
            self.input_device.clear();
        }
        if old_output != output_asio {
            self.device = if old_input && input_asio && output_asio {
                self.input_device.clone()
            } else {
                String::new()
            };
        }
        if input_asio && output_asio {
            self.input_device.clear();
        }
        self.backend = if output_asio { "ASIO" } else { "Windows Audio" }.into();
        self.input_backend = if input_asio == output_asio { "" } else if input_asio { "ASIO" } else { "Windows Audio" }.into();
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub loop_patches: bool,
    pub windows_on_top: bool,
    pub ignore_pin_names: bool,
    pub midi_program_change: bool,
    pub mmc_transport: bool,
    // Old settings saved false even when the tray preference had never been touched.
    #[serde(default)]
    tray_settings_version: u8,
    pub use_tray_icon: bool,
    pub start_in_tray: bool,
    pub fixed_size_windows: bool,
    pub pdl_audio_settings: bool,
    pub auto_mappings_window: bool,
    pub osc_port: u16,
    /// Empty for a one-to-one connection.
    pub osc_multicast: String,
    pub audio_input: bool,
    pub midi_input: bool,
    pub osc_input: bool,
    pub audio: AudioConfig,
    pub midi_inputs: Vec<String>,
    pub plugins: Vec<PluginInfo>,
    pub vst2_folders: Vec<String>,
    pub vst3_folders: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub app_mappings: Vec<AppMapping>,
    pub colour_scheme: String,
    pub last_file: String,
}

impl Default for Settings {
    /// The original defaults, with the tray icon enabled for the rebuilt host.
    fn default() -> Self {
        Settings {
            loop_patches: true,
            windows_on_top: false,
            ignore_pin_names: false,
            midi_program_change: false,
            mmc_transport: false,
            tray_settings_version: 1,
            use_tray_icon: true,
            start_in_tray: false,
            fixed_size_windows: true,
            pdl_audio_settings: false,
            auto_mappings_window: true,
            osc_port: 5678,
            osc_multicast: String::new(),
            audio_input: true,
            midi_input: true,
            osc_input: true,
            audio: AudioConfig::default(),
            midi_inputs: Vec::new(),
            plugins: Vec::new(),
            vst2_folders: vec![
                r"C:\Program Files\VSTPlugins".into(),
                r"C:\Program Files\Steinberg\VSTPlugins".into(),
                r"C:\Program Files\Common Files\VST2".into(),
                r"C:\Program Files\Common Files\Steinberg\VST2".into(),
            ],
            vst3_folders: vec![r"C:\Program Files\Common Files\VST3".into()],
            app_mappings: Vec::new(),
            colour_scheme: "default".into(),
            last_file: String::new(),
        }
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("PedalboardHost")
}

impl Settings {
    fn from_json(text: &str) -> serde_json::Result<Self> {
        let document: serde_json::Value = serde_json::from_str(text)?;
        let old_icon_enabled = document.get("use_tray_icon").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut settings: Self = serde_json::from_value(document)?;
        if settings.tray_settings_version == 0 {
            settings.start_in_tray &= old_icon_enabled;
            settings.use_tray_icon = true;
            settings.tray_settings_version = 1;
        }
        Ok(settings)
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(dir().join("settings.json"))
            .ok()
            .and_then(|t| Self::from_json(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let d = dir();
        let _ = std::fs::create_dir_all(&d);
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(d.join("settings.json"), t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_enables_the_icon_once_and_remembers_later_choices() {
        let mut settings = Settings::from_json(r#"{"use_tray_icon":false,"start_in_tray":true,"osc_port":7777}"#).unwrap();
        assert!(settings.use_tray_icon && !settings.start_in_tray);
        assert_eq!(settings.osc_port, 7777);
        settings.use_tray_icon = false;
        assert!(!Settings::from_json(&serde_json::to_string(&settings).unwrap()).unwrap().use_tray_icon);
        let hidden = Settings::from_json(r#"{"use_tray_icon":true,"start_in_tray":true}"#).unwrap();
        assert!(hidden.use_tray_icon && hidden.start_in_tray);
        assert!(!Settings::from_json(r#"{"start_in_tray":true}"#).unwrap().start_in_tray);
        assert!(Settings::default().use_tray_icon);
        assert!(Settings::from_json("{}").unwrap().use_tray_icon);
    }
}
