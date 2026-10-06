//! Sanity checks for the output-capture subset (the full
//! `test/harness/output-capture.test.ts` oracle lands with M3b Task 11).

use std::sync::Arc;

use super::*;

fn capture(limits: Option<ShellOutputLimits>) -> OutputCapture {
    OutputCapture::new(
        limits
            .map(|limits| ShellOutputCaptureOptions {
                limits,
                spill: None,
            })
            .as_ref(),
        Context::background(),
        OutputCaptureHandlers {
            on_update: None,
            on_error: Arc::new(|_| {}),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn pushes_bytes_into_a_bounded_view_and_reports_totals() {
    let capture = capture(Some(ShellOutputLimits {
        max_bytes: 100,
        max_lines: 10,
        retain: None,
    }));
    capture.push(b"hello ");
    capture.push(&[0x77, 0x6f, 0x72, 0x6c, 0x64]); // "world"
    let view = capture.snapshot();
    assert_eq!(view.text, "hello world");
    assert!(!view.metadata.truncation.truncated);
    assert_eq!(view.metadata.truncation.total_bytes, 11);
    assert_eq!(view.metadata.truncation.total_lines, 1);
    assert_eq!(view.metadata.spill_path, None);
}

#[test]
fn strips_control_characters_but_keeps_tab_and_newline() {
    assert_eq!(
        sanitize_shell_output("a\u{0}b\u{1f}c\td\ne\u{b}f\r"),
        "abc\td\nef"
    );
    assert_eq!(sanitize_shell_output("x\u{fff9}y\u{fffb}z"), "xyz");
    assert_eq!(sanitize_shell_output("ok"), "ok");
}

#[tokio::test]
async fn reports_truncation_metadata_once_a_limit_is_crossed() {
    let capture = capture(Some(ShellOutputLimits {
        max_bytes: 8,
        max_lines: 2,
        retain: Some(ShellOutputRetention::Tail),
    }));
    capture.push(b"one\ntwo\nthree");
    let view = capture.snapshot();
    assert!(view.metadata.truncation.truncated);
    assert_eq!(
        view.metadata.truncation.truncated_by,
        Some(TruncatedBy::Lines)
    );
    assert_eq!(view.metadata.truncation.total_lines, 3);
    assert_eq!(view.metadata.truncation.output_lines, 1);
    assert_eq!(view.metadata.truncation.total_bytes, 13);
    assert_eq!(view.text, "three");
}

#[test]
fn constructor_rejects_non_positive_limits_with_the_upstream_messages() {
    let error = OutputCapture::new(
        Some(&ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 0,
                max_lines: 10,
                retain: None,
            },
            spill: None,
        }),
        Context::background(),
        OutputCaptureHandlers {
            on_update: None,
            on_error: Arc::new(|_| {}),
        },
    )
    .unwrap_err();
    assert_eq!(error, "Output maxBytes must be a positive finite number");

    let error = OutputCapture::new(
        Some(&ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 100,
                max_lines: 0,
                retain: None,
            },
            spill: None,
        }),
        Context::background(),
        OutputCaptureHandlers {
            on_update: None,
            on_error: Arc::new(|_| {}),
        },
    )
    .unwrap_err();
    assert_eq!(error, "Output maxLines must be a positive integer");
}

#[tokio::test]
async fn publishes_replace_first_then_appends_without_duplicate_publications() {
    let seen: Arc<Mutex<Vec<ShellOutputUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_writer = Arc::clone(&seen);
    let capture = OutputCapture::new(
        Some(&ShellOutputCaptureOptions {
            limits: ShellOutputLimits {
                max_bytes: 1024,
                max_lines: 10,
                retain: None,
            },
            spill: None,
        }),
        Context::background(),
        OutputCaptureHandlers {
            on_update: Some(Arc::new(
                move |update: ShellOutputUpdate, _context: &Context| {
                    seen_writer.lock().unwrap().push(update);
                },
            )),
            on_error: Arc::new(|_| {}),
        },
    )
    .unwrap();
    capture.push(b"one");
    capture.push(b" two");
    // The first publication is immediate; the second is rate-limited onto the
    // trailing timer. The dirty gate must keep the total at exactly two.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    capture.flush();
    let updates_seen: Vec<ShellOutputUpdate> = seen.lock().unwrap().drain(..).collect();
    assert_eq!(updates_seen.len(), 2, "updates: {updates_seen:?}");
    match &updates_seen[0] {
        ShellOutputUpdate::Replace { output } => assert_eq!(output.text, "one"),
        other => panic!("expected first update replace, got {other:?}"),
    }
    match &updates_seen[1] {
        ShellOutputUpdate::Append { text, .. } => assert_eq!(text, " two"),
        other => panic!("expected second update append, got {other:?}"),
    }
}

#[test]
fn apply_shell_output_update_folds_replace_append_slide_metadata() {
    let view = apply_shell_output_update(
        None,
        ShellOutputUpdate::Replace {
            output: ShellOutputView {
                metadata: ShellOutputMetadata {
                    truncation: Default::default(),
                    spill_path: None,
                    last_line_bytes: None,
                },
                text: "abc".into(),
            },
        },
    );
    assert_eq!(view.text, "abc");

    let view = apply_shell_output_update(
        Some(view),
        ShellOutputUpdate::Append {
            text: "def".into(),
            metadata: ShellOutputMetadata {
                truncation: Default::default(),
                spill_path: None,
                last_line_bytes: None,
            },
        },
    );
    assert_eq!(view.text, "abcdef");

    let view = apply_shell_output_update(
        Some(view),
        ShellOutputUpdate::Slide {
            drop: 2,
            text: "XY".into(),
            metadata: ShellOutputMetadata {
                truncation: Default::default(),
                spill_path: None,
                last_line_bytes: None,
            },
        },
    );
    assert_eq!(view.text, "cdefXY");

    let view = apply_shell_output_update(
        Some(view),
        ShellOutputUpdate::Metadata {
            metadata: ShellOutputMetadata {
                truncation: Default::default(),
                spill_path: Some("spill.log".into()),
                last_line_bytes: None,
            },
        },
    );
    assert_eq!(view.text, "cdefXY");
    assert_eq!(view.metadata.spill_path.as_deref(), Some("spill.log"));
}
