// BMS (Basic Mapping Support) Screen Engine.
// Provides structured screen I/O for CICS SEND MAP / RECEIVE MAP.
// Maps field definitions to a 3270-compatible screen model.

use std::collections::HashMap;

// ── DFHBMSCA — BMS Character Attributes ─────────────────────────────

pub mod dfhbmsca {
    pub const UNPROT: u8       = 0x00;  // Unprotected
    pub const UNPROT_NUM: u8   = 0x10;  // Unprotected Numeric
    pub const UNPROT_BRT: u8   = 0x08;  // Unprotected Bright
    pub const UNPROT_DRK: u8   = 0x0C;  // Unprotected Dark
    pub const PROT: u8         = 0x20;  // Protected
    pub const PROT_BRT: u8     = 0x28;  // Protected Bright
    pub const PROT_DRK: u8     = 0x2C;  // Protected Dark
    pub const ASKIP: u8        = 0x30;  // Autoskip
    pub const ASKIP_BRT: u8    = 0x38;  // Autoskip Bright
    pub const ASKIP_DRK: u8    = 0x3C;  // Autoskip Dark
    pub const UNPROT_MDT: u8   = 0x01;  // Unprotected + Modified Data Tag
    pub const UNPROT_NUM_MDT: u8 = 0x11; // Unprotected Numeric + MDT
    pub const UNPROT_BRT_MDT: u8 = 0x09; // Unprotected Bright + MDT

    pub fn is_protected(attr: u8) -> bool { attr & 0x20 != 0 }
    pub fn is_numeric(attr: u8) -> bool { attr & 0x10 != 0 }
    pub fn is_bright(attr: u8) -> bool { attr & 0x08 != 0 && attr & 0x04 == 0 }
    pub fn is_dark(attr: u8) -> bool { attr & 0x0C == 0x0C }
    pub fn is_autoskip(attr: u8) -> bool { attr & 0x30 == 0x30 }
    pub fn is_mdt(attr: u8) -> bool { attr & 0x01 != 0 }
}

// ── DFHAID — Attention Identifiers ──────────────────────────────────

pub mod dfhaid {
    pub const ENTER: u8  = 0x7D;
    pub const CLEAR: u8  = 0x6D;
    pub const PA1: u8    = 0x6C;
    pub const PA2: u8    = 0x6E;
    pub const PA3: u8    = 0x6B;
    pub const PF1: u8    = 0xF1;
    pub const PF2: u8    = 0xF2;
    pub const PF3: u8    = 0xF3;
    pub const PF4: u8    = 0xF4;
    pub const PF5: u8    = 0xF5;
    pub const PF6: u8    = 0xF6;
    pub const PF7: u8    = 0xF7;
    pub const PF8: u8    = 0xF8;
    pub const PF9: u8    = 0xF9;
    pub const PF10: u8   = 0x7A;
    pub const PF11: u8   = 0x7B;
    pub const PF12: u8   = 0x7C;
    pub const PF13: u8   = 0xC1;
    pub const PF14: u8   = 0xC2;
    pub const PF15: u8   = 0xC3;
    pub const PF16: u8   = 0xC4;
    pub const PF17: u8   = 0xC5;
    pub const PF18: u8   = 0xC6;
    pub const PF19: u8   = 0xC7;
    pub const PF20: u8   = 0xC8;
    pub const PF21: u8   = 0xC9;
    pub const PF22: u8   = 0x4A;
    pub const PF23: u8   = 0x4B;
    pub const PF24: u8   = 0x4C;

    pub fn name(aid: u8) -> &'static str {
        match aid {
            ENTER => "ENTER", CLEAR => "CLEAR",
            PA1 => "PA1", PA2 => "PA2", PA3 => "PA3",
            PF1 => "PF1", PF2 => "PF2", PF3 => "PF3",
            PF4 => "PF4", PF5 => "PF5", PF6 => "PF6",
            PF7 => "PF7", PF8 => "PF8", PF9 => "PF9",
            PF10 => "PF10", PF11 => "PF11", PF12 => "PF12",
            PF13 => "PF13", PF14 => "PF14", PF15 => "PF15",
            PF16 => "PF16", PF17 => "PF17", PF18 => "PF18",
            PF19 => "PF19", PF20 => "PF20", PF21 => "PF21",
            PF22 => "PF22", PF23 => "PF23", PF24 => "PF24",
            _ => "UNKNOWN",
        }
    }

    pub fn from_name(name: &str) -> u8 {
        match name.to_uppercase().as_str() {
            "ENTER" => ENTER, "CLEAR" => CLEAR,
            "PA1" => PA1, "PA2" => PA2, "PA3" => PA3,
            "PF1" => PF1, "PF2" => PF2, "PF3" => PF3,
            "PF4" => PF4, "PF5" => PF5, "PF6" => PF6,
            "PF7" => PF7, "PF8" => PF8, "PF9" => PF9,
            "PF10" => PF10, "PF11" => PF11, "PF12" => PF12,
            "PF13" => PF13, "PF14" => PF14, "PF15" => PF15,
            "PF16" => PF16, "PF17" => PF17, "PF18" => PF18,
            "PF19" => PF19, "PF20" => PF20, "PF21" => PF21,
            "PF22" => PF22, "PF23" => PF23, "PF24" => PF24,
            _ => 0,
        }
    }
}

// ── BMS Color ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BmsColor {
    Default, Blue, Red, Pink, Green, Turquoise, Yellow, White,
}

impl BmsColor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default", Self::Blue => "blue",
            Self::Red => "red", Self::Pink => "pink",
            Self::Green => "green", Self::Turquoise => "turquoise",
            Self::Yellow => "yellow", Self::White => "white",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "BLUE" => Self::Blue, "RED" => Self::Red,
            "PINK" => Self::Pink, "GREEN" => Self::Green,
            "TURQUOISE" | "TURQ" => Self::Turquoise,
            "YELLOW" => Self::Yellow, "WHITE" => Self::White,
            _ => Self::Default,
        }
    }
}

// ── BMS Highlight ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BmsHighlight {
    Off, Blink, Reverse, Underline,
}

// ── BMS Field ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BmsField {
    pub name: String,
    pub row: u16,
    pub col: u16,
    pub length: u16,
    pub initial: String,
    pub attr: u8,
    pub color: BmsColor,
    pub highlight: BmsHighlight,
    pub pic_in: String,
    pub pic_out: String,
}

impl BmsField {
    pub fn new(name: &str, row: u16, col: u16, length: u16) -> Self {
        Self {
            name: name.to_uppercase(), row, col, length,
            initial: String::new(), attr: dfhbmsca::UNPROT,
            color: BmsColor::Green, highlight: BmsHighlight::Off,
            pic_in: String::new(), pic_out: String::new(),
        }
    }

    pub fn with_attr(mut self, attr: u8) -> Self { self.attr = attr; self }
    pub fn with_initial(mut self, s: &str) -> Self { self.initial = s.to_string(); self }
    pub fn with_color(mut self, c: BmsColor) -> Self { self.color = c; self }
    pub fn with_highlight(mut self, h: BmsHighlight) -> Self { self.highlight = h; self }

    pub fn is_protected(&self) -> bool { dfhbmsca::is_protected(self.attr) }
    pub fn is_numeric(&self) -> bool { dfhbmsca::is_numeric(self.attr) }
    pub fn is_bright(&self) -> bool { dfhbmsca::is_bright(self.attr) }
    pub fn is_dark(&self) -> bool { dfhbmsca::is_dark(self.attr) }
}

// ── BMS Map / Mapset ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BmsMap {
    pub name: String,
    pub rows: u16,
    pub cols: u16,
    pub fields: Vec<BmsField>,
}

impl BmsMap {
    pub fn new(name: &str, rows: u16, cols: u16) -> Self {
        Self { name: name.to_uppercase(), rows, cols, fields: Vec::new() }
    }

    pub fn add_field(&mut self, field: BmsField) {
        self.fields.push(field);
    }

    pub fn get_field(&self, name: &str) -> Option<&BmsField> {
        let uname = name.to_uppercase();
        self.fields.iter().find(|f| f.name == uname)
    }
}

#[derive(Debug, Clone)]
pub struct BmsMapset {
    pub name: String,
    pub maps: HashMap<String, BmsMap>,
}

impl BmsMapset {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_uppercase(), maps: HashMap::new() }
    }

    pub fn add_map(&mut self, map: BmsMap) {
        self.maps.insert(map.name.clone(), map);
    }

    pub fn get_map(&self, name: &str) -> Option<&BmsMap> {
        self.maps.get(&name.to_uppercase())
    }
}

// ── Screen Output (SEND MAP result) ─────────────────────────────────

#[derive(Debug, Clone)]
pub struct ScreenFieldOutput {
    pub name: String,
    pub row: u16,
    pub col: u16,
    pub length: u16,
    pub value: String,
    pub protected: bool,
    pub numeric: bool,
    pub bright: bool,
    pub dark: bool,
    pub color: String,
}

#[derive(Debug, Clone)]
pub struct ScreenOutput {
    pub map: String,
    pub mapset: String,
    pub fields: Vec<ScreenFieldOutput>,
    pub cursor_field: Option<String>,
    pub cursor_pos: Option<(u16, u16)>,
    pub erase: bool,
    pub alarm: bool,
}

impl ScreenOutput {
    /// Render the screen to a text representation (for stdio mode).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("--- MAP: {} MAPSET: {} ---\n", self.map, self.mapset));
        for f in &self.fields {
            if !f.value.is_empty() {
                out.push_str(&format!("  {}: {}\n", f.name, f.value));
            }
        }
        out.push_str("---\n");
        out
    }

    /// Build from a BMS map definition + runtime field data.
    pub fn from_map(map: &BmsMap, mapset: &str, data: &HashMap<String, String>,
                    erase: bool, cursor: Option<&str>) -> Self {
        let fields = map.fields.iter().map(|f| {
            let value = data.get(&f.name)
                .cloned()
                .unwrap_or_else(|| f.initial.clone());
            ScreenFieldOutput {
                name: f.name.clone(), row: f.row, col: f.col, length: f.length,
                value, protected: f.is_protected(), numeric: f.is_numeric(),
                bright: f.is_bright(), dark: f.is_dark(),
                color: f.color.as_str().to_string(),
            }
        }).collect();
        Self {
            map: map.name.clone(), mapset: mapset.to_uppercase(), fields,
            cursor_field: cursor.map(|s| s.to_uppercase()),
            cursor_pos: None, erase, alarm: false,
        }
    }
}

// ── Screen Input (RECEIVE MAP result) ───────────────────────────────

#[derive(Debug, Clone)]
pub struct ScreenInput {
    pub aid: u8,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub fields: HashMap<String, String>,
}

impl Default for ScreenInput {
    fn default() -> Self {
        Self {
            aid: dfhaid::ENTER,
            cursor_row: 0, cursor_col: 0,
            fields: HashMap::new(),
        }
    }
}

// ── Screen Channel (pluggable I/O) ──────────────────────────────────

pub trait ScreenChannel: Send {
    fn send_screen(&mut self, output: &ScreenOutput) -> Result<(), String>;
    fn receive_screen(&mut self) -> Result<ScreenInput, String>;
}

/// Stdio screen channel — text-based I/O (backward compat).
pub struct StdioScreenChannel {
    output: Box<dyn std::io::Write + Send>,
    input: Box<dyn std::io::BufRead + Send>,
}

impl StdioScreenChannel {
    pub fn new(output: Box<dyn std::io::Write + Send>,
               input: Box<dyn std::io::BufRead + Send>) -> Self {
        Self { output, input }
    }
}

impl ScreenChannel for StdioScreenChannel {
    fn send_screen(&mut self, screen: &ScreenOutput) -> Result<(), String> {
        if screen.erase {
            let _ = self.output.write_all(b"\x1B[2J\x1B[H");
        }
        let _ = self.output.write_all(screen.to_text().as_bytes());
        let _ = self.output.flush();
        Ok(())
    }

    fn receive_screen(&mut self) -> Result<ScreenInput, String> {
        let mut line = String::new();
        self.input.read_line(&mut line).map_err(|e| e.to_string())?;
        let mut input = ScreenInput::default();
        // Parse "AID=ENTER,FIELD1=val1,FIELD2=val2" format
        for pair in line.trim().split(',') {
            if let Some((k, v)) = pair.split_once('=') {
                let key = k.trim().to_uppercase();
                let val = v.trim().to_string();
                if key == "AID" {
                    input.aid = dfhaid::from_name(&val);
                } else {
                    input.fields.insert(key, val);
                }
            }
        }
        Ok(input)
    }
}

// ── BMS Map Registry ────────────────────────────────────────────────

pub struct BmsRegistry {
    mapsets: HashMap<String, BmsMapset>,
}

impl BmsRegistry {
    pub fn new() -> Self {
        Self { mapsets: HashMap::new() }
    }

    pub fn register_mapset(&mut self, mapset: BmsMapset) {
        self.mapsets.insert(mapset.name.clone(), mapset);
    }

    pub fn get_map(&self, map: &str, mapset: &str) -> Option<&BmsMap> {
        self.mapsets.get(&mapset.to_uppercase())?.get_map(map)
    }

    /// Build a ScreenOutput from registered map + runtime data.
    pub fn build_screen(&self, map: &str, mapset: &str,
                        data: &HashMap<String, String>, erase: bool,
                        cursor: Option<&str>) -> Option<ScreenOutput> {
        let bms_map = self.get_map(map, mapset)?;
        Some(ScreenOutput::from_map(bms_map, mapset, data, erase, cursor))
    }
}

impl Default for BmsRegistry {
    fn default() -> Self { Self::new() }
}

// ── JSON Serialization (behind web feature) ─────────────────────────

// JSON support requires serde — uncomment when web feature is available:
// #[cfg(feature = "web")]
// mod json_support { ... }

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dfhbmsca_attribute_flags() {
        assert!(dfhbmsca::is_protected(dfhbmsca::PROT));
        assert!(!dfhbmsca::is_protected(dfhbmsca::UNPROT));
        assert!(dfhbmsca::is_numeric(dfhbmsca::UNPROT_NUM));
        assert!(dfhbmsca::is_bright(dfhbmsca::PROT_BRT));
        assert!(dfhbmsca::is_dark(dfhbmsca::PROT_DRK));
        assert!(dfhbmsca::is_autoskip(dfhbmsca::ASKIP));
        assert!(dfhbmsca::is_mdt(dfhbmsca::UNPROT_MDT));
    }

    #[test]
    fn dfhaid_name_roundtrip() {
        assert_eq!(dfhaid::name(dfhaid::ENTER), "ENTER");
        assert_eq!(dfhaid::name(dfhaid::PF3), "PF3");
        assert_eq!(dfhaid::name(dfhaid::PF24), "PF24");
        assert_eq!(dfhaid::from_name("ENTER"), dfhaid::ENTER);
        assert_eq!(dfhaid::from_name("pf12"), dfhaid::PF12);
    }

    #[test]
    fn bms_field_builder() {
        let f = BmsField::new("USERID", 5, 20, 8)
            .with_attr(dfhbmsca::UNPROT_BRT)
            .with_color(BmsColor::Green)
            .with_initial("________");
        assert_eq!(f.name, "USERID");
        assert!(!f.is_protected());
        assert!(f.is_bright());
        assert_eq!(f.length, 8);
    }

    #[test]
    fn bms_map_fields() {
        let mut map = BmsMap::new("SIGNON", 24, 80);
        map.add_field(BmsField::new("USERID", 10, 20, 8));
        map.add_field(BmsField::new("PASSWD", 12, 20, 8).with_attr(dfhbmsca::UNPROT_DRK));
        assert!(map.get_field("USERID").is_some());
        assert!(map.get_field("passwd").is_some());
        assert!(map.get_field("NOPE").is_none());
    }

    #[test]
    fn bms_mapset() {
        let mut mapset = BmsMapset::new("CSIGNON");
        let mut map = BmsMap::new("SIGNON", 24, 80);
        map.add_field(BmsField::new("MSG", 20, 1, 79));
        mapset.add_map(map);
        assert!(mapset.get_map("SIGNON").is_some());
        assert!(mapset.get_map("NOPE").is_none());
    }

    #[test]
    fn screen_output_from_map() {
        let mut map = BmsMap::new("MENU", 24, 80);
        map.add_field(BmsField::new("TITLE", 1, 1, 30).with_initial("MAIN MENU"));
        map.add_field(BmsField::new("OPT", 10, 20, 2));

        let mut data = HashMap::new();
        data.insert("OPT".to_string(), "01".to_string());

        let screen = ScreenOutput::from_map(&map, "CMENU", &data, true, Some("OPT"));
        assert_eq!(screen.map, "MENU");
        assert_eq!(screen.mapset, "CMENU");
        assert_eq!(screen.fields.len(), 2);
        assert_eq!(screen.fields[0].value, "MAIN MENU"); // from initial
        assert_eq!(screen.fields[1].value, "01"); // from data
        assert_eq!(screen.cursor_field, Some("OPT".to_string()));
        assert!(screen.erase);
    }

    #[test]
    fn screen_output_to_text() {
        let screen = ScreenOutput {
            map: "MAP1".into(), mapset: "MS1".into(),
            fields: vec![
                ScreenFieldOutput {
                    name: "F1".into(), row: 1, col: 1, length: 10,
                    value: "hello".into(), protected: true, numeric: false,
                    bright: false, dark: false, color: "green".into(),
                },
            ],
            cursor_field: None, cursor_pos: None, erase: false, alarm: false,
        };
        let text = screen.to_text();
        assert!(text.contains("MAP: MAP1"));
        assert!(text.contains("F1: hello"));
    }

    #[test]
    fn screen_input_default() {
        let input = ScreenInput::default();
        assert_eq!(input.aid, dfhaid::ENTER);
        assert!(input.fields.is_empty());
    }

    #[test]
    fn bms_registry_lookup() {
        let mut reg = BmsRegistry::new();
        let mut mapset = BmsMapset::new("CSIGNON");
        let map = BmsMap::new("SIGNON", 24, 80);
        mapset.add_map(map);
        reg.register_mapset(mapset);

        assert!(reg.get_map("SIGNON", "CSIGNON").is_some());
        assert!(reg.get_map("NOPE", "CSIGNON").is_none());
    }

    #[test]
    fn bms_registry_build_screen() {
        let mut reg = BmsRegistry::new();
        let mut mapset = BmsMapset::new("MS");
        let mut map = BmsMap::new("M1", 24, 80);
        map.add_field(BmsField::new("F1", 1, 1, 10));
        mapset.add_map(map);
        reg.register_mapset(mapset);

        let mut data = HashMap::new();
        data.insert("F1".to_string(), "test".to_string());
        let screen = reg.build_screen("M1", "MS", &data, false, None);
        assert!(screen.is_some());
        assert_eq!(screen.unwrap().fields[0].value, "test");
    }

    #[test]
    fn bms_color_roundtrip() {
        assert_eq!(BmsColor::from_str("BLUE"), BmsColor::Blue);
        assert_eq!(BmsColor::from_str("turquoise"), BmsColor::Turquoise);
        assert_eq!(BmsColor::Blue.as_str(), "blue");
    }

    #[test]
    fn stdio_channel_send_receive() {
        use std::io::{BufReader, Cursor};
        let output_buf = Vec::<u8>::new();
        let input_data = b"AID=ENTER,USERID=ADMIN,PASSWD=secret\n";
        let mut ch = StdioScreenChannel::new(
            Box::new(output_buf),
            Box::new(BufReader::new(Cursor::new(input_data.to_vec()))),
        );
        let input = ch.receive_screen().unwrap();
        assert_eq!(input.aid, dfhaid::ENTER);
        assert_eq!(input.fields.get("USERID"), Some(&"ADMIN".to_string()));
        assert_eq!(input.fields.get("PASSWD"), Some(&"secret".to_string()));
    }

    #[test]
    fn dfhaid_all_pf_keys() {
        for i in 1..=24 {
            let name = format!("PF{}", i);
            let code = dfhaid::from_name(&name);
            assert_ne!(code, 0, "PF{} should have a code", i);
            assert_eq!(dfhaid::name(code), name);
        }
    }

    #[test]
    fn field_attribute_combinations() {
        let f = BmsField::new("X", 1, 1, 5).with_attr(dfhbmsca::UNPROT_NUM);
        assert!(f.is_numeric());
        assert!(!f.is_protected());

        let f2 = BmsField::new("Y", 1, 1, 5).with_attr(dfhbmsca::PROT_BRT);
        assert!(f2.is_protected());
        assert!(f2.is_bright());
        assert!(!f2.is_dark());
    }
}
