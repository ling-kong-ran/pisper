//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/session-selector-search.ts` (194
//! lines, sha256
//! `5b1811240c39c958f863091ece06d67cb8432d4d7e70185087d5e041432dff36`):
//! search-query parsing, session matching and filter/sort for the session
//! selector.
//!
//! Disclosed divergence (S19.9 in `components/mod.rs`): `re:` queries build a
//! `RegExp` with the `i` flag upstream; the port uses the vendored `regex`
//! crate (same subset of syntax for the patterns the selector accepts — the
//! upstream error message text for invalid patterns is not reproduced, the
//! error string carries the crate's message).

use crate::coding_agent::session_manager::SessionInfo;
use crate::tui::fuzzy::fuzzy_match;

/// Upstream `SortMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortMode {
    #[default]
    Threaded,
    Recent,
    Relevance,
}

/// Upstream `NameFilter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NameFilter {
    #[default]
    All,
    Named,
}

/// Upstream `ParsedSearchQuery`.
#[derive(Clone, Debug)]
pub enum ParsedSearchQuery {
    Tokens(Vec<SearchToken>),
    Regex {
        regex: Option<regex::Regex>,
        error: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchToken {
    Fuzzy(String),
    Phrase(String),
}

fn normalize_whitespace_lower(text: &str) -> String {
    // toLowerCase + \s+ → " " + trim
    let mut out = String::with_capacity(text.len());
    let mut last_ws = false;
    for c in text.to_lowercase().chars() {
        if c.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    out.trim().to_string()
}

fn get_session_search_text(session: &SessionInfo) -> String {
    format!(
        "{} {} {} {}",
        session.id,
        session.name.as_deref().unwrap_or(""),
        session.all_messages_text,
        session.cwd
    )
}

/// Upstream `hasSessionName`.
pub fn has_session_name(session: &SessionInfo) -> bool {
    session
        .name
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty())
}

fn matches_name_filter(session: &SessionInfo, filter: NameFilter) -> bool {
    if filter == NameFilter::All {
        return true;
    }
    has_session_name(session)
}

/// Upstream `parseSearchQuery`.
pub fn parse_search_query(query: &str) -> ParsedSearchQuery {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return ParsedSearchQuery::Tokens(Vec::new());
    }

    // Regex mode: re:<pattern>
    if let Some(pattern_with_prefix) = trimmed.strip_prefix("re:") {
        let pattern = pattern_with_prefix.trim();
        if pattern.is_empty() {
            return ParsedSearchQuery::Regex {
                regex: None,
                error: Some("Empty regex".to_string()),
            };
        }
        return match regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
        {
            Ok(regex) => ParsedSearchQuery::Regex {
                regex: Some(regex),
                error: None,
            },
            Err(err) => ParsedSearchQuery::Regex {
                regex: None,
                error: Some(err.to_string()),
            },
        };
    }

    // Token mode with quote support. Example: foo "node cve" bar
    let mut tokens: Vec<SearchToken> = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    let mut had_unclosed_quote = false;

    let flush =
        |buf: &mut String, tokens: &mut Vec<SearchToken>, kind: fn(String) -> SearchToken| {
            let value = buf.trim().to_string();
            buf.clear();
            if !value.is_empty() {
                tokens.push(kind(value));
            }
        };

    for ch in trimmed.chars() {
        if ch == '"' {
            if in_quote {
                flush(&mut buf, &mut tokens, SearchToken::Phrase);
                in_quote = false;
            } else {
                flush(&mut buf, &mut tokens, SearchToken::Fuzzy);
                in_quote = true;
            }
            continue;
        }
        if !in_quote && ch.is_whitespace() {
            flush(&mut buf, &mut tokens, SearchToken::Fuzzy);
            continue;
        }
        buf.push(ch);
    }

    if in_quote {
        had_unclosed_quote = true;
    }

    // If quotes were unbalanced, fall back to plain whitespace tokenization.
    if had_unclosed_quote {
        return ParsedSearchQuery::Tokens(
            trimmed
                .split_whitespace()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .map(SearchToken::Fuzzy)
                .collect(),
        );
    }

    // upstream: `flush(inQuote ? "phrase" : "fuzzy")` — the in-quote case
    // already returned above, so this is always a fuzzy flush.
    let _ = in_quote;
    flush(&mut buf, &mut tokens, SearchToken::Fuzzy);

    ParsedSearchQuery::Tokens(tokens)
}

/// Upstream `MatchResult`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchResult {
    pub matches: bool,
    /// Lower is better; only meaningful when `matches`.
    pub score: f64,
}

/// Upstream `matchSession`.
pub fn match_session(session: &SessionInfo, parsed: &ParsedSearchQuery) -> MatchResult {
    let text = get_session_search_text(session);

    match parsed {
        ParsedSearchQuery::Regex { regex, .. } => {
            let Some(regex) = regex else {
                return MatchResult {
                    matches: false,
                    score: 0.0,
                };
            };
            match regex.find(&text) {
                None => MatchResult {
                    matches: false,
                    score: 0.0,
                },
                Some(found) => MatchResult {
                    matches: true,
                    score: found.start() as f64 * 0.1,
                },
            }
        }
        ParsedSearchQuery::Tokens(tokens) => {
            if tokens.is_empty() {
                return MatchResult {
                    matches: true,
                    score: 0.0,
                };
            }
            let mut total_score = 0.0;
            let mut normalized_text: Option<String> = None;
            for token in tokens {
                match token {
                    SearchToken::Phrase(phrase) => {
                        if normalized_text.is_none() {
                            normalized_text = Some(normalize_whitespace_lower(&text));
                        }
                        let phrase = normalize_whitespace_lower(phrase);
                        if phrase.is_empty() {
                            continue;
                        }
                        let Some(index) =
                            normalized_text.as_deref().unwrap_or_default().find(&phrase)
                        else {
                            return MatchResult {
                                matches: false,
                                score: 0.0,
                            };
                        };
                        total_score += index as f64 * 0.1;
                    }
                    SearchToken::Fuzzy(query) => {
                        let m = fuzzy_match(query, &text);
                        if !m.matches {
                            return MatchResult {
                                matches: false,
                                score: 0.0,
                            };
                        }
                        total_score += m.score;
                    }
                }
            }
            MatchResult {
                matches: true,
                score: total_score,
            }
        }
    }
}

/// Upstream `filterAndSortSessions`.
pub fn filter_and_sort_sessions(
    sessions: &[SessionInfo],
    query: &str,
    sort_mode: SortMode,
    name_filter: NameFilter,
) -> Vec<SessionInfo> {
    let name_filtered: Vec<SessionInfo> = if name_filter == NameFilter::All {
        sessions.to_vec()
    } else {
        sessions
            .iter()
            .filter(|session| matches_name_filter(session, name_filter))
            .cloned()
            .collect()
    };
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return name_filtered;
    }

    let parsed = parse_search_query(query);
    if let ParsedSearchQuery::Regex { error: Some(_), .. } = &parsed {
        return Vec::new();
    }

    // Recent mode: filter only, keep incoming order.
    if sort_mode == SortMode::Recent {
        return name_filtered
            .into_iter()
            .filter(|s| match_session(s, &parsed).matches)
            .collect();
    }

    // Relevance mode: sort by score, tie-break by modified desc.
    // Threaded mode shares the relevance ordering in this module (the tree
    // rebuild happens in the already-ported session-selector).
    let mut scored: Vec<(SessionInfo, f64)> = Vec::new();
    for session in name_filtered {
        let result = match_session(&session, &parsed);
        if !result.matches {
            continue;
        }
        scored.push((session, result.score));
    }

    scored.sort_by(|a, b| {
        if a.1 != b.1 {
            a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            b.0.modified.cmp(&a.0.modified)
        }
    });

    scored.into_iter().map(|(session, _)| session).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, name: Option<&str>, text: &str, cwd: &str, modified: i64) -> SessionInfo {
        SessionInfo {
            path: format!("C:\\s\\{id}.jsonl"),
            id: id.to_string(),
            cwd: cwd.to_string(),
            name: name.map(str::to_string),
            parent_session_path: None,
            created: Some(modified - 1000),
            modified,
            message_count: 3,
            first_message: text.chars().take(10).collect(),
            all_messages_text: text.to_string(),
        }
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `session_selector_search`.
    #[test]
    fn parse_search_query_matches_oracle() {
        // empty → no tokens
        let ParsedSearchQuery::Tokens(empty) = parse_search_query("") else {
            panic!()
        };
        assert!(empty.is_empty());
        let ParsedSearchQuery::Tokens(spaces) = parse_search_query("   ") else {
            panic!()
        };
        assert!(spaces.is_empty());

        let ParsedSearchQuery::Tokens(plain) = parse_search_query("foo bar") else {
            panic!()
        };
        assert_eq!(
            plain,
            vec![
                SearchToken::Fuzzy("foo".into()),
                SearchToken::Fuzzy("bar".into())
            ]
        );

        let ParsedSearchQuery::Tokens(quoted) = parse_search_query("foo \"node cve\" bar") else {
            panic!()
        };
        assert_eq!(
            quoted,
            vec![
                SearchToken::Fuzzy("foo".into()),
                SearchToken::Phrase("node cve".into()),
                SearchToken::Fuzzy("bar".into()),
            ]
        );

        let ParsedSearchQuery::Tokens(unclosed) = parse_search_query("unclosed \"quote here")
        else {
            panic!()
        };
        assert_eq!(
            unclosed,
            vec![
                SearchToken::Fuzzy("unclosed".into()),
                SearchToken::Fuzzy("\"quote".into()),
                SearchToken::Fuzzy("here".into()),
            ]
        );

        let ParsedSearchQuery::Regex { regex, error } = parse_search_query("re:^Err(or)?$") else {
            panic!()
        };
        assert!(regex.is_some());
        assert!(error.is_none());

        let ParsedSearchQuery::Regex { regex, error } = parse_search_query("re:") else {
            panic!()
        };
        assert!(regex.is_none());
        assert_eq!(error.as_deref(), Some("Empty regex"));

        let ParsedSearchQuery::Regex { regex, error } = parse_search_query("re:   ") else {
            panic!()
        };
        assert!(regex.is_none());
        assert_eq!(error.as_deref(), Some("Empty regex"));

        let ParsedSearchQuery::Regex { regex, error } = parse_search_query("re:([bad") else {
            panic!()
        };
        assert!(regex.is_none());
        assert!(error.is_some());

        let ParsedSearchQuery::Tokens(multi) = parse_search_query("multi  spaces\ttabs") else {
            panic!()
        };
        assert_eq!(
            multi,
            vec![
                SearchToken::Fuzzy("multi".into()),
                SearchToken::Fuzzy("spaces".into()),
                SearchToken::Fuzzy("tabs".into()),
            ]
        );
    }

    #[test]
    fn match_and_filter_match_oracle() {
        assert!(has_session_name(&session("a", Some("named"), "", "", 0)));
        assert!(!has_session_name(&session("b", Some("  "), "", "", 0)));
        assert!(!has_session_name(&session("c", None, "", "", 0)));

        let s1 = session(
            "1",
            Some("alpha session"),
            "fix the parser bug",
            "C:\\work",
            2000,
        );
        let s2 = session("2", None, "review PR node cve fix", "C:\\work", 3000);
        let s3 = session(
            "3",
            Some("beta notes"),
            "random chatter about rust",
            "C:\\other",
            1000,
        );
        let sessions = vec![s1.clone(), s2.clone(), s3.clone()];

        // fuzzy token match
        assert!(match_session(&s1, &parse_search_query("parser")).matches);
        // phrase match
        assert!(match_session(&s1, &parse_search_query("fix \"parser bug\"")).matches);
        // regex match scores by start position
        let regex_match = match_session(&s2, &parse_search_query("re:node cve"));
        assert!(regex_match.matches);
        // regex miss
        assert!(!match_session(&s2, &parse_search_query("re:zzz")).matches);
        // invalid regex never matches
        assert!(!match_session(&s1, &parse_search_query("re:(")).matches);
        // empty query matches everything with score 0
        assert_eq!(
            match_session(&s1, &parse_search_query("")),
            MatchResult {
                matches: true,
                score: 0.0
            }
        );
        // fuzzy miss
        assert!(!match_session(&s3, &parse_search_query("alpha")).matches);

        // recent mode keeps order, filters only
        let recent = filter_and_sort_sessions(&sessions, "work", SortMode::Recent, NameFilter::All);
        assert_eq!(
            recent.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["1", "2"]
        );
        // named filter drops the unnamed session
        let named = filter_and_sort_sessions(&sessions, "", SortMode::Recent, NameFilter::Named);
        assert_eq!(
            named.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["1", "3"]
        );
        // phrase filter
        let phrase =
            filter_and_sort_sessions(&sessions, "node cve", SortMode::Recent, NameFilter::All);
        assert_eq!(
            phrase.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["2"]
        );
        // invalid regex → no results
        assert!(
            filter_and_sort_sessions(&sessions, "re:(", SortMode::Recent, NameFilter::All)
                .is_empty()
        );

        // relevance sorts by score then modified desc
        let relevance =
            filter_and_sort_sessions(&sessions, "the", SortMode::Relevance, NameFilter::All);
        assert!(!relevance.is_empty());
        let relevance_fix =
            filter_and_sort_sessions(&sessions, "fix", SortMode::Relevance, NameFilter::All);
        assert_eq!(relevance_fix.first().map(|s| s.id.as_str()), Some("1"));

        let threaded =
            filter_and_sort_sessions(&sessions, "alpha", SortMode::Threaded, NameFilter::All);
        assert_eq!(
            threaded.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["1"]
        );
    }
}
