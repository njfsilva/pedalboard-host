//! Named colours of the original "Colour Schemes" dialog (defaults from 0x140026970).

use crate::pdl::El;
use egui::Color32;

pub const NAMES: [(&str, u32); 22] = [
    ("Window Background", 0xFFEEECE1),
    ("Field Background", 0xFFF3F2EB),
    ("Text Colour", 0xFF000000),
    ("Plugin Border", 0xFFB0B0FF),
    ("Plugin Background", 0xFFFFFFFF),
    ("Button Colour", 0xFFEEECE1),
    ("Button Highlight", 0xB0B0B0FF),
    ("Text Editor Colour", 0xFFFFFFFF),
    ("Dialog Inner Background", 0xFFFFFFFF),
    ("CPU Meter Colour", 0xB0B0B0FF),
    ("Slider Colour", 0xFF9A9181),
    ("List Selected Colour", 0xFFB0B0FF),
    ("VU Meter Lower Colour", 0x7F00BF00),
    ("VU Meter Upper Colour", 0x7FFFFF00),
    ("VU Meter Over Colour", 0x7FFF0000),
    ("Vector Colour", 0x80000000),
    ("Menu Selection Colour", 0x40000000),
    ("Waveform Colour", 0xFFB0B0FF),
    ("Level Dial Colour", 0xFF8C8CCC),
    ("Tick Box Colour", 0x809A9181),
    ("Audio Connection", 0xFF8C8CCC),
    ("Parameter Connection", 0xFFCCA88F),
];

pub const WINDOW_BG: usize = 0;
pub const FIELD_BG: usize = 1;
pub const TEXT: usize = 2;
pub const PLUGIN_BORDER: usize = 3;
pub const PLUGIN_BG: usize = 4;
pub const BUTTON: usize = 5;
pub const BUTTON_HIGHLIGHT: usize = 6;
pub const TEXT_EDITOR: usize = 7;
pub const DIALOG_INNER: usize = 8;
pub const CPU_METER: usize = 9;
pub const SLIDER: usize = 10;
pub const LIST_SELECTED: usize = 11;
pub const VU_LOWER: usize = 12;
pub const VU_UPPER: usize = 13;
pub const VU_OVER: usize = 14;
pub const VECTOR: usize = 15;
pub const MENU_SELECTION: usize = 16;
pub const WAVEFORM: usize = 17;
pub const LEVEL_DIAL: usize = 18;
pub const TICK_BOX: usize = 19;
pub const AUDIO_CONNECTION: usize = 20;
pub const PARAM_CONNECTION: usize = 21;

pub fn argb(v: u32) -> Color32 {
    Color32::from_rgba_unmultiplied((v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8)
}

pub fn to_argb(c: Color32) -> u32 {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// JUCE's `PixelARGB::premultiply`: see-through colours are premultiplied in sRGB values and
/// blended that way. egui's own conversion premultiplies in linear light, which draws the
/// half-transparent defaults (Tick Box, Button Highlight, the meters) much paler than the
/// original does.
fn premultiply(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    if a == 255 {
        return Color32::from_rgb(r, g, b);
    }
    let f = |v: u8| ((v as u32 * a as u32 + 0x7f) >> 8) as u8;
    Color32::from_rgba_premultiplied(f(r), f(g), f(b), a)
}

/// The straight sRGB channels of a colour premultiplied as `premultiply` does.
fn unpremultiply(c: Color32) -> [u8; 4] {
    let a = c.a() as u32;
    if a == 0 || a == 255 {
        return [c.r(), c.g(), c.b(), c.a()];
    }
    let f = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as u8;
    [f(c.r()), f(c.g()), f(c.b()), c.a()]
}

/// JUCE `Colour::brighter`: each channel moves towards 255 by 1/(1+amount), truncating.
pub fn brighter(c: Color32, amount: f32) -> Color32 {
    let [r, g, b, a] = unpremultiply(c);
    let f = |v: u8| (255.0 - (255.0 - v as f32) / (1.0 + amount)) as u8;
    premultiply(f(r), f(g), f(b), a)
}

pub fn darker(c: Color32, amount: f32) -> Color32 {
    let [r, g, b, a] = unpremultiply(c);
    let f = |v: u8| (v as f32 / (1.0 + amount)) as u8;
    premultiply(f(r), f(g), f(b), a)
}

/// A straight (unpremultiplied) colour ready to draw, premultiplied as JUCE does.
pub fn draw(c: [u8; 4]) -> Color32 {
    premultiply(c[0], c[1], c[2], c[3])
}

/// JUCE `Colour::withAlpha (float)`.
pub fn with_alpha(c: [u8; 4], a: f32) -> [u8; 4] {
    [c[0], c[1], c[2], (a.clamp(0.0, 1.0) * 255.0).round() as u8]
}

/// JUCE `Colour::withMultipliedAlpha`.
pub fn mult_alpha(c: [u8; 4], m: f32) -> [u8; 4] {
    [c[0], c[1], c[2], (c[3] as f32 * m).round().clamp(0.0, 255.0) as u8]
}

/// JUCE `Colour::overlaidWith`: `src` laid over `dst`, both straight colours.
pub fn overlaid(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let da = dst[3] as i32;
    if da == 0 {
        return src;
    }
    let inv = 255 - src[3] as i32;
    let ra = 255 - (((255 - da) * inv) >> 8);
    if ra <= 0 {
        return dst;
    }
    let k = inv * da / ra;
    let ch = |d: u8, s: u8| (s as i32 + (((d as i32 - s as i32) * k) >> 8)).clamp(0, 255) as u8;
    [ch(dst[0], src[0]), ch(dst[1], src[1]), ch(dst[2], src[2]), ra as u8]
}

/// JUCE `Colour::withMultipliedBrightness`: the HSB brightness scaled (at most 1), hue and
/// saturation kept, which scales every channel alike.
pub fn mult_brightness(c: [u8; 4], m: f32) -> [u8; 4] {
    let v = c[0].max(c[1]).max(c[2]) as f32;
    if v <= 0.0 {
        return c;
    }
    let k = (v * m.max(0.0)).min(255.0) / v;
    let f = |x: u8| (x as f32 * k).round().clamp(0.0, 255.0) as u8;
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

/// Premultiplied `src` drawn over premultiplied `dst`.
pub fn over(src: Color32, dst: Color32) -> Color32 {
    let k = 255 - src.a() as u32;
    let f = |s: u8, d: u8| (s as u32 + (d as u32 * k + 127) / 255).min(255) as u8;
    Color32::from_rgba_premultiplied(f(src.r(), dst.r()), f(src.g(), dst.g()), f(src.b(), dst.b()), f(src.a(), dst.a()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scheme {
    pub name: String,
    pub colours: Vec<Color32>,
}

impl Default for Scheme {
    fn default() -> Self {
        Scheme { name: "default".into(), colours: NAMES.iter().map(|(_, v)| argb(*v)).collect() }
    }
}

impl Scheme {
    /// A colour ready to draw: stored as the colour dialog edits it, premultiplied as JUCE does.
    pub fn get(&self, idx: usize) -> Color32 {
        let c = self.stored(idx);
        if c.a() == 255 || c.a() == 0 {
            return c;
        }
        let [r, g, b, a] = c.to_srgba_unmultiplied();
        premultiply(r, g, b, a)
    }

    fn stored(&self, idx: usize) -> Color32 {
        self.colours.get(idx).copied().unwrap_or(Color32::BLACK)
    }

    /// The colour's straight sRGB channels, as JUCE's `Colour` holds them before drawing.
    pub fn straight(&self, idx: usize) -> [u8; 4] {
        self.stored(idx).to_srgba_unmultiplied()
    }

    pub fn to_xml(&self) -> String {
        let mut root = El::new("Pedalboard2ColourScheme");
        for (i, (name, _)) in NAMES.iter().enumerate() {
            root.children.push(El::new("Colour").attr("name", name).attr("value", format!("{:08x}", to_argb(self.stored(i)))));
        }
        root.to_document()
    }

    pub fn from_xml(text: &str, name: &str) -> Option<Scheme> {
        let root = El::parse(text).ok()?;
        if root.name != "Pedalboard2ColourScheme" {
            return None;
        }
        let mut s = Scheme { name: name.to_string(), ..Default::default() };
        for c in root.children.iter().filter(|c| c.name.eq_ignore_ascii_case("colour")) {
            let (Some(n), Some(v)) = (c.get("name"), c.get("value")) else { continue };
            if let (Some(i), Ok(val)) = (NAMES.iter().position(|(k, _)| *k == n), u32::from_str_radix(v.trim(), 16)) {
                s.colours[i] = argb(val);
            }
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn see_through_colours_draw_like_juce_and_save_unchanged() {
        let s = Scheme::default();
        // Tick Box 0x809A9181 is half of each channel, so over white it draws as 0xCCC8C0.
        let t = s.get(TICK_BOX);
        assert_eq!([t.r(), t.g(), t.b(), t.a()], [77, 72, 64, 128]);
        assert_eq!(s.get(WINDOW_BG), Color32::from_rgb(0xEE, 0xEC, 0xE1));
        assert!(s.to_xml().contains("809a9181"));
    }

    #[test]
    fn overlaid_colours_match_juce_glass_sphere_shades() {
        // drawGlassSphere: white overlaid with the Tick Box colour at 0.3 alpha, then at full.
        let tick = Scheme::default().straight(TICK_BOX);
        assert_eq!(overlaid([255; 4], mult_alpha(tick, 0.3)), [239, 238, 235, 255]);
        assert_eq!(overlaid([255; 4], tick), [204, 199, 191, 255]);
        assert_eq!(overlaid([0, 0, 0, 0], tick), tick, "nothing underneath gives the top colour");
    }
}
