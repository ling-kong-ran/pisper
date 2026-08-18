//! 粘贴突发（paste burst）检测：部分终端会把粘贴内容当作普通按键事件逐个
//! 发送（而不是发送 bracketed-paste 事件），这里用字符到达间隔识别这种粘贴。
//!
//! 状态机改编自 Codex CLI 的 `PasteBurst`：只有当突发进行中才捕获 Enter，
//! 因此可见粘贴后的第一个 Enter 仍能正常提交，不会误吞。
//!
//! 判定逻辑：
//! - `armed`：显式粘贴失败（读剪贴板失败）后待命，下一个字符直接视为突发；
//! - 字符间隔 ≤ `CHAR_INTERVAL` 的两个字符视为突发开始（`BeginFromPending`）；
//! - 突发活跃时所有字符（含 Enter/Tab）都追加进缓冲区；
//! - 空闲超过 `ACTIVE_IDLE_TIMEOUT` 后按内容判定：多行/含制表符/足够长 → Paste，
//!   否则视为普通输入 Typed。

use std::time::Duration;
use tokio::time::Instant;

// 判定「连续输入」的字符间隔阈值。Windows 终端事件洪峰更慢，
// 因此需要更宽裕的间隔，否则粘贴会被拆成多个普通输入。
#[cfg(not(windows))]
const CHAR_INTERVAL: Duration = Duration::from_millis(8);
#[cfg(windows)]
const CHAR_INTERVAL: Duration = Duration::from_millis(25);

// 突发活跃后认为「粘贴结束」的空闲时间；同样按平台区分。
#[cfg(not(windows))]
const ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(8);
#[cfg(windows)]
const ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(60);

/// 粘贴突发检测器状态机。
/// 关键不变量：一旦 `active` 为真，后续字符（包括 Enter）全部进缓冲区，
/// 直到超时 flush；`pending_first_char` 只保留一个候选字符，用于识别突发起点。
#[derive(Debug, Default)]
pub struct PasteBurst {
    last_char_at: Option<Instant>,
    buffer: String,
    active: bool,
    armed: bool,
    pending_first_char: Option<(char, Instant)>,
}

/// 对单个字符的判定结果。
#[derive(Debug, Eq, PartialEq)]
pub enum CharDecision {
    /// 首个字符：先保留在 `pending_first_char`，等待第二个字符确认是否突发。
    RetainFirst,
    /// 第二个字符：确认突发开始，调用方须把此前保留的首字符补进缓冲区。
    BeginFromPending,
    /// 突发已确认（或已 armed），追加到缓冲区。
    Append,
}

/// 一次 flush 的结果。
#[derive(Debug, Eq, PartialEq)]
pub enum FlushResult {
    /// 判定为粘贴：多行/含制表符/超长文本，按整块粘贴处理（可折叠显示）。
    Paste(String),
    /// 判定为普通输入：按逐字符输入处理。
    Typed(String),
    /// 无内容可 flush。
    None,
}

impl PasteBurst {
    /// 待命：下一个字符无条件视为突发开始。
    /// 用于剪贴板读取失败等场景——用户明确想要粘贴，不能因间隔过短被忽略。
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// 输入一个字符，返回调用方应如何处理。
    /// `now` 必须是单调时钟（`Instant`），避免系统时钟跳变影响间隔判定。
    pub fn on_char(&mut self, character: char, now: Instant) -> CharDecision {
        self.last_char_at = Some(now);
        if self.armed {
            self.armed = false;
            self.active = true;
            return CharDecision::Append;
        }
        if self.active {
            return CharDecision::Append;
        }
        if let Some((held, held_at)) = self.pending_first_char {
            if now.duration_since(held_at) <= CHAR_INTERVAL {
                self.active = true;
                self.pending_first_char = None;
                self.buffer.push(held);
                return CharDecision::BeginFromPending;
            }
        }
        self.pending_first_char = Some((character, now));
        CharDecision::RetainFirst
    }

    /// 把字符追加进缓冲区（应在 `on_char` 返回 Append/BeginFromPending 后调用）。
    pub fn append_char(&mut self, character: char, now: Instant) {
        self.buffer.push(character);
        self.last_char_at = Some(now);
    }

    /// 突发活跃时把 Enter 追加为换行，返回是否被吞掉。
    /// 被吞的 Enter 不触发提交——粘贴内容里的换行不应结束输入。
    pub fn append_newline_if_active(&mut self, now: Instant) -> bool {
        self.try_append_char_if_active('\n', now)
    }

    /// 突发活跃（或已 armed）时追加任意字符，返回是否被吞掉。
    pub fn try_append_char_if_active(&mut self, character: char, now: Instant) -> bool {
        if self.armed {
            self.armed = false;
            self.active = true;
        } else if !self.active_internal() {
            return false;
        }
        self.append_char(character, now);
        true
    }

    /// 是否正在缓冲（影响事件循环是否等待 flush 定时器）。
    pub fn is_buffering(&self) -> bool {
        self.armed || self.active_internal() || self.pending_first_char.is_some()
    }

    /// 下一次应触发 flush 的时刻；没有待处理输入时为 `None`。
    pub fn deadline(&self) -> Option<Instant> {
        let last = self.last_char_at?;
        Some(
            last + if self.active_internal() {
                ACTIVE_IDLE_TIMEOUT
            } else {
                CHAR_INTERVAL
            },
        )
    }

    /// 到点即 flush：超过截止时刻才真正取出内容，防止提前把正在输入的字符当粘贴。
    pub fn flush_if_due(&mut self, now: Instant) -> FlushResult {
        let Some(deadline) = self.deadline() else {
            return FlushResult::None;
        };
        if now <= deadline {
            return FlushResult::None;
        }
        self.flush()
    }

    /// 立即取出缓冲内容：突发中按内容判定 Paste/Typed；
    /// 仅有一个悬留字符时按普通输入处理；否则为 None。
    pub fn flush(&mut self) -> FlushResult {
        if self.active_internal() {
            self.active = false;
            self.last_char_at = None;
            let text = std::mem::take(&mut self.buffer);
            if text.chars().count() >= 80 || text.contains('\n') || text.contains('\t') {
                return FlushResult::Paste(text);
            }
            return FlushResult::Typed(text);
        }
        if let Some((character, _)) = self.pending_first_char.take() {
            self.last_char_at = None;
            return FlushResult::Typed(character.to_string());
        }
        FlushResult::None
    }

    /// 取消所有缓冲（例如用户按了 Esc）：清空并回到初始状态。
    pub fn cancel(&mut self) {
        self.last_char_at = None;
        self.buffer.clear();
        self.active = false;
        self.armed = false;
        self.pending_first_char = None;
    }

    /// 显式粘贴（bracketed-paste 事件或剪贴板直读）后清理状态，
    /// 避免残留缓冲把后续按键误判为新一轮粘贴。
    pub fn clear_after_explicit_paste(&mut self) {
        self.cancel();
    }

    /// 内部活跃判断：显式标记的 `active` 或缓冲区非空都算活跃。
    fn active_internal(&self) -> bool {
        self.active || !self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 arm 后立即输入的首字符进入粘贴缓冲（Append）。
    #[test]
    fn armed_paste_captures_the_first_character() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        burst.arm();
        assert_eq!(burst.on_char('a', start), CharDecision::Append);
        burst.append_char('a', start);
        assert_eq!(burst.flush(), FlushResult::Typed("a".to_owned()));
    }

    /// 验证单字符在超时后按普通键入（Typed）冲刷，而非误判为粘贴。
    #[test]
    fn one_character_flushes_as_typing() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('a', start), CharDecision::RetainFirst);
        assert!(burst.is_buffering());
        assert_eq!(
            burst.flush_if_due(start + CHAR_INTERVAL + Duration::from_millis(1)),
            FlushResult::Typed("a".to_owned())
        );
        assert!(burst.deadline().is_none());
        assert!(!burst.is_buffering());
    }

    /// 验证跨渲染帧（16ms）的连续快速输入仍能被识别为一次粘贴
    /// （Windows 终端粘贴可能跨帧到达）。
    #[cfg(windows)]
    #[test]
    fn windows_paste_can_begin_across_a_render_frame() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('a', start), CharDecision::RetainFirst);
        assert_eq!(
            burst.on_char('b', start + Duration::from_millis(16)),
            CharDecision::BeginFromPending
        );
    }

    /// 验证快速字符 + 回车合并为一次粘贴（回车作为粘贴的终止符）。
    #[test]
    fn fast_characters_and_enter_flush_as_one_paste() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('a', start), CharDecision::RetainFirst);
        assert_eq!(
            burst.on_char('b', start + Duration::from_millis(1)),
            CharDecision::BeginFromPending
        );
        burst.append_char('b', start + Duration::from_millis(1));
        assert!(burst.append_newline_if_active(start + Duration::from_millis(2)));
        assert_eq!(
            burst.flush_if_due(start + ACTIVE_IDLE_TIMEOUT + Duration::from_millis(4)),
            FlushResult::Paste("ab\n".to_owned())
        );
    }

    /// 验证 Unicode（多字节）快速输入也在回车前被识别为粘贴并完整冲刷。
    #[test]
    fn unicode_characters_activate_the_paste_before_enter() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('你', start), CharDecision::RetainFirst);
        assert_eq!(
            burst.on_char('好', start + Duration::from_millis(1)),
            CharDecision::BeginFromPending
        );
        burst.append_char('好', start + Duration::from_millis(1));
        assert!(burst.append_newline_if_active(start + Duration::from_millis(2)));
        assert_eq!(burst.flush(), FlushResult::Paste("你好\n".to_owned()));
    }

    /// 验证短而快的 Unicode 文本按普通键入处理（未达到粘贴判定阈值）。
    #[test]
    fn short_fast_unicode_text_remains_normal_input() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('一', start), CharDecision::RetainFirst);
        assert_eq!(
            burst.on_char('段', start + Duration::from_millis(1)),
            CharDecision::BeginFromPending
        );
        burst.append_char('段', start + Duration::from_millis(1));
        assert_eq!(burst.flush(), FlushResult::Typed("一段".to_owned()));
    }

    /// 验证粘贴冲刷后回车不再被捕获（后续键入按普通输入处理）。
    #[test]
    fn enter_is_not_captured_after_burst_flush() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        burst.on_char('a', start);
        burst.on_char('b', start + Duration::from_millis(1));
        burst.append_char('b', start + Duration::from_millis(1));
        let flushed_at = start + ACTIVE_IDLE_TIMEOUT + Duration::from_millis(2);
        assert!(matches!(
            burst.flush_if_due(flushed_at),
            FlushResult::Typed(_)
        ));
        assert!(!burst.append_newline_if_active(flushed_at));
    }
}
