// 键鼠输入。Enigo 在 macOS 上不能跨线程，也不该长期持有：调用方每个动作新建一个 Controller，
// 放在主线程上用，用完就丢。
use enigo::{Axis, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    fn enigo(self) -> enigo::Button {
        match self {
            Self::Left => enigo::Button::Left,
            Self::Right => enigo::Button::Right,
            Self::Middle => enigo::Button::Middle,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyName {
    Control,
    Shift,
    Alt,
    Meta,
    Return,
    Escape,
    Tab,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Function(u8),
    Char(char),
}

impl KeyName {
    fn enigo(self) -> Key {
        match self {
            Self::Control => Key::Control,
            Self::Shift => Key::Shift,
            Self::Alt => Key::Alt,
            Self::Meta => Key::Meta,
            Self::Return => Key::Return,
            Self::Escape => Key::Escape,
            Self::Tab => Key::Tab,
            Self::Backspace => Key::Backspace,
            Self::Delete => Key::Delete,
            Self::Space => Key::Space,
            Self::Up => Key::UpArrow,
            Self::Down => Key::DownArrow,
            Self::Left => Key::LeftArrow,
            Self::Right => Key::RightArrow,
            Self::Home => Key::Home,
            Self::End => Key::End,
            Self::PageUp => Key::PageUp,
            Self::PageDown => Key::PageDown,
            Self::Function(number) => match number {
                1 => Key::F1,
                2 => Key::F2,
                3 => Key::F3,
                4 => Key::F4,
                5 => Key::F5,
                6 => Key::F6,
                7 => Key::F7,
                8 => Key::F8,
                9 => Key::F9,
                10 => Key::F10,
                11 => Key::F11,
                _ => Key::F12,
            },
            Self::Char(value) => Key::Unicode(value),
        }
    }
}

/// 一组同时按下的键：前面是修饰键，最后一个是主键。
pub type Chord = Vec<KeyName>;

/// 解析 "ctrl+shift+a"、"cmd+l"、"Enter"。不认识的键直接报错，不猜。
pub fn parse_chord(text: &str) -> Result<Chord, String> {
    let chord = text
        .split('+')
        .map(|part| {
            let part = part.trim();
            let name = part.to_ascii_lowercase();
            Ok(match name.as_str() {
                "ctrl" | "control" => KeyName::Control,
                "shift" => KeyName::Shift,
                "alt" | "option" | "opt" => KeyName::Alt,
                "cmd" | "command" | "meta" | "win" | "windows" | "super" => KeyName::Meta,
                "enter" | "return" => KeyName::Return,
                "esc" | "escape" => KeyName::Escape,
                "tab" => KeyName::Tab,
                "backspace" => KeyName::Backspace,
                "delete" | "del" => KeyName::Delete,
                "space" => KeyName::Space,
                "up" | "arrowup" => KeyName::Up,
                "down" | "arrowdown" => KeyName::Down,
                "left" | "arrowleft" => KeyName::Left,
                "right" | "arrowright" => KeyName::Right,
                "home" => KeyName::Home,
                "end" => KeyName::End,
                "pageup" => KeyName::PageUp,
                "pagedown" => KeyName::PageDown,
                other => {
                    if let Some(number) = other
                        .strip_prefix('f')
                        .and_then(|digits| digits.parse::<u8>().ok())
                        .filter(|number| (1..=12).contains(number))
                    {
                        KeyName::Function(number)
                    } else {
                        let mut chars = other.chars();
                        match (chars.next(), chars.next()) {
                            (Some(value), None) => KeyName::Char(value),
                            _ => return Err(format!("Unknown key \"{part}\".")),
                        }
                    }
                }
            })
        })
        .collect::<Result<Chord, String>>()?;
    if chord.is_empty() {
        return Err("No key was given.".into());
    }
    Ok(chord)
}

pub struct Controller(Enigo);

impl Controller {
    pub fn new() -> Result<Self, String> {
        let settings = Settings {
            // 权限由 request_permissions 统一处理，这里不再弹一次。
            open_prompt_to_get_permissions: false,
            ..Settings::default()
        };
        Enigo::new(&settings)
            .map(Self)
            .map_err(|error| format!("The input device is not available: {error}"))
    }

    pub fn location(&self) -> Result<(i32, i32), String> {
        self.0.location().map_err(|error| error.to_string())
    }

    pub fn move_to(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.0
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|error| error.to_string())
    }

    pub fn press(&mut self, button: Button) -> Result<(), String> {
        self.0
            .button(button.enigo(), Direction::Press)
            .map_err(|error| error.to_string())
    }

    pub fn release(&mut self, button: Button) -> Result<(), String> {
        self.0
            .button(button.enigo(), Direction::Release)
            .map_err(|error| error.to_string())
    }

    pub fn click(&mut self, button: Button) -> Result<(), String> {
        self.0
            .button(button.enigo(), Direction::Click)
            .map_err(|error| error.to_string())
    }

    /// 正数向下或向右，单位是滚轮格。
    pub fn scroll(&mut self, amount: i32, horizontal: bool) -> Result<(), String> {
        let axis = if horizontal {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        self.0
            .scroll(amount, axis)
            .map_err(|error| error.to_string())
    }

    pub fn text(&mut self, text: &str) -> Result<(), String> {
        self.0.text(text).map_err(|error| error.to_string())
    }

    /// 依次按下再倒序松开。中途出错也要松开已经按下的键，否则修饰键会卡住。
    pub fn chord(&mut self, chord: &[KeyName]) -> Result<(), String> {
        let mut pressed = Vec::with_capacity(chord.len());
        let mut failure = None;
        for key in chord {
            match self.0.key(key.enigo(), Direction::Press) {
                Ok(()) => pressed.push(*key),
                Err(error) => {
                    failure = Some(error.to_string());
                    break;
                }
            }
        }
        for key in pressed.iter().rev() {
            if let Err(error) = self.0.key(key.enigo(), Direction::Release) {
                failure.get_or_insert(error.to_string());
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_with_aliases() {
        assert_eq!(
            parse_chord("Ctrl+Shift+a"),
            Ok(vec![KeyName::Control, KeyName::Shift, KeyName::Char('a')])
        );
        assert_eq!(
            parse_chord("cmd + l"),
            Ok(vec![KeyName::Meta, KeyName::Char('l')])
        );
        assert_eq!(parse_chord("Enter"), Ok(vec![KeyName::Return]));
        assert_eq!(parse_chord("F5"), Ok(vec![KeyName::Function(5)]));
        assert_eq!(
            parse_chord("alt+f4"),
            Ok(vec![KeyName::Alt, KeyName::Function(4)])
        );
        assert_eq!(parse_chord("f"), Ok(vec![KeyName::Char('f')]));
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_guessed() {
        assert!(parse_chord("hyper+a").is_err());
        assert!(parse_chord("f13").is_err());
        assert!(parse_chord("").is_err());
        assert!(parse_chord("ctrl+").is_err());
    }
}
