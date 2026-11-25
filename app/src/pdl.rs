//! A small XML element tree (JUCE XmlElement style), JUCE's base64 variant and
//! `copyXmlToBinary`, used for .pdl patch files, colour schemes and plugin state.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct El {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<El>,
    pub text: String,
}

impl El {
    pub fn new(name: &str) -> El {
        El { name: name.to_string(), ..Default::default() }
    }

    pub fn attr(mut self, k: &str, v: impl ToString) -> El {
        self.set(k, v);
        self
    }

    pub fn with_child(mut self, c: El) -> El {
        self.children.push(c);
        self
    }

    pub fn set(&mut self, k: &str, v: impl ToString) {
        let v = v.to_string();
        match self.attrs.iter_mut().find(|(n, _)| n == k) {
            Some(a) => a.1 = v,
            None => self.attrs.push((k.to_string(), v)),
        }
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }

    pub fn get_f(&self, k: &str, d: f64) -> f64 {
        self.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d)
    }

    pub fn get_i(&self, k: &str, d: i64) -> i64 {
        self.get(k).and_then(|v| v.trim().parse::<f64>().ok()).map(|v| v as i64).unwrap_or(d)
    }

    pub fn get_b(&self, k: &str, d: bool) -> bool {
        match self.get(k) {
            Some(v) => matches!(v.trim(), "1" | "true" | "TRUE" | "True"),
            None => d,
        }
    }

    pub fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Every descendant (depth first) with this tag.
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a El>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.find_all(name, out);
        }
    }

    pub fn parse(text: &str) -> Result<El, String> {
        let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
        fn conv(n: roxmltree::Node) -> El {
            let mut e = El::new(n.tag_name().name());
            for a in n.attributes() {
                e.attrs.push((a.name().to_string(), a.value().to_string()));
            }
            for c in n.children() {
                if c.is_element() {
                    e.children.push(conv(c));
                } else if c.is_text() {
                    e.text.push_str(c.text().unwrap_or(""));
                }
            }
            e
        }
        Ok(conv(doc.root_element()))
    }

    pub fn to_document(&self) -> String {
        let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\n");
        self.write(&mut s, 0, true);
        s
    }

    pub fn to_single_line(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, 0, false);
        s
    }

    fn write(&self, s: &mut String, depth: usize, pretty: bool) {
        let pad = if pretty { "  ".repeat(depth) } else { String::new() };
        s.push_str(&pad);
        s.push('<');
        s.push_str(&self.name);
        for (k, v) in &self.attrs {
            s.push(' ');
            s.push_str(k);
            s.push_str("=\"");
            escape(v, s);
            s.push('"');
        }
        let text = self.text.trim();
        if self.children.is_empty() && text.is_empty() {
            s.push_str("/>");
        } else if self.children.is_empty() {
            s.push('>');
            escape(text, s);
            s.push_str("</");
            s.push_str(&self.name);
            s.push('>');
        } else {
            s.push('>');
            for c in &self.children {
                if pretty {
                    s.push('\n');
                }
                c.write(s, depth + 1, pretty);
            }
            if pretty {
                s.push('\n');
                s.push_str(&pad);
            }
            s.push_str("</");
            s.push_str(&self.name);
            s.push('>');
        }
        if depth == 0 && pretty {
            s.push('\n');
        }
    }
}

fn escape(v: &str, s: &mut String) {
    for ch in v.chars() {
        match ch {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '"' => s.push_str("&quot;"),
            '\'' => s.push_str("&apos;"),
            c if (c as u32) < 32 => s.push_str(&format!("&#{};", c as u32)),
            c => s.push(c),
        }
    }
}

const B64: &[u8; 64] = b".ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+";

/// JUCE `MemoryBlock::toBase64Encoding`: "<size>." followed by 6-bit little-endian bit groups.
pub fn b64_encode(data: &[u8]) -> String {
    let mut s = format!("{}.", data.len());
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in data {
        acc |= (b as u32) << bits;
        bits += 8;
        while bits >= 6 {
            s.push(B64[(acc & 63) as usize] as char);
            acc >>= 6;
            bits -= 6;
        }
    }
    if bits > 0 {
        s.push(B64[(acc & 63) as usize] as char);
    }
    s
}

pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    let dot = text.find('.')?;
    let size: usize = text[..dot].trim().parse().ok()?;
    let mut out = Vec::with_capacity(size);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in text[dot + 1..].bytes() {
        let Some(v) = B64.iter().position(|&c| c == ch) else { continue };
        acc |= (v as u32) << bits;
        bits += 6;
        while bits >= 8 {
            if out.len() < size {
                out.push(acc as u8);
            }
            acc >>= 8;
            bits -= 8;
        }
    }
    out.resize(size, 0);
    Some(out)
}

const XML_MAGIC: u32 = 0x21324356;

/// JUCE `AudioProcessor::copyXmlToBinary`.
pub fn xml_to_binary(e: &El) -> Vec<u8> {
    let text = e.to_single_line();
    let mut v = Vec::with_capacity(text.len() + 9);
    v.extend_from_slice(&XML_MAGIC.to_le_bytes());
    v.extend_from_slice(&(text.len() as u32).to_le_bytes());
    v.extend_from_slice(text.as_bytes());
    v.push(0);
    v
}

pub fn xml_from_binary(data: &[u8]) -> Option<El> {
    if data.len() < 8 || u32::from_le_bytes(data[0..4].try_into().ok()?) != XML_MAGIC {
        return None;
    }
    let len = (u32::from_le_bytes(data[4..8].try_into().ok()?) as usize).min(data.len() - 8);
    let text = String::from_utf8_lossy(&data[8..8 + len]);
    El::parse(text.trim_end_matches('\0')).ok()
}
