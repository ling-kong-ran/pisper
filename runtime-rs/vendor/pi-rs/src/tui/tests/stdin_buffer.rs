//! Ports of upstream `packages/tui/test/stdin-buffer.test.ts`.
//!
//! Upstream flush timers become explicit `flush_after_ms` assertions plus a
//! deterministic `flush_emit()` call; the observable event order matches.

use crate::tui::keys::matches_key;
use crate::tui::stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};

/// Collect only the `data` events (the `paste` cases assert separately).
fn data_of(outcome: &mut Vec<StdinEvent>) -> Vec<String> {
    let mut result = Vec::new();
    for event in outcome.drain(..) {
        match event {
            StdinEvent::Data(sequence) => result.push(sequence),
            StdinEvent::Paste(_) => {}
        }
    }
    result
}

#[test]
fn regular_characters_pass_through_immediately() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    emitted.extend(data_of(&mut buffer.process("a").events));
    assert_eq!(emitted, vec!["a"]);
}

#[test]
fn regular_multiple_characters_pass_through() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    emitted.extend(data_of(&mut buffer.process("abc").events));
    assert_eq!(emitted, vec!["a", "b", "c"]);
}

#[test]
fn unicode_characters_pass_through_one_char_each() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    emitted.extend(data_of(&mut buffer.process("hello 世界").events));
    assert_eq!(emitted, vec!["h", "e", "l", "l", "o", " ", "世", "界"]);
}

#[test]
fn complete_escape_sequences_pass_through() {
    let mut buffer = StdinBuffer::default();
    for (input, expected) in [
        ("\x1b[<35;20;5m", "\x1b[<35;20;5m"),
        ("\x1b[A", "\x1b[A"),
        ("\x1b[11~", "\x1b[11~"),
        ("\x1ba", "\x1ba"),
        ("\x1bOA", "\x1bOA"),
    ] {
        let mut emitted = Vec::new();
        emitted.extend(data_of(&mut buffer.process(input).events));
        assert_eq!(emitted, vec![expected], "input {input:?}");
    }
}

#[test]
fn incomplete_mouse_sgr_sequence_buffers_across_chunks() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions {
        timeout_ms: Some(10),
        ..Default::default()
    });
    let mut emitted = Vec::new();

    let outcome = buffer.process("\x1b");
    assert!(outcome.events.is_empty());
    assert_eq!(buffer.get_buffer(), "\x1b");

    let outcome = buffer.process("[<35");
    assert!(outcome.events.is_empty());
    assert_eq!(buffer.get_buffer(), "\x1b[<35");

    let outcome = buffer.process(";20;5m");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[<35;20;5m"]);
    assert_eq!(buffer.get_buffer(), "");
}

#[test]
fn incomplete_csi_sequence_buffers() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();

    let outcome = buffer.process("\x1b[");
    assert!(outcome.events.is_empty());
    let outcome = buffer.process("1;");
    assert!(outcome.events.is_empty());
    let outcome = buffer.process("5H");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[1;5H"]);
}

#[test]
fn split_across_many_chunks() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    for chunk in ["\x1b", "[", "<", "3", "5", ";", "2", "0", ";", "5", "m"] {
        emitted.extend(data_of(&mut buffer.process(chunk).events));
    }
    assert_eq!(emitted, vec!["\x1b[<35;20;5m"]);
}

#[test]
fn flush_incomplete_sequence_after_timeout() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions {
        timeout_ms: Some(10),
        ..Default::default()
    });
    let outcome = buffer.process("\x1b[<35");
    assert!(outcome.events.is_empty());
    assert_eq!(outcome.flush_after_ms, Some(10));

    // The scheduled timeout fires: upstream emits via emitDataSequence.
    let emitted = data_of(&mut buffer.flush_emit());
    assert_eq!(emitted, vec!["\x1b[<35"]);
}

#[test]
fn flush_lone_esc_as_escape_when_cr_arrives_after_timeout() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions {
        timeout_ms: Some(10),
        ..Default::default()
    });
    let outcome = buffer.process("\x1b");
    assert!(outcome.events.is_empty());
    assert_eq!(outcome.flush_after_ms, Some(10)); // escape timeout, not 10ms sequence? equals here

    // The escape timeout fires before CR arrives: ESC flushes alone.
    let mut emitted = data_of(&mut buffer.flush_emit());
    assert_eq!(emitted, vec!["\x1b"]);

    let outcome = buffer.process("\r");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b", "\r"]);
    assert!(matches_key(&emitted[0].clone(), "escape"));
}

#[test]
fn merge_esc_cr_split_across_chunks_within_larger_timeout() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions {
        escape_timeout_ms: Some(100),
        ..Default::default()
    });
    let outcome = buffer.process("\x1b");
    assert!(outcome.events.is_empty());
    assert_eq!(outcome.flush_after_ms, Some(100));

    // Within the configured escape window the CR arrives and merges.
    let mut emitted = Vec::new();
    let outcome = buffer.process("\r");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b\r"]);
    assert!(matches_key(&emitted[0].clone(), "alt+enter"));
}

#[test]
fn sequence_timeout_does_not_apply_to_lone_esc() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions {
        timeout_ms: Some(100),
        ..Default::default()
    });
    let outcome = buffer.process("\x1b");
    assert!(outcome.events.is_empty());
    // A lone ESC uses the (shorter) escape timeout, not the sequence timeout.
    assert_eq!(outcome.flush_after_ms, Some(10));

    let mut emitted = data_of(&mut buffer.flush_emit());
    assert_eq!(emitted, vec!["\x1b"]);
    let outcome = buffer.process("\r");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b", "\r"]);
    assert!(matches_key(&emitted[0].clone(), "escape"));
}

#[test]
fn fragmented_mouse_sequences_stay_buffered_with_default_timeout() {
    let mut buffer = StdinBuffer::default();
    let outcome = buffer.process("\x1b[");
    assert!(outcome.events.is_empty());
    assert_eq!(outcome.flush_after_ms, Some(50));

    let mut emitted = Vec::new();
    let outcome = buffer.process("<65;48;39M");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[<65;48;39M"]);
}

#[test]
fn mixed_content_orders_characters_and_sequences() {
    // Upstream's beforeEach gives each case a fresh buffer; mirror that.
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process("abc\x1b[A");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["a", "b", "c", "\x1b[A"]);

    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process("\x1b[Aabc");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[A", "a", "b", "c"]);

    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process("\x1b[A\x1b[B\x1b[C");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[A", "\x1b[B", "\x1b[C"]);
}

#[test]
fn partial_sequence_with_preceding_characters() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();

    let outcome = buffer.process("abc\x1b[<35");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["a", "b", "c"]);
    assert_eq!(buffer.get_buffer(), "\x1b[<35");

    let outcome = buffer.process(";20;5m");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["a", "b", "c", "\x1b[<35;20;5m"]);
}

#[test]
fn kitty_csi_u_press_release_and_batches() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();

    let outcome = buffer.process("\x1b[97u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[97u"]);

    let outcome = buffer.process("\x1b[97;1:3u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.last().map(String::as_str), Some("\x1b[97;1:3u"));

    let outcome = buffer.process("\x1b[97u\x1b[97;1:3u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(
        emitted[emitted.len() - 2..],
        ["\x1b[97u".to_string(), "\x1b[97;1:3u".to_string()]
    );

    let outcome = buffer.process("\x1b[97u\x1b[97;1:3u\x1b[98u\x1b[98;1:3u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.len(), 8);
}

#[test]
fn kitty_arrow_and_functional_keys_with_event_type() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process("\x1b[1;1:1A");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[1;1:1A"]);
    let outcome = buffer.process("\x1b[3;1:3~");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.last().map(String::as_str), Some("\x1b[3;1:3~"));
}

#[test]
fn wezterm_escape_press_then_kitty_release_splits() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process("\x1b\x1b[27;129:3u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b", "\x1b[27;129:3u"]);

    let outcome = buffer.process("\x1b\x1b[27;1:3u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.last().map(String::as_str), Some("\x1b[27;1:3u"));
}

#[test]
fn esc_esc_alone_stays_single_sequence() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process("\x1b\x1b");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b\x1b"]);
}

#[test]
fn kitty_printable_dedup_interactions() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();

    // Plain 'a' followed by Kitty release.
    let outcome = buffer.process("a\x1b[97;1:3u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["a", "\x1b[97;1:3u"]);

    // Drop raw duplicate after a matching Kitty printable sequence.
    let outcome = buffer.process("\x1b[224uà");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.last().map(String::as_str), Some("\x1b[224u"));

    // ... including across chunks.
    let outcome = buffer.process("\x1b[64u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    let outcome = buffer.process("@");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.last().map(String::as_str), Some("\x1b[64u"));

    // Keep a non-matching plain character.
    let outcome = buffer.process("\x1b[97ub");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.last().map(String::as_str), Some("b"));

    // Keep the raw character after a MODIFIED Kitty printable sequence.
    let outcome = buffer.process("\x1b[64;3u@");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.last().map(String::as_str), Some("@"));
}

#[test]
fn rapid_typing_simulation_with_kitty_protocol() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process("\x1b[104u\x1b[104;1:3u\x1b[105u\x1b[105;1:3u");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(
        emitted,
        vec!["\x1b[104u", "\x1b[104;1:3u", "\x1b[105u", "\x1b[105;1:3u"]
    );
}

#[test]
fn mouse_events_press_release_move_and_splits() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();

    for (input, expected) in [
        ("\x1b[<0;10;5M", "\x1b[<0;10;5M"),
        ("\x1b[<0;10;5m", "\x1b[<0;10;5m"),
        ("\x1b[<35;20;5m", "\x1b[<35;20;5m"),
    ] {
        let outcome = buffer.process(input);
        emitted.extend(data_of(&mut outcome.events.clone()));
        assert_eq!(emitted.last().map(String::as_str), Some(expected));
    }

    for chunk in ["\x1b[<3", "5;1", "5;", "10m"] {
        emitted.extend(data_of(&mut buffer.process(chunk).events));
    }
    assert_eq!(emitted.last().map(String::as_str), Some("\x1b[<35;15;10m"));

    let outcome = buffer.process("\x1b[<35;1;1m\x1b[<35;2;2m\x1b[<35;3;3m");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(
        emitted[emitted.len() - 3..],
        [
            "\x1b[<35;1;1m".to_string(),
            "\x1b[<35;2;2m".to_string(),
            "\x1b[<35;3;3m".to_string()
        ]
    );
}

#[test]
fn old_style_mouse_sequence_needs_three_trailing_bytes() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();

    let outcome = buffer.process("\x1b[M abc");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[M ab", "c"]);

    let outcome = buffer.process("\x1b[M");
    assert!(outcome.events.is_empty());
    assert_eq!(buffer.get_buffer(), "\x1b[M");
    let outcome = buffer.process(" a");
    assert!(outcome.events.is_empty());
    assert_eq!(buffer.get_buffer(), "\x1b[M a");
    let outcome = buffer.process("b");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted.last().map(String::as_str), Some("\x1b[M ab"));
}

#[test]
fn empty_input_emits_empty_data_event() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process("");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec![""]);
}

#[test]
fn lone_escape_with_explicit_flush() {
    let mut buffer = StdinBuffer::default();
    let outcome = buffer.process("\x1b");
    assert!(outcome.events.is_empty());
    assert_eq!(buffer.flush(), vec!["\x1b"]);
}

#[test]
fn buffer_bytes_input() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let outcome = buffer.process_bytes(b"\x1b[A");
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec!["\x1b[A"]);
}

#[test]
fn very_long_sequences() {
    let mut buffer = StdinBuffer::default();
    let long_seq = format!("\x1b[{}H", "1;".repeat(50));
    let mut emitted = Vec::new();
    let outcome = buffer.process(&long_seq);
    emitted.extend(data_of(&mut outcome.events.clone()));
    assert_eq!(emitted, vec![long_seq]);
}

#[test]
fn flush_returns_empty_when_nothing_buffered() {
    let mut buffer = StdinBuffer::default();
    assert!(buffer.flush().is_empty());
}

#[test]
fn clear_discards_buffered_content_without_emitting() {
    let mut buffer = StdinBuffer::default();
    let outcome = buffer.process("\x1b[<35");
    assert!(outcome.events.is_empty());
    assert_eq!(buffer.get_buffer(), "\x1b[<35");
    buffer.clear();
    assert_eq!(buffer.get_buffer(), "");
}

#[test]
fn bracketed_paste_complete_and_chunked() {
    let mut buffer = StdinBuffer::default();

    let outcome = buffer.process("\x1b[200~hello world\x1b[201~");
    assert_eq!(
        outcome.events,
        vec![StdinEvent::Paste("hello world".to_string())]
    );

    let mut pastes = Vec::new();
    let outcome = buffer.process("\x1b[200~");
    assert!(outcome.events.is_empty());
    let outcome = buffer.process("hello ");
    assert!(outcome.events.is_empty());
    let outcome = buffer.process("world\x1b[201~");
    for event in outcome.events {
        if let StdinEvent::Paste(content) = event {
            pastes.push(content);
        }
    }
    assert_eq!(pastes, vec!["hello world"]);
}

#[test]
fn bracketed_paste_with_input_before_and_after() {
    let mut buffer = StdinBuffer::default();
    let mut emitted = Vec::new();
    let mut pastes = Vec::new();

    emitted.extend(data_of(&mut buffer.process("a").events));
    for event in buffer.process("\x1b[200~pasted\x1b[201~").events {
        match event {
            StdinEvent::Paste(content) => pastes.push(content),
            other => emitted.push(match other {
                StdinEvent::Data(sequence) => sequence,
                StdinEvent::Paste(_) => unreachable!(),
            }),
        }
    }
    emitted.extend(data_of(&mut buffer.process("b").events));

    assert_eq!(emitted, vec!["a", "b"]);
    assert_eq!(pastes, vec!["pasted"]);
}

#[test]
fn bracketed_paste_with_newlines_and_unicode() {
    let mut buffer = StdinBuffer::default();
    let mut pastes = Vec::new();
    for event in buffer
        .process("\x1b[200~line1\nline2\nline3\x1b[201~")
        .events
    {
        if let StdinEvent::Paste(content) = event {
            pastes.push(content);
        }
    }
    assert_eq!(pastes, vec!["line1\nline2\nline3"]);

    let mut pastes = Vec::new();
    for event in buffer.process("\x1b[200~Hello 世界 🎉\x1b[201~").events {
        if let StdinEvent::Paste(content) = event {
            pastes.push(content);
        }
    }
    assert_eq!(pastes, vec!["Hello 世界 🎉"]);
}

#[test]
fn destroy_clears_buffer() {
    let mut buffer = StdinBuffer::default();
    let outcome = buffer.process("\x1b[<35");
    assert!(outcome.events.is_empty());
    buffer.destroy();
    assert_eq!(buffer.get_buffer(), "");
}
