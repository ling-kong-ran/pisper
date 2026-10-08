use super::*;
fn from_hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn trace(acc: &mut OutputAccumulator, persist: bool) -> Value {
    let mut snapshot = acc.snapshot(persist).unwrap().as_value();
    if snapshot.get("fullOutputPath").is_some() {
        snapshot["fullOutputPath"] = json!("$SPILL");
    }
    json!({"snapshot":snapshot,"lastLineBytes":acc.get_last_line_bytes()})
}
#[test]
fn snapshots_streaming_utf8_truncation_and_raw_spills_match_real_upstream() {
    let oracle: Value =
        serde_json::from_str(include_str!("output_accumulator_oracle.json")).unwrap();
    let cases = oracle["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 162);
    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let mut accumulator = OutputAccumulator::with_temp_directory(
            OutputAccumulatorOptions {
                max_lines: case["options"]["maxLines"].as_u64(),
                max_bytes: case["options"]["maxBytes"].as_u64(),
                temp_file_prefix: Some("fixture-output".into()),
            },
            dir.path(),
        );
        let mut actual = vec![trace(&mut accumulator, false)];
        for (index, chunk) in case["chunks"].as_array().unwrap().iter().enumerate() {
            accumulator
                .append(&from_hex(chunk.as_str().unwrap()))
                .unwrap();
            actual.push(trace(&mut accumulator, index % 2 == 0));
        }
        accumulator.finish().unwrap();
        actual.push(trace(&mut accumulator, true));
        accumulator.finish().unwrap();
        actual.push(trace(&mut accumulator, false));
        assert_eq!(json!(actual), case["trace"], "{}", case["id"]);
        assert_eq!(
            accumulator.append(b"late").unwrap_err().to_string(),
            case["appendAfterFinish"].as_str().unwrap()
        );
        accumulator.close_temp_file().unwrap();
        accumulator.close_temp_file().unwrap();
        let files = fs::read_dir(dir.path())
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        if let Some(expected) = case["fullOutputHex"].as_str() {
            assert_eq!(files.len(), 1, "{}", case["id"]);
            assert_eq!(
                fs::read(files[0].path()).unwrap(),
                from_hex(expected),
                "{} raw spill",
                case["id"]
            );
        } else {
            assert!(files.is_empty(), "{} unexpected spill", case["id"]);
        }
    }
}
#[test]
fn bounded_storage_keeps_spill_after_drop_and_randomizes_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut acc = OutputAccumulator::with_temp_directory(
        OutputAccumulatorOptions {
            max_bytes: Some(100),
            ..Default::default()
        },
        dir.path(),
    );
    let chunk = "a🙂b\n".repeat(200);
    for _ in 0..100 {
        acc.append(chunk.as_bytes()).unwrap();
        assert!(acc.raw_chunks.is_empty());
        assert!(acc.tail_bytes <= 400);
        assert!(acc.tail_text.capacity() <= 400);
        assert!(acc.decoder.pending.len() <= 3);
    }
    acc.finish().unwrap();
    let snapshot = acc.snapshot(true).unwrap();
    assert_eq!(snapshot.truncation.total_lines, 20_000);
    assert_eq!(snapshot.truncation.total_bytes, (chunk.len() * 100) as u64);
    let path = snapshot.full_output_path.unwrap();
    let name = path.file_name().unwrap().to_str().unwrap();
    let random = name
        .strip_prefix("pi-output-")
        .unwrap()
        .strip_suffix(".log")
        .unwrap();
    assert_eq!(random.len(), 16);
    assert!(random
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    acc.close_temp_file().unwrap();
    drop(acc);
    assert_eq!(fs::read(&path).unwrap(), chunk.repeat(100).into_bytes());
}
#[test]
fn spill_creation_errors_propagate_without_success_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut acc = OutputAccumulator::with_temp_directory(
        OutputAccumulatorOptions {
            max_bytes: Some(1),
            ..Default::default()
        },
        dir.path().join("absent-parent"),
    );
    assert_eq!(
        acc.append(b"long").unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert!(acc.temp_file_path.is_none());
    acc.close_temp_file().unwrap();
}
