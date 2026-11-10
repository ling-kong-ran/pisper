//! 被当成按键送来的鼠标上报：有些环境（Windows 的 cmd / 终端宿主）会把鼠标的移动、点击、滚轮
//! 编码成 `ESC [ < 按钮 ; 列 ; 行 M`（SGR）或 `ESC [ M` 加三个字节（X10）的字符序列，逐个字符
//! 作为按键事件送进来，crossterm 在 Windows 上不会把它们解析成鼠标事件。不拦的话，开头的 Esc
//! 会取消正在做的事，后面的 `[<35;54;20M` 被当成输入进了输入框，鼠标一动就一串一串地攒。
//!
//! 这里在按键进入输入处理之前认出这种序列：认出来就还原成鼠标事件（滚轮、点击照常可用），
//! 认到一半发现不是，就把扣住的按键原样放行。真的按 Esc 只会被扣住 `HOLD` 这么久。

use std::time::Duration;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use tokio::time::Instant;

/// Esc 之后等后续字符的时间：序列里的字符是一起到的，人按完 Esc 再打字不会这么快。
const HOLD: Duration = Duration::from_millis(30);
/// SGR 序列最长也就 `[<255;9999;9999M`；超过这个长度还没结束就不是鼠标上报。
const MAX_SEQUENCE_CHARS: usize = 24;

#[derive(Debug, PartialEq)]
pub enum Filtered {
    Key(KeyEvent),
    Mouse(MouseEvent),
}

#[derive(Debug, Default)]
pub struct VtMouseFilter {
    /// 扣住的按键：第一个一定是 Esc，后面是序列里已经到的字符。
    held: Vec<KeyEvent>,
    started: Option<Instant>,
}

enum Parsed {
    /// 还可能是鼠标上报，继续等。
    Partial,
    Complete(MouseEvent),
    /// 不是鼠标上报。
    Mismatch,
}

impl VtMouseFilter {
    /// 喂一个按键，返回这次可以放行的事件（可能为空：还在等序列的后续）。
    pub fn feed(&mut self, key: KeyEvent, now: Instant) -> Vec<Filtered> {
        // 松开事件不参与识别：被吞掉的字符的松开事件没有对应的按下，后面的 ReleaseOnlyChars 会丢掉它。
        if key.kind == KeyEventKind::Release {
            return vec![Filtered::Key(key)];
        }
        if self.held.is_empty() {
            if key.code == KeyCode::Esc && key.modifiers.is_empty() {
                self.held.push(key);
                self.started = Some(now);
                return Vec::new();
            }
            return vec![Filtered::Key(key)];
        }
        let plain_char = matches!(key.code, KeyCode::Char(_))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if !plain_char {
            // 序列里只有普通字符。来了别的键：先放行扣住的，再照常处理这个键（它自己可能又是一个 Esc）。
            let mut released = self.release();
            released.extend(self.feed(key, now));
            return released;
        }
        self.held.push(key);
        match parse(&self.sequence()) {
            Parsed::Partial => Vec::new(),
            Parsed::Complete(mouse) => {
                self.held.clear();
                self.started = None;
                vec![Filtered::Mouse(mouse)]
            }
            Parsed::Mismatch => self.release(),
        }
    }

    /// 扣住的按键最晚什么时候放行。
    pub fn deadline(&self) -> Option<Instant> {
        self.started.map(|started| started + HOLD)
    }

    /// 等够了还没凑成序列：就是一次真的 Esc（或不完整的残片），原样放行。
    pub fn flush_if_due(&mut self, now: Instant) -> Vec<Filtered> {
        match self.deadline() {
            Some(deadline) if now >= deadline => self.release(),
            _ => Vec::new(),
        }
    }

    fn release(&mut self) -> Vec<Filtered> {
        self.started = None;
        self.held.drain(..).map(Filtered::Key).collect()
    }

    /// Esc 之后已经到的字符。
    fn sequence(&self) -> Vec<char> {
        self.held
            .iter()
            .skip(1)
            .filter_map(|key| match key.code {
                KeyCode::Char(character) => Some(character),
                _ => None,
            })
            .collect()
    }
}

fn parse(sequence: &[char]) -> Parsed {
    if sequence.len() > MAX_SEQUENCE_CHARS {
        return Parsed::Mismatch;
    }
    match sequence {
        [] => Parsed::Partial,
        ['['] => Parsed::Partial,
        ['[', '<', rest @ ..] => parse_sgr(rest),
        ['[', 'M', rest @ ..] => parse_x10(rest),
        _ => Parsed::Mismatch,
    }
}

/// `按钮;列;行` 后跟 `M`（按下、移动、滚轮）或 `m`（松开）。
fn parse_sgr(rest: &[char]) -> Parsed {
    let Some((last, body)) = rest.split_last() else {
        return Parsed::Partial;
    };
    let finished = matches!(last, 'M' | 'm');
    let digits = if finished { body } else { rest };
    if !digits
        .iter()
        .all(|character| character.is_ascii_digit() || *character == ';')
    {
        return Parsed::Mismatch;
    }
    let text: String = digits.iter().collect();
    let fields: Vec<&str> = text.split(';').collect();
    if fields.len() > 3 {
        return Parsed::Mismatch;
    }
    if !finished {
        return Parsed::Partial;
    }
    let numbers: Vec<u16> = fields
        .iter()
        .filter_map(|field| field.parse::<u16>().ok())
        .collect();
    let [code, column, row] = numbers[..] else {
        return Parsed::Mismatch;
    };
    Parsed::Complete(mouse_event(code, column, row, *last == 'm'))
}

/// 旧式上报：三个字节，各自减 32 得到按钮、列、行。
fn parse_x10(rest: &[char]) -> Parsed {
    if rest.len() < 3 {
        return Parsed::Partial;
    }
    let [code, column, row] = rest[..] else {
        return Parsed::Mismatch;
    };
    let value = |character: char| (character as u32).saturating_sub(32) as u16;
    let code = value(code);
    // 旧式没有单独的“松开”结尾，按钮位全 1 表示松开。
    Parsed::Complete(mouse_event(
        code,
        value(column),
        value(row),
        code & 0b11 == 0b11,
    ))
}

fn mouse_event(code: u16, column: u16, row: u16, released: bool) -> MouseEvent {
    let mut modifiers = KeyModifiers::NONE;
    if code & 4 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if code & 8 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if code & 16 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    let button = match code & 0b11 {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        _ => MouseButton::Right,
    };
    let kind = if code & 64 != 0 {
        match code & 0b11 {
            0 => MouseEventKind::ScrollUp,
            1 => MouseEventKind::ScrollDown,
            2 => MouseEventKind::ScrollLeft,
            _ => MouseEventKind::ScrollRight,
        }
    } else if code & 32 != 0 {
        if code & 0b11 == 0b11 {
            MouseEventKind::Moved
        } else {
            MouseEventKind::Drag(button)
        }
    } else if released {
        MouseEventKind::Up(button)
    } else {
        MouseEventKind::Down(button)
    };
    MouseEvent {
        kind,
        // 上报的坐标从 1 开始。
        column: column.saturating_sub(1),
        row: row.saturating_sub(1),
        modifiers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn feed_all(filter: &mut VtMouseFilter, text: &str, now: Instant) -> Vec<Filtered> {
        let mut output = filter.feed(press(KeyCode::Esc), now);
        for character in text.chars() {
            output.extend(filter.feed(press(KeyCode::Char(character)), now));
        }
        output
    }

    #[tokio::test]
    async fn mouse_reports_typed_as_keys_become_mouse_events_not_text() {
        let now = Instant::now();
        let mut filter = VtMouseFilter::default();
        // 移动、滚轮、点击、松开：一个字符都不放进输入。
        assert_eq!(
            feed_all(&mut filter, "[<35;54;20M", now),
            vec![Filtered::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: 53,
                row: 19,
                modifiers: KeyModifiers::NONE,
            })]
        );
        let kinds = |output: Vec<Filtered>| match output.as_slice() {
            [Filtered::Mouse(mouse)] => mouse.kind,
            other => panic!("expected one mouse event, got {other:?}"),
        };
        assert_eq!(
            kinds(feed_all(&mut filter, "[<64;10;5M", now)),
            MouseEventKind::ScrollUp
        );
        assert_eq!(
            kinds(feed_all(&mut filter, "[<65;10;5M", now)),
            MouseEventKind::ScrollDown
        );
        assert_eq!(
            kinds(feed_all(&mut filter, "[<0;3;4M", now)),
            MouseEventKind::Down(MouseButton::Left)
        );
        assert_eq!(
            kinds(feed_all(&mut filter, "[<0;3;4m", now)),
            MouseEventKind::Up(MouseButton::Left)
        );
        assert_eq!(
            kinds(feed_all(&mut filter, "[M #$", now)),
            MouseEventKind::Down(MouseButton::Left)
        );
        assert_eq!(filter.deadline(), None);
    }

    #[tokio::test]
    async fn a_real_escape_is_only_held_briefly_and_other_keys_pass_through() {
        let now = Instant::now();
        let mut filter = VtMouseFilter::default();
        assert_eq!(
            filter.feed(press(KeyCode::Char('a')), now),
            vec![Filtered::Key(press(KeyCode::Char('a')))]
        );
        assert!(filter.feed(press(KeyCode::Esc), now).is_empty());
        assert!(filter.flush_if_due(now).is_empty());
        assert_eq!(
            filter.flush_if_due(now + HOLD),
            vec![Filtered::Key(press(KeyCode::Esc))]
        );
        // Esc 后面跟的不是鼠标上报：扣住的都原样放行，顺序不变。
        assert_eq!(
            feed_all(&mut filter, "[A", now),
            vec![
                Filtered::Key(press(KeyCode::Esc)),
                Filtered::Key(press(KeyCode::Char('['))),
                Filtered::Key(press(KeyCode::Char('A'))),
            ]
        );
        assert!(filter.feed(press(KeyCode::Esc), now).is_empty());
        assert_eq!(
            filter.feed(press(KeyCode::Enter), now),
            vec![
                Filtered::Key(press(KeyCode::Esc)),
                Filtered::Key(press(KeyCode::Enter)),
            ]
        );
        // 连按两次 Esc：第一次放行，第二次接着等。
        assert!(filter.feed(press(KeyCode::Esc), now).is_empty());
        assert_eq!(
            filter.feed(press(KeyCode::Esc), now),
            vec![Filtered::Key(press(KeyCode::Esc))]
        );
        assert!(filter.deadline().is_some());
    }
}
