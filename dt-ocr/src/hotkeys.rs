//! Global hotkeys via RegisterHotKey (non-Win combos only — Win combos need
//! the kbdhook, which lands with the keyboard tools; see AGENTS.md).
//! Provenance: vk_of_key/modifiers_of adapted from desktop-tools-rs
//! src/win32.rs (repo folder window-tools-rs).

use crate::config::{Action, Bindings};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub(crate) fn vk_of_key(key: dt_common::keys::Key) -> Option<VIRTUAL_KEY> {
    use dt_common::keys::Key;
    Some(match key {
        Key::Char('`') => VK_OEM_3,
        Key::Char(';') => VK_OEM_1,
        Key::Char('=') => VK_OEM_PLUS,
        Key::Char(',') => VK_OEM_COMMA,
        Key::Char('-') => VK_OEM_MINUS,
        Key::Char('.') => VK_OEM_PERIOD,
        Key::Char('/') => VK_OEM_2,
        Key::Char('[') => VK_OEM_4,
        Key::Char('\\') => VK_OEM_5,
        Key::Char(']') => VK_OEM_6,
        Key::Char('\'') => VK_OEM_7,
        Key::Char(c) => VIRTUAL_KEY(c.to_ascii_uppercase() as u16),
        Key::F(1) => VK_F1,
        Key::F(2) => VK_F2,
        Key::F(3) => VK_F3,
        Key::F(4) => VK_F4,
        Key::F(5) => VK_F5,
        Key::F(6) => VK_F6,
        Key::F(7) => VK_F7,
        Key::F(8) => VK_F8,
        Key::F(9) => VK_F9,
        Key::F(10) => VK_F10,
        Key::F(11) => VK_F11,
        Key::F(12) => VK_F12,
        Key::F(n) => VIRTUAL_KEY(VK_F12.0 + (n - 12) as u16),
        Key::Insert => VK_INSERT,
        Key::Delete => VK_DELETE,
        Key::Space => VK_SPACE,
        Key::Tab => VK_TAB,
        Key::Backspace => VK_BACK,
        Key::Escape => VK_ESCAPE,
        Key::Enter => VK_RETURN,
        Key::Left => VK_LEFT,
        Key::Right => VK_RIGHT,
        Key::Up => VK_UP,
        Key::Down => VK_DOWN,
        Key::Home => VK_HOME,
        Key::End => VK_END,
        Key::PageUp => VK_PRIOR,
        Key::PageDown => VK_NEXT,
        Key::Snapshot => VK_SNAPSHOT,
        Key::Pause => VK_PAUSE,
        Key::Other => return None,
    })
}

fn modifiers_of(c: &dt_common::keys::Combo) -> HOT_KEY_MODIFIERS {
    let mut m = MOD_NOREPEAT;
    if c.ctrl {
        m |= MOD_CONTROL;
    }
    if c.shift {
        m |= MOD_SHIFT;
    }
    if c.alt {
        m |= MOD_ALT;
    }
    if c.win {
        m |= MOD_WIN;
    }
    m
}

/// Registered hotkey set. Ids are 1-based indexes into `Action::ALL`.
pub struct Hotkeys {
    combos: Vec<dt_common::keys::Combo>,
}

impl Hotkeys {
    /// Registers every binding against `hwnd` (window-tied so hotkey
    /// detective tools can attribute the owner; a thread-tied registration
    /// works the same but shows no owner). Returns the combos that could
    /// not be registered (conflict or unsupported key); the caller warns
    /// and runs with the rest, per the desktop-tools-rs convention.
    pub fn register(hwnd: HWND, bindings: &Bindings) -> (Hotkeys, Vec<String>) {
        let mut combos = Vec::new();
        let mut failed = Vec::new();
        for action in Action::ALL {
            let combo = action.get(bindings);
            match vk_of_key(combo.key) {
                Some(vk) => {
                    let id = (combos.len() + 1) as i32;
                    let ok = unsafe {
                        RegisterHotKey(Some(hwnd), id, modifiers_of(&combo), vk.0 as u32).is_ok()
                    };
                    if ok {
                        combos.push(combo);
                    } else {
                        failed.push(format!("{} = \"{}\"", action.name(), combo));
                    }
                }
                None => failed.push(format!("{} = \"{}\" (unknown key)", action.name(), combo)),
            }
        }
        (Hotkeys { combos }, failed)
    }

    pub fn unregister_all(&self, hwnd: HWND) {
        for (i, _) in self.combos.iter().enumerate() {
            unsafe {
                let _ = UnregisterHotKey(Some(hwnd), (i + 1) as i32);
            }
        }
    }

    /// Maps a WM_HOTKEY id back to its action.
    pub fn action_of(&self, id: i32) -> Option<Action> {
        if id < 1 || id as usize > self.combos.len() {
            return None;
        }
        Action::ALL.get((id - 1) as usize).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.combos.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dt_common::keys::Key;

    #[test]
    fn vk_mapping_covers_all_combo_keys() {
        // Every Key variant a binding can parse must map to a VK code,
        // otherwise register() silently fails the binding.
        for key in [
            Key::Char('a'),
            Key::Char('`'),
            Key::Char('['),
            Key::F(13),
            Key::Enter,
            Key::Snapshot,
        ] {
            assert!(vk_of_key(key).is_some(), "{key:?} has no VK mapping");
        }
        assert!(vk_of_key(Key::Other).is_none());
    }
}
