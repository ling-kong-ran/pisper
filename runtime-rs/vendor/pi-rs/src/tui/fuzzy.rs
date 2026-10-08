//! Port of upstream `packages/tui/src/fuzzy.ts`: fuzzy matching where all
//! query characters appear in order (lower score = better match).
//!
//! Disclosed substitution: match positions use char indices (JS uses UTF-16
//! unit offsets); scores agree for the ASCII-dominated inputs this feeds.

/// Upstream `FuzzyMatch`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FuzzyMatch {
    pub matches: bool,
    pub score: f64,
}

const WORD_BOUNDARY_CHARS: &[char] = &[' ', '\t', '\n', '\r', '-', '_', '.', '/'];

fn is_word_boundary_char(c: char) -> bool {
    // /[\s\-_./:]/
    c.is_whitespace() || WORD_BOUNDARY_CHARS.contains(&c)
}

pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();

    let match_query = |normalized_query: &str| -> FuzzyMatch {
        if normalized_query.is_empty() {
            return FuzzyMatch {
                matches: true,
                score: 0.0,
            };
        }

        if normalized_query.chars().count() > text_lower.chars().count() {
            return FuzzyMatch {
                matches: false,
                score: 0.0,
            };
        }

        let text_chars: Vec<char> = text_lower.chars().collect();
        let mut query_index = 0usize;
        let mut score = 0.0f64;
        let mut last_match_index: i64 = -1;
        let mut consecutive_matches = 0i64;

        let query_chars: Vec<char> = normalized_query.chars().collect();
        while query_index < query_chars.len() {
            let wanted = query_chars[query_index];
            let search_start = (last_match_index + 1).max(0) as usize;
            let found = text_chars[search_start..]
                .iter()
                .position(|&c| c == wanted)
                .map(|offset| search_start + offset);
            let Some(i) = found else {
                break;
            };

            let is_word_boundary = i == 0 || text_chars[i - 1].is_word_boundary_for_fuzzy();

            // Reward consecutive matches.
            if last_match_index == i as i64 - 1 {
                consecutive_matches += 1;
                score -= (consecutive_matches * 5) as f64;
            } else {
                consecutive_matches = 0;
                // Penalize gaps.
                if last_match_index >= 0 {
                    score += ((i as i64 - last_match_index - 1) * 2) as f64;
                }
            }

            // Reward word boundary matches.
            if is_word_boundary {
                score -= 10.0;
            }

            // Slight penalty for later matches.
            score += i as f64 * 0.1;

            last_match_index = i as i64;
            query_index += 1;
        }

        if query_index < query_chars.len() {
            return FuzzyMatch {
                matches: false,
                score: 0.0,
            };
        }

        if normalized_query == text_lower {
            score -= 100.0;
        }

        FuzzyMatch {
            matches: true,
            score,
        }
    };

    let primary_match = match_query(&query_lower);
    if primary_match.matches {
        return primary_match;
    }

    // Swap a trailing/leading letters+digits query (e.g. "codex52" -> "52codex").
    let swapped_query = swapped_alpha_numeric(&query_lower);
    let Some(swapped_query) = swapped_query else {
        return primary_match;
    };

    let swapped_match = match_query(&swapped_query);
    if !swapped_match.matches {
        return primary_match;
    }

    FuzzyMatch {
        matches: true,
        score: swapped_match.score + 5.0,
    }
}

trait WordBoundaryForFuzzy {
    fn is_word_boundary_for_fuzzy(&self) -> bool;
}

impl WordBoundaryForFuzzy for char {
    fn is_word_boundary_for_fuzzy(&self) -> bool {
        is_word_boundary_char(*self)
    }
}

/// `^(?<letters>[a-z]+)(?<digits>[0-9]+)$` / `^(?<digits>[0-9]+)(?<letters>[a-z]+)$`
/// swap: returns letters+digits or digits+letters respectively.
fn swapped_alpha_numeric(query: &str) -> Option<String> {
    let is_letters = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    let is_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());

    for split_at in 1..query.len() {
        let (head, tail) = query.split_at(split_at);
        if is_letters(head) && is_digits(tail) {
            return Some(format!("{tail}{head}"));
        }
        if is_digits(head) && is_letters(tail) {
            return Some(format!("{tail}{head}"));
        }
    }
    None
}

/// Upstream `fuzzyFilter`: filter and sort items by fuzzy match quality (best
/// first). Supports whitespace- and slash-separated tokens: all must match.
pub fn fuzzy_filter<T>(items: Vec<T>, query: &str, get_text: impl Fn(&T) -> &str) -> Vec<T> {
    if query.trim().is_empty() {
        return items;
    }

    let tokens: Vec<&str> = query
        .trim()
        .split(|c: char| c.is_whitespace() || c == '/')
        .filter(|token| !token.is_empty())
        .collect();

    if tokens.is_empty() {
        return items;
    }

    let mut results: Vec<(T, f64)> = Vec::new();

    for item in items {
        let text = get_text(&item);
        let mut total_score = 0.0;
        let mut all_match = true;

        for token in &tokens {
            let m = fuzzy_match(token, text);
            if m.matches {
                total_score += m.score;
            } else {
                all_match = false;
                break;
            }
        }

        if all_match {
            results.push((item, total_score));
        }
    }

    results.sort_by(|a, b| a.1.total_cmp(&b.1));
    results.into_iter().map(|(item, _)| item).collect()
}
