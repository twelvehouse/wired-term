use wired_vt_sys as sys;

/// A physical key, named by its unshifted meaning on a US layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A character key (letters as lowercase, digits, punctuation).
    Char(char),
    /// Enter / Return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Space bar.
    Space,
    /// Insert.
    Insert,
    /// Delete.
    Delete,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Function key F1..F24.
    F(u8),
}

impl Key {
    /// Parses a key name as used by gpui and most keymaps (`"enter"`, `"f5"`, `"a"`, `"-"`).
    pub fn from_name(name: &str) -> Option<Self> {
        let key = match name {
            "enter" => Self::Enter,
            "tab" => Self::Tab,
            "backspace" => Self::Backspace,
            "escape" => Self::Escape,
            "space" => Self::Space,
            "insert" => Self::Insert,
            "delete" => Self::Delete,
            "home" => Self::Home,
            "end" => Self::End,
            "pageup" => Self::PageUp,
            "pagedown" => Self::PageDown,
            "up" => Self::Up,
            "down" => Self::Down,
            "left" => Self::Left,
            "right" => Self::Right,
            _ => {
                if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                    return (1..=24).contains(&n).then_some(Self::F(n));
                }
                let mut chars = name.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                Self::Char(c.to_ascii_lowercase())
            }
        };
        Some(key)
    }

    /// Returns `true` for keys that never produce text on their own (arrows, F-keys, Enter, ...).
    pub fn is_special(self) -> bool {
        !matches!(self, Self::Char(_) | Self::Space)
    }

    pub(crate) fn to_ghostty(self) -> sys::GhosttyKey {
        match self {
            Self::Char(c) => char_key(c),
            Self::Enter => sys::GHOSTTY_KEY_ENTER,
            Self::Tab => sys::GHOSTTY_KEY_TAB,
            Self::Backspace => sys::GHOSTTY_KEY_BACKSPACE,
            Self::Escape => sys::GHOSTTY_KEY_ESCAPE,
            Self::Space => sys::GHOSTTY_KEY_SPACE,
            Self::Insert => sys::GHOSTTY_KEY_INSERT,
            Self::Delete => sys::GHOSTTY_KEY_DELETE,
            Self::Home => sys::GHOSTTY_KEY_HOME,
            Self::End => sys::GHOSTTY_KEY_END,
            Self::PageUp => sys::GHOSTTY_KEY_PAGE_UP,
            Self::PageDown => sys::GHOSTTY_KEY_PAGE_DOWN,
            Self::Up => sys::GHOSTTY_KEY_ARROW_UP,
            Self::Down => sys::GHOSTTY_KEY_ARROW_DOWN,
            Self::Left => sys::GHOSTTY_KEY_ARROW_LEFT,
            Self::Right => sys::GHOSTTY_KEY_ARROW_RIGHT,
            Self::F(n) => sys::GHOSTTY_KEY_F1 + sys::GhosttyKey::from(n.clamp(1, 24) - 1),
        }
    }

    pub(crate) fn unshifted_codepoint(self) -> Option<u32> {
        match self {
            Self::Char(c) => Some(u32::from(c)),
            Self::Space => Some(u32::from(' ')),
            _ => None,
        }
    }
}

fn char_key(c: char) -> sys::GhosttyKey {
    match c {
        'a'..='z' => sys::GHOSTTY_KEY_A + (u32::from(c) - u32::from('a')) as sys::GhosttyKey,
        '0'..='9' => sys::GHOSTTY_KEY_DIGIT_0 + (u32::from(c) - u32::from('0')) as sys::GhosttyKey,
        '`' => sys::GHOSTTY_KEY_BACKQUOTE,
        '\\' => sys::GHOSTTY_KEY_BACKSLASH,
        '[' => sys::GHOSTTY_KEY_BRACKET_LEFT,
        ']' => sys::GHOSTTY_KEY_BRACKET_RIGHT,
        ',' => sys::GHOSTTY_KEY_COMMA,
        '=' => sys::GHOSTTY_KEY_EQUAL,
        '-' => sys::GHOSTTY_KEY_MINUS,
        '.' => sys::GHOSTTY_KEY_PERIOD,
        '\'' => sys::GHOSTTY_KEY_QUOTE,
        ';' => sys::GHOSTTY_KEY_SEMICOLON,
        '/' => sys::GHOSTTY_KEY_SLASH,
        _ => sys::GHOSTTY_KEY_UNIDENTIFIED,
    }
}

/// Modifier keys held during a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt.
    pub alt: bool,
    /// Windows / Super.
    pub super_key: bool,
}

impl Mods {
    pub(crate) fn to_ghostty(self) -> sys::GhosttyMods {
        let mut bits = 0;
        if self.shift {
            bits |= sys::GHOSTTY_MODS_SHIFT;
        }
        if self.ctrl {
            bits |= sys::GHOSTTY_MODS_CTRL;
        }
        if self.alt {
            bits |= sys::GHOSTTY_MODS_ALT;
        }
        if self.super_key {
            bits |= sys::GHOSTTY_MODS_SUPER;
        }
        bits as sys::GhosttyMods
    }
}

/// A key press to encode for the program running in the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    /// The key.
    pub key: Key,
    /// Held modifiers.
    pub mods: Mods,
    /// Text the key produces with the current modifiers and layout, if any.
    pub text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gpui_key_names() {
        assert_eq!(Key::from_name("enter"), Some(Key::Enter));
        assert_eq!(Key::from_name("f12"), Some(Key::F(12)));
        assert_eq!(Key::from_name("f25"), None);
        assert_eq!(Key::from_name("C"), Some(Key::Char('c')));
        assert_eq!(Key::from_name("-"), Some(Key::Char('-')));
        assert_eq!(Key::from_name("unknown"), None);
    }

    #[test]
    fn maps_letters_and_function_keys() {
        assert_eq!(Key::Char('z').to_ghostty(), sys::GHOSTTY_KEY_Z);
        assert_eq!(Key::Char('5').to_ghostty(), sys::GHOSTTY_KEY_DIGIT_5);
        assert_eq!(Key::F(12).to_ghostty(), sys::GHOSTTY_KEY_F12);
    }
}
