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
    /// Show the format hint on the spec-mode command bar (default true).
    pub show_hint: bool,
    /// Status-line font face (default Consolas); any installed GDI face
    /// name works, monospace recommended.
    pub font: Option<String>,
    pub bindings: Bindings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bindings {
    pub draw: Combo,
    pub resize: Combo,
    pub pin: Combo,
    pub capture: Combo,
    pub r#move: Combo,
}

/// Frame actions a binding can trigger (hotkey ids index into this order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Draw,
    Resize,
    Pin,
    Capture,
    Move,
}

impl Action {
    pub const ALL: [Action; 5] = [
        Action::Draw,
        Action::Resize,
        Action::Pin,
        Action::Capture,
        Action::Move,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Action::Draw => "draw",
            Action::Resize => "resize",
            Action::Pin => "pin",
            Action::Capture => "capture",
            Action::Move => "move",
        }
    }
    fn default_combo(self) -> Combo {
        // Combos are plain data; unwrap is unreachable for these literals
        // and the roundtrip test below guards against typos.
        match self {
            Action::Draw => "ctrl+alt+d".parse().unwrap(),
            Action::Resize => "ctrl+alt+s".parse().unwrap(),
            Action::Pin => "ctrl+t".parse().unwrap(),
            Action::Capture => "ctrl+enter".parse().unwrap(),
            Action::Move => "ctrl+alt+m".parse().unwrap(),
        }
    }
    pub(crate) fn get(self, b: &Bindings) -> Combo {
        match self {
            Action::Draw => b.draw,
            Action::Resize => b.resize,
            Action::Pin => b.pin,
            Action::Capture => b.capture,
            Action::Move => b.r#move,
        }
    }
    pub(crate) fn set(self, b: &mut Bindings, c: Combo) {
        match self {
            Action::Draw => b.draw = c,
            Action::Resize => b.resize = c,
            Action::Pin => b.pin = c,
            Action::Capture => b.capture = c,
            Action::Move => b.r#move = c,
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
            r#move: Combo::default(),
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
            show_hint: true,
            font: None,
            bindings: Bindings::default(),
        }
    }
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
    if let Some(binds) = table.get("bindings") {
        match binds.as_table() {
            Some(t) => {
                for (name, value) in t {
                    let Some(action) = Action::ALL.iter().find(|a| a.name() == name) else {
                        eprintln!("warn: unknown binding `{name}` (known: draw, resize, pin, capture, move); skipped");
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
                show_hint: true,
                font: None,
                bindings: Bindings::default(),
            }
        );
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
