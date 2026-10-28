//! dt-ocr TOML config: default frame size + hotkey bindings. One bad value
//! keeps its default with a warning; only a fundamentally unreadable file is
//! an error (desktop-tools-rs convention).

use anyhow::{Context, Result};
use dt_common::keys::Combo;

pub const CONFIG_NAME: &str = "dt-ocr.toml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub frame_w: i32,
    pub frame_h: i32,
    /// Constrain the frame to the monitor work area, excluding the taskbar.
    pub work_area: bool,
    /// Show the format hint on the spec-mode command bar (default true).
    pub show_hint: bool,
    /// Status-line font face (default Consolas); any installed GDI face
    /// name works, monospace recommended.
    pub font: Option<String>,
    /// Upscale factor applied to the capture before OCR (1..=4, default 2):
    /// small/compact screen fonts read much better at 2-3x (ShareX/UiPath
    /// call the same knob "scale factor"). Clamped down when the scaled
    /// image would exceed the engine's max dimension or the pixel budget.
    pub ocr_scale: u32,
    /// Recognizer language tag (`zh`, `en`, `zh-Hans-CN`, ...). `None` =
    /// auto: prefer a CJK pack, then English. A tag that is not installed
    /// falls back to auto with a warning.
    pub ocr_language: Option<String>,
    /// How the mask paints the recognised text: per-word boxes (the original
    /// geometry) or one continuous band per OCR line segment (words keep
    /// their exact positions; only the gaps between them are filled).
    pub mask_fill: MaskFill,
    /// Mask text layout: `overlay` keeps the original geometry, `reader`
    /// reflows the text as one wrapped document.
    pub mask_layout: MaskLayout,
    /// Mask font face (default = the dotfiles WezTerm font, Sarasa Term SC
    /// Nerd, so re-rendered text matches the terminal).
    pub mask_font: String,
    /// Mask text size in points; `Auto` keeps the per-line source height.
    pub mask_size: MaskSize,
    /// Directory for saved captures (env vars expanded); empty = clipboard
    /// only, no file.
    pub capture_dir: String,
    /// Pin edge decoration: translucent hatch (default), a solid line, or
    /// nothing.
    pub pin_border: PinBorder,
    /// `line` border colour, 0xRRGGBB (default black).
    pub pin_line_color: u32,
    /// `hatch` colour, 0xRRGGBB (default yellow).
    pub pin_hatch_color: u32,
    pub bindings: Bindings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinBorder {
    Hatch,
    Line,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskFill {
    Boxes,
    Lines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskLayout {
    Overlay,
    Reader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskSize {
    /// Legacy: size each line from its OCR box height.
    Auto,
    /// One fixed size for all text, in points (WezTerm's `font_size` unit).
    Pt(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bindings {
    pub draw: Combo,
    pub resize: Combo,
    pub pin: Combo,
    pub capture: Combo,
    pub frame_window: Combo,
    pub pin_shot: Combo,
    pub r#move: Combo,
    pub toggle: Combo,
}

/// Frame actions a binding can trigger (hotkey ids index into this order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Draw,
    Resize,
    Pin,
    Capture,
    FrameWindow,
    PinShot,
    Move,
    Toggle,
}

impl Action {
    pub const ALL: [Action; 8] = [
        Action::Draw,
        Action::Resize,
        Action::Pin,
        Action::Capture,
        Action::FrameWindow,
        Action::PinShot,
        Action::Move,
        Action::Toggle,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Action::Draw => "draw",
            Action::Resize => "resize",
            Action::Pin => "pin",
            Action::Capture => "capture",
            Action::FrameWindow => "frame_window",
            Action::PinShot => "pin_shot",
            Action::Move => "move",
            Action::Toggle => "toggle",
        }
    }
    fn default_combo(self) -> Combo {
        // Combos are plain data; unwrap is unreachable for these literals
        // and the roundtrip test below guards against typos.
        match self {
            Action::Draw => "ctrl+alt+d".parse().unwrap(),
            Action::Resize => "ctrl+alt+s".parse().unwrap(),
            Action::Pin => "ctrl+shift+alt+t".parse().unwrap(),
            Action::Capture => "ctrl+enter".parse().unwrap(),
            Action::FrameWindow => "ctrl+alt+w".parse().unwrap(),
            Action::PinShot => "ctrl+alt+p".parse().unwrap(),
            Action::Move => "ctrl+alt+m".parse().unwrap(),
            // Win+O is reserved by Windows (orientation lock; RegisterHotKey
            // fails with 1409), Win+Shift+O is free.
            Action::Toggle => "win+shift+o".parse().unwrap(),
        }
    }
    pub(crate) fn get(self, b: &Bindings) -> Combo {
        match self {
            Action::Draw => b.draw,
            Action::Resize => b.resize,
            Action::Pin => b.pin,
            Action::Capture => b.capture,
            Action::FrameWindow => b.frame_window,
            Action::PinShot => b.pin_shot,
            Action::Move => b.r#move,
            Action::Toggle => b.toggle,
        }
    }
    pub(crate) fn set(self, b: &mut Bindings, c: Combo) {
        match self {
            Action::Draw => b.draw = c,
            Action::Resize => b.resize = c,
            Action::Pin => b.pin = c,
            Action::Capture => b.capture = c,
            Action::FrameWindow => b.frame_window = c,
            Action::PinShot => b.pin_shot = c,
            Action::Move => b.r#move = c,
            Action::Toggle => b.toggle = c,
        }
    }
}

impl Default for Bindings {
    fn default() -> Self {
        let mut b = Bindings {
            draw: Combo::default(),
            resize: Combo::default(),
            pin: Combo::default(),
            capture: Combo::default(),
            frame_window: Combo::default(),
            pin_shot: Combo::default(),
            r#move: Combo::default(),
            toggle: Combo::default(),
        };
        for a in Action::ALL {
            a.set(&mut b, a.default_combo());
        }
        b
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            frame_w: 800,
            frame_h: 600,
            work_area: true,
            show_hint: true,
            font: None,
            ocr_scale: 4,
            ocr_language: None,
            mask_fill: MaskFill::Lines,
            mask_layout: MaskLayout::Overlay,
            mask_font: "Sarasa Term SC Nerd".to_string(),
            mask_size: MaskSize::Pt(10),
            capture_dir: "%USERPROFILE%\\Pictures\\dt-ocr".to_string(),
            pin_border: PinBorder::Hatch,
            pin_line_color: 0,
            pin_hatch_color: 0xffff00,
            bindings: Bindings::default(),
        }
    }
}

/// Colour names plus `#RRGGBB`, enough for a border setting.
fn parse_color(s: &str) -> Option<u32> {
    let named = match s.to_ascii_lowercase().as_str() {
        "black" => Some(0x000000),
        "white" => Some(0xffffff),
        "red" => Some(0xff0000),
        "green" => Some(0x00ff00),
        "blue" => Some(0x0000ff),
        "cyan" => Some(0x00ffff),
        "yellow" => Some(0xffff00),
        "magenta" => Some(0xff00ff),
        "gray" | "grey" => Some(0x808080),
        _ => None,
    };
    named.or_else(|| {
        s.strip_prefix('#')
            .filter(|h| h.len() == 6)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
    })
}

pub fn parse(text: &str) -> Result<Config> {
    let value: toml::Value = text.parse().context("dt-ocr.toml is not valid TOML")?;
    let table = value
        .as_table()
        .context("dt-ocr.toml root must be a TOML table")?;
    let mut cfg = Config::default();
    if let Some(frame) = table.get("frame") {
        match frame.as_table() {
            Some(t) => {
                if let Some(w) = t.get("width") {
                    match w.as_integer() {
                        Some(n) if (16..=20_000).contains(&n) => cfg.frame_w = n as i32,
                        _ => eprintln!(
                            "warn: frame.width must be 16..=20000; using {}",
                            cfg.frame_w
                        ),
                    }
                }
                if let Some(h) = t.get("height") {
                    match h.as_integer() {
                        Some(n) if (16..=20_000).contains(&n) => cfg.frame_h = n as i32,
                        _ => eprintln!(
                            "warn: frame.height must be 16..=20000; using {}",
                            cfg.frame_h
                        ),
                    }
                }
                if let Some(v) = t.get("work_area") {
                    match v.as_bool() {
                        Some(b) => cfg.work_area = b,
                        None => eprintln!("warn: frame.work_area must be true/false; using true"),
                    }
                }
                if let Some(v) = t.get("show_hint") {
                    match v.as_bool() {
                        Some(b) => cfg.show_hint = b,
                        None => eprintln!("warn: frame.show_hint must be true/false; using true"),
                    }
                }
                if let Some(v) = t.get("font") {
                    match v.as_str() {
                        Some(s) if !s.is_empty() => cfg.font = Some(s.to_string()),
                        _ => {
                            eprintln!("warn: frame.font must be a non-empty string; using Consolas")
                        }
                    }
                }
            }
            None => eprintln!("warn: [frame] must be a table; using defaults"),
        }
    }
    if let Some(ocr) = table.get("ocr") {
        match ocr.as_table() {
            Some(t) => {
                if let Some(v) = t.get("scale") {
                    match v.as_integer() {
                        Some(n) if (1..=4).contains(&n) => cfg.ocr_scale = n as u32,
                        _ => eprintln!("warn: ocr.scale must be 1..=4; using 4"),
                    }
                }
                if let Some(v) = t.get("language") {
                    match v.as_str() {
                        Some("auto") | Some("") => cfg.ocr_language = None,
                        Some(s) => cfg.ocr_language = Some(s.to_string()),
                        None => eprintln!("warn: ocr.language must be a string; using auto"),
                    }
                }
            }
            None => eprintln!("warn: [ocr] must be a table; using defaults"),
        }
    }
    if let Some(mask) = table.get("mask") {
        match mask.as_table() {
            Some(t) => {
                if let Some(v) = t.get("fill") {
                    match v.as_str() {
                        Some("lines") => cfg.mask_fill = MaskFill::Lines,
                        Some("boxes") => cfg.mask_fill = MaskFill::Boxes,
                        _ => {
                            eprintln!("warn: mask.fill must be \"lines\" or \"boxes\"; using lines")
                        }
                    }
                }
                if let Some(v) = t.get("layout") {
                    match v.as_str() {
                        Some("overlay") => cfg.mask_layout = MaskLayout::Overlay,
                        Some("reader") => cfg.mask_layout = MaskLayout::Reader,
                        _ => eprintln!(
                            "warn: mask.layout must be \"overlay\" or \"reader\"; using overlay"
                        ),
                    }
                }
                if let Some(v) = t.get("font") {
                    match v.as_str() {
                        Some(s) if !s.is_empty() => cfg.mask_font = s.to_string(),
                        _ => eprintln!(
                            "warn: mask.font must be a non-empty string; using the default"
                        ),
                    }
                }
                if let Some(v) = t.get("size") {
                    match v {
                        toml::Value::String(s) if s == "auto" => cfg.mask_size = MaskSize::Auto,
                        toml::Value::Integer(n) if (1..=72).contains(n) => {
                            cfg.mask_size = MaskSize::Pt(*n as u32)
                        }
                        _ => eprintln!(
                            "warn: mask.size must be \"auto\" or 1..=72 (points); using 10"
                        ),
                    }
                }
            }
            None => eprintln!("warn: [mask] must be a table; using defaults"),
        }
    }
    if let Some(capture) = table.get("capture") {
        match capture.as_table() {
            Some(t) => {
                if let Some(v) = t.get("dir") {
                    match v.as_str() {
                        Some(s) => cfg.capture_dir = s.to_string(),
                        None => eprintln!("warn: capture.dir must be a string; using the default"),
                    }
                }
            }
            None => eprintln!("warn: [capture] must be a table; using defaults"),
        }
    }
    if let Some(pin) = table.get("pin") {
        match pin.as_table() {
            Some(t) => {
                if let Some(v) = t.get("border") {
                    match v.as_str() {
                        Some("hatch") => cfg.pin_border = PinBorder::Hatch,
                        Some("line") => cfg.pin_border = PinBorder::Line,
                        Some("none") => cfg.pin_border = PinBorder::None,
                        _ => eprintln!(
                            "warn: pin.border must be \"hatch\", \"line\" or \"none\"; using hatch"
                        ),
                    }
                }
                if let Some(v) = t.get("line-color") {
                    match v.as_str().and_then(parse_color) {
                        Some(c) => cfg.pin_line_color = c,
                        None => eprintln!(
                            "warn: pin.line-color must be a colour name or #RRGGBB; using black"
                        ),
                    }
                }
                if let Some(v) = t.get("hatch-color") {
                    match v.as_str().and_then(parse_color) {
                        Some(c) => cfg.pin_hatch_color = c,
                        None => eprintln!(
                            "warn: pin.hatch-color must be a colour name or #RRGGBB; using yellow"
                        ),
                    }
                }
            }
            None => eprintln!("warn: [pin] must be a table; using defaults"),
        }
    }
    if let Some(binds) = table.get("bindings") {
        match binds.as_table() {
            Some(t) => {
                for (name, value) in t {
                    let Some(action) = Action::ALL.iter().find(|a| a.name() == name) else {
                        eprintln!("warn: unknown binding `{name}` (known: draw, resize, pin, capture, window, move, toggle); skipped");
                        continue;
                    };
                    match value.as_str() {
                        Some(s) => match s.parse::<Combo>() {
                            Ok(c) => action.set(&mut cfg.bindings, c),
                            Err(e) => eprintln!("warn: binding `{name}`: {e}; using default"),
                        },
                        None => eprintln!("warn: binding `{name}` must be a string; using default"),
                    }
                }
            }
            None => eprintln!("warn: [bindings] must be a table; using defaults"),
        }
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dt_common::keys::Key;

    #[test]
    fn parses_repo_template() {
        let cfg = parse(include_str!("../../dt-ocr.toml")).unwrap();
        assert_eq!(
            cfg,
            Config {
                frame_w: 800,
                frame_h: 600,
                work_area: true,
                show_hint: true,
                font: None,
                ocr_scale: 4,
                ocr_language: None,
                mask_fill: MaskFill::Lines,
                mask_layout: MaskLayout::Overlay,
                mask_font: "Sarasa Term SC Nerd".to_string(),
                mask_size: MaskSize::Pt(10),
                capture_dir: "%USERPROFILE%\\Pictures\\dt-ocr".to_string(),
                pin_border: PinBorder::Hatch,
                pin_line_color: 0,
                pin_hatch_color: 0xffff00,
                bindings: Bindings::default(),
            }
        );
    }

    #[test]
    fn pin_border_and_line_colour_parse() {
        let cfg = parse("[pin]\nborder = \"none\"\n").unwrap();
        assert_eq!(cfg.pin_border, PinBorder::None);
        let cfg = parse("[pin]\nborder = \"line\"\nline-color = \"#ff8800\"\n").unwrap();
        assert_eq!(cfg.pin_border, PinBorder::Line);
        assert_eq!(cfg.pin_line_color, 0xff8800);
        let cfg = parse("[pin]\nborder = \"bogus\"\nline-color = \"nope\"\n").unwrap();
        assert_eq!(cfg.pin_border, PinBorder::Hatch);
        assert_eq!(cfg.pin_line_color, 0);
        assert_eq!(cfg.pin_hatch_color, 0xffff00);
        let cfg = parse("[pin]\nhatch-color = \"#ffaa00\"\n").unwrap();
        assert_eq!(cfg.pin_hatch_color, 0xffaa00);
        assert_eq!(parse_color("Black"), Some(0));
        assert_eq!(parse_color("#00FF00"), Some(0x00ff00));
        assert_eq!(parse_color("#12345"), None);
    }

    #[test]
    fn empty_config_uses_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn bad_values_keep_defaults_and_parse_continues() {
        let cfg = parse("[frame]\nwidth = 5\nheight = 90000\n").unwrap();
        assert_eq!((cfg.frame_w, cfg.frame_h), (800, 600));
        let cfg = parse("[frame]\nwidth = \"wide\"\nheight = 1024\n").unwrap();
        assert_eq!((cfg.frame_w, cfg.frame_h), (800, 1024));
    }

    #[test]
    fn work_area_parses_and_bad_value_keeps_taskbar_exclusion() {
        assert!(!parse("[frame]\nwork_area = false\n").unwrap().work_area);
        assert!(parse("[frame]\nwork_area = 3\n").unwrap().work_area);
    }

    #[test]
    fn frame_must_be_a_table() {
        let cfg = parse("frame = 3\n").unwrap();
        assert_eq!((cfg.frame_w, cfg.frame_h), (800, 600));
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(parse("[[[").is_err());
        assert!(parse("42").is_err());
    }

    #[test]
    fn bindings_parse_and_validate() {
        let cfg = parse("[bindings]\ndraw = \"^!g\"\nresize = \"ctrl+alt+s\"\n").unwrap();
        assert_eq!(cfg.bindings.draw.key, Key::Char('g'));
        assert!(cfg.bindings.draw.ctrl && cfg.bindings.draw.alt);
        assert_eq!(cfg.bindings.resize, "ctrl+alt+s".parse::<Combo>().unwrap());
        // Untouched actions keep their defaults.
        assert_eq!(cfg.bindings.pin, Action::Pin.default_combo());
        assert_eq!(cfg.bindings.capture, Action::Capture.default_combo());
    }

    #[test]
    fn ocr_scale_parses_and_keeps_default_on_bad_values() {
        assert_eq!(parse("[ocr]\nscale = 3\n").unwrap().ocr_scale, 3);
        assert_eq!(parse("[ocr]\nscale = 1\n").unwrap().ocr_scale, 1);
        // Out of range, wrong type, or not a table: keep the default.
        assert_eq!(parse("[ocr]\nscale = 0\n").unwrap().ocr_scale, 4);
        assert_eq!(parse("[ocr]\nscale = 9\n").unwrap().ocr_scale, 4);
        assert_eq!(parse("[ocr]\nscale = \"2x\"\n").unwrap().ocr_scale, 4);
        assert_eq!(parse("ocr = 3\n").unwrap().ocr_scale, 4);
    }

    #[test]
    fn ocr_language_parses_and_auto_is_none() {
        assert_eq!(
            parse("[ocr]\nlanguage = \"en\"\n").unwrap().ocr_language,
            Some("en".to_string())
        );
        assert_eq!(
            parse("[ocr]\nlanguage = \"auto\"\n").unwrap().ocr_language,
            None
        );
        assert_eq!(parse("[ocr]\nlanguage = 5\n").unwrap().ocr_language, None);
    }

    #[test]
    fn mask_fill_parses_and_invalid_keeps_lines() {
        assert_eq!(
            parse("[mask]\nfill = \"boxes\"\n").unwrap().mask_fill,
            MaskFill::Boxes
        );
        assert_eq!(
            parse("[mask]\nfill = \"lines\"\n").unwrap().mask_fill,
            MaskFill::Lines
        );
        assert_eq!(
            parse("[mask]\nfill = \"blur\"\n").unwrap().mask_fill,
            MaskFill::Lines
        );
        assert_eq!(parse("mask = 1\n").unwrap().mask_fill, MaskFill::Lines);
    }

    #[test]
    fn mask_style_parses() {
        let cfg = parse("[mask]\nlayout = \"reader\"\nfont = \"Consolas\"\nsize = 12\n").unwrap();
        assert_eq!(cfg.mask_layout, MaskLayout::Reader);
        assert_eq!(cfg.mask_font, "Consolas");
        assert_eq!(cfg.mask_size, MaskSize::Pt(12));
        assert_eq!(
            parse("[mask]\nsize = \"auto\"\n").unwrap().mask_size,
            MaskSize::Auto
        );
        // Invalid values keep the fixed-size default.
        assert_eq!(
            parse("[mask]\nsize = 0\n").unwrap().mask_size,
            MaskSize::Pt(10)
        );
        assert_eq!(
            parse("[mask]\nsize = 100\n").unwrap().mask_size,
            MaskSize::Pt(10)
        );
        assert_eq!(
            parse("[mask]\nlayout = \"page\"\n").unwrap().mask_layout,
            MaskLayout::Overlay
        );
    }

    #[test]
    fn toggle_binding_parses_and_defaults_to_win_shift_o() {
        let cfg = parse("").unwrap();
        let combo = cfg.bindings.toggle;
        assert!(combo.win && combo.shift && !combo.ctrl && !combo.alt);
        assert_eq!(combo, "win+shift+o".parse::<Combo>().unwrap());
        let cfg = parse("[bindings]\ntoggle = \"ctrl+alt+o\"\n").unwrap();
        assert_eq!(cfg.bindings.toggle, "ctrl+alt+o".parse::<Combo>().unwrap());
    }

    #[test]
    fn bad_bindings_warn_and_keep_defaults() {
        let cfg =
            parse("[bindings]\npin = \"not-a-combo\"\ncapture = 42\nnope = \"ctrl+a\"\n").unwrap();
        assert_eq!(cfg.bindings.pin, Action::Pin.default_combo());
        assert_eq!(cfg.bindings.capture, Action::Capture.default_combo());
        assert_eq!(cfg.bindings.draw, Action::Draw.default_combo());
    }

    #[test]
    fn bindings_must_be_a_table() {
        let cfg = parse("bindings = \"ctrl+a\"\n").unwrap();
        assert_eq!(cfg.bindings, Bindings::default());
    }
}
