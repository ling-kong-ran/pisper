import fs from 'node:fs';
const p = 'src/ai/retry.rs';
let t = fs.readFileSync(p, 'utf8');
const eol = t.includes('\r\n') ? '\r\n' : '\n';
const from = [
  '    async fn retry_after_past_date_or_garbage_uses_exponential_backoff() {',
  '        for header in ["Sat, 01 Jan 2000 00:00:00 GMT", "garbage"] {',
  '            let (result, attempts, elapsed) = drive(',
  '                1,',
  '                None,',
  '                vec[',
  '                    Err(provider_error(429, &[("retry-after", header)])),',
  '                    Ok("ok"),',
  '                ],',
  '            )',
  '            .await;',
  '            assert_eq!(result, Ok("ok"), "header {header}");',
  '            assert_eq!(attempts, 2, "header {header}");',
  '            assert!(',
  '                (350..550).contains(&elapsed),',
  '                "header {header} slept {elapsed}ms (expected the 375-500ms exponential band)"',
  '            );',
  '        }',
  '    }',
].join(eol);
const to = [
  '    async fn retry_after_past_date_sleeps_immediately_and_garbage_uses_exponential_backoff() {',
  '        // Past date: a negative (finite) delay still returns; the sleep',
  '        // clamps to zero, so the retry is immediate.',
  '        let (result, attempts, elapsed) = drive(',
  '            1,',
  '            None,',
  '            vec[',
  '                Err(provider_error(',
  '                    429,',
  '                    &[("retry-after", "Sat, 01 Jan 2000 00:00:00 GMT")],',
  '                )),',
  '                Ok("ok"),',
  '            ],',
  '        )',
  '        .await;',
  '        assert_eq!(result, Ok("ok"));',
  '        assert_eq!(attempts, 2);',
  '        assert!(elapsed < 300, "slept {elapsed}ms");',
  '',
  '        // Garbage: NaN fails the upstream Number.isFinite gate and falls',
  '        // through to the exponential backoff (index 0: 0.5s *',
  '        // (1 - random()*0.25) → 375–500ms with the jitter bounds).',
  '        let (result, attempts, elapsed) = drive(',
  '            1,',
  '            None,',
  '            vec[',
  '                Err(provider_error(429, &[("retry-after", "garbage")])),',
  '                Ok("ok"),',
  '            ],',
  '        )',
  '        .await;',
  '        assert_eq!(result, Ok("ok"));',
  '        assert_eq!(attempts, 2);',
  '        assert!(',
  '            (350..550).contains(&elapsed),',
  '            "slept {elapsed}ms (expected the 375-500ms exponential band)"',
  '        );',
  '    }',
].join(eol);
if (!t.includes(from)) { console.error('MISS retry.rs'); process.exit(1); }
t = t.replace(from, to);
// Update the doc comment above the test.
const docFrom = [
  '    /// Upstream (2bbfcca43): an unparseable Retry-After — a past HTTP-date',
  '    /// (negative delay) or garbage (NaN) — no longer sleeps ~0; the',
  '    /// non-finite/negative check skips the server-delay branch and the',
  '    /// EXPONENTIAL backoff applies (index 0: 0.5s * (1 - random()*0.25) →',
  '    /// 375–500ms with the jitter bounds).',
].join(eol);
const docTo = [
  '    /// Upstream (2bbfcca43): `Number.isFinite(delayMs)` gates the',
  '    /// Retry-After branch. A PAST HTTP-date gives a negative (finite)',
  '    /// delay that still returns — the sleep clamps to zero, so the retry',
  '    /// is immediate. Unparseable garbage gives NaN, fails the finite',
  '    /// check, and falls through to the exponential backoff.',
].join(eol);
if (!t.includes(docFrom)) { console.error('MISS doc'); process.exit(1); }
t = t.replace(docFrom, docTo);
fs.writeFileSync(p, t);
console.log('patched retry.rs');
