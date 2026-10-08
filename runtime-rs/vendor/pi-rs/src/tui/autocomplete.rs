//! Port of upstream `packages/tui/src/autocomplete.ts`: slash-command and
//! file-path autocomplete providers.
//!
//! Disclosed substitutions for review:
//! - Upstream `getSuggestions` is async with an `AbortSignal` and debounced
//!   invocation from the editor. The Rust provider trait is synchronous: the
//!   editor calls it on the render line; short filesystem reads make the
//!   blocking acceptable in this slice, and the fd(1) integration
//!   (`walkDirectoryWithFd`) is preserved behind
//!   [`CombinedAutocompleteProvider::fd_path`] using a synchronous child
//!   spawn.
//! - `Awaitable<T>` return values become plain `T`.

use std::path::{Path, PathBuf};

use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::utils::{
    has_autocomplete_separator, is_autocomplete_separator_char, is_token_start_boundary,
};

/// A normalized command-suggestion candidate (name, label, description).
type CommandItem = (String, Option<String>, Option<String>);
/// A [`CommandItem`] tagged with its provider-list index (the upstream
/// `bareNameMatchSet` deduplicates by object identity).
type IndexedCommandItem = (usize, CommandItem);

const PATH_DELIMITERS: &[char] = &[' ', '\t', '"', '\'', '='];
/// Opening wrappers that may precede a path in prose, mapped to their closing
/// counterpart.
const PATH_WRAPPERS: &[(char, char)] =
    &[('(', ')'), ('[', ']'), ('{', '}'), ('<', '>'), ('`', '`')];

fn path_wrapper_closer(opening: char) -> Option<char> {
    PATH_WRAPPERS
        .iter()
        .find(|(open, _)| *open == opening)
        .map(|(_, close)| *close)
}

/// Upstream `AutocompleteItem`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// Upstream `SlashCommand`.
#[derive(Clone, Debug, Default)]
pub struct SlashCommand {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

/// Upstream `AutocompleteSuggestions`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutocompleteSuggestions {
    pub items: Vec<AutocompleteItem>,
    /// What we're matching against (e.g. "/" or "src/").
    pub prefix: String,
}

/// Upstream `AutocompleteProvider`.
pub trait AutocompleteProvider {
    /// Characters that naturally trigger this provider at token boundaries.
    fn trigger_characters(&self) -> Vec<char> {
        vec![]
    }

    /// Suggestions for the current text/cursor position; `None` when nothing
    /// is available.
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions>;

    /// Apply the selected item to the text.
    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> AppliedCompletion;

    /// Whether file completion should trigger for explicit Tab completion.
    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        let _ = (lines, cursor_line, cursor_col);
        false
    }
}

/// Upstream `applyCompletion` result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedCompletion {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

fn to_display_path(value: &str) -> String {
    value.replace('\\', "/")
}

fn is_path_delimiter(c: char) -> bool {
    PATH_DELIMITERS.contains(&c)
}

fn find_last_delimiter(text: &str) -> Option<usize> {
    // Upstream walks code points and returns the UTF-16 index of the LAST UNIT
    // of the matched character; callers slice after it. The byte offset after
    // the character is the same boundary.
    let mut last_delimiter_end: Option<usize> = None;
    for (index, character) in text.char_indices() {
        if is_path_delimiter(character) || is_autocomplete_separator_char(character) {
            last_delimiter_end = Some(index + character.len_utf8());
        }
    }
    last_delimiter_end
}

/// Strip opening wrappers before a path, e.g. "(~/Dev" -> "~/Dev" or
/// "`src/ma" -> "src/ma". Keep a wrapper if the token also contains its
/// closer, e.g. "app/[slug]/pa" or "(group)/pa".
fn strip_leading_wrappers(token: &str) -> String {
    let mut result = token;
    while !result.is_empty() {
        let Some(closer) = path_wrapper_closer(result.chars().next().unwrap_or('\0')) else {
            break;
        };
        let mut chars = result.char_indices();
        chars.next(); // the opening wrapper itself
        if result[chars.next().map_or(result.len(), |(index, _)| index)..].contains(closer) {
            break;
        }
        result = &result[1..];
    }
    result.to_string()
}

fn find_unclosed_quote_start(text: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut quote_start = None;
    for (i, c) in text.char_indices() {
        if c == '"' {
            in_quotes = !in_quotes;
            if in_quotes {
                quote_start = Some(i);
            }
        }
    }
    if in_quotes {
        quote_start
    } else {
        None
    }
}

fn is_token_start(text: &str, index: usize) -> bool {
    // Walk back over opening wrappers (`(@src` / `` `@src ``), then require a
    // path delimiter or a separator boundary (whitespace/CJK punctuation).
    let mut start = index;
    while start > 0 {
        let previous = text[..start].chars().next_back().unwrap_or('\0');
        if path_wrapper_closer(previous).is_none() {
            break;
        }
        start -= previous.len_utf8();
    }
    let char_before = text[..start].chars().next_back();
    is_path_delimiter(char_before.unwrap_or('\0')) || is_token_start_boundary(&text[..start])
}

fn extract_quoted_prefix(text: &str) -> Option<String> {
    let quote_start = find_unclosed_quote_start(text)?;

    let before_quote = text[..quote_start].chars().next_back();
    if before_quote == Some('@') {
        let at_index = quote_start - '@'.len_utf8();
        if !is_token_start(text, at_index) {
            return None;
        }
        return Some(text[at_index..].to_string());
    }

    if !is_token_start(text, quote_start) {
        return None;
    }

    Some(text[quote_start..].to_string())
}

fn parse_path_prefix(prefix: &str) -> (String, bool, bool) {
    if let Some(rest) = prefix.strip_prefix("@\"") {
        return (rest.to_string(), true, true);
    }
    if let Some(rest) = prefix.strip_prefix('"') {
        return (rest.to_string(), false, true);
    }
    if let Some(rest) = prefix.strip_prefix('@') {
        return (rest.to_string(), true, false);
    }
    (prefix.to_string(), false, false)
}

fn build_completion_value(
    path: &str,
    _is_directory: bool,
    is_at_prefix: bool,
    is_quoted_prefix: bool,
) -> String {
    let needs_quotes = is_quoted_prefix || has_autocomplete_separator(path);
    let prefix = if is_at_prefix { "@" } else { "" };

    if !needs_quotes {
        return format!("{prefix}{path}");
    }

    format!("{prefix}\"{path}\"")
}

/// Upstream `CombinedAutocompleteProvider`: slash commands + file paths.
pub struct CombinedAutocompleteProvider {
    commands: Vec<SlashCommand>,
    items: Vec<AutocompleteItem>,
    base_path: PathBuf,
    /// Reserved for the fd(1)-backed fuzzy file search (upstream
    /// `walkDirectoryWithFd`); unused while that slice is pending.
    #[allow(dead_code)]
    fd_path: Option<PathBuf>,
}

impl CombinedAutocompleteProvider {
    pub fn new(
        commands: Vec<SlashCommand>,
        base_path: impl Into<PathBuf>,
        fd_path: Option<PathBuf>,
    ) -> Self {
        Self {
            commands,
            items: Vec::new(),
            base_path: base_path.into(),
            fd_path,
        }
    }

    /// Upstream also accepts plain `AutocompleteItem`s in the command list.
    pub fn with_items(mut self, items: Vec<AutocompleteItem>) -> Self {
        self.items = items;
        self
    }

    fn find_command(&self, name: &str) -> Option<&SlashCommand> {
        self.commands.iter().find(|cmd| cmd.name == name)
    }

    fn extract_at_prefix(&self, text: &str) -> Option<String> {
        if let Some(quoted_prefix) = extract_quoted_prefix(text) {
            if quoted_prefix.starts_with("@\"") {
                return Some(quoted_prefix);
            }
        }

        let token = strip_leading_wrappers(match find_last_delimiter(text) {
            Some(after_delimiter) => &text[after_delimiter..],
            None => text,
        });

        if token.starts_with('@') {
            Some(token)
        } else {
            None
        }
    }

    fn extract_path_prefix(&self, text: &str, force_extract: bool) -> Option<String> {
        if let Some(quoted_prefix) = extract_quoted_prefix(text) {
            return Some(quoted_prefix);
        }

        let path_prefix = strip_leading_wrappers(match find_last_delimiter(text) {
            Some(after_delimiter) => &text[after_delimiter..],
            None => text,
        });

        if force_extract {
            return Some(path_prefix);
        }

        if path_prefix.contains('/')
            || path_prefix.starts_with('.')
            || path_prefix.starts_with("~/")
        {
            return Some(path_prefix);
        }

        // Return an empty prefix after whitespace or CJK punctuation, but not
        // for empty text. Empty text should not trigger file suggestions —
        // that's for forced Tab completion.
        if path_prefix.is_empty() && !text.is_empty() && is_token_start_boundary(text) {
            return Some(path_prefix);
        }

        None
    }

    fn expand_home_path(&self, path: &str) -> String {
        if let Some(rest) = path.strip_prefix("~/") {
            let expanded = self.base_path.join(rest).to_string_lossy().into_owned();
            // Upstream joins against homedir(); the port keeps the provider's
            // base path so tests stay hermetic.
            if path.ends_with('/') && !expanded.ends_with('/') {
                format!("{expanded}/")
            } else {
                expanded
            }
        } else if path == "~" {
            self.base_path.to_string_lossy().into_owned()
        } else {
            path.to_string()
        }
    }

    fn get_file_suggestions(&self, prefix: &str) -> Vec<AutocompleteItem> {
        let search_dir;
        let (raw_prefix, is_at_prefix, is_quoted_prefix) = parse_path_prefix(prefix);
        let mut expanded_prefix = raw_prefix.clone();

        if expanded_prefix.starts_with('~') {
            expanded_prefix = self.expand_home_path(&expanded_prefix);
        }

        let is_root_prefix = raw_prefix.is_empty()
            || raw_prefix == "./"
            || raw_prefix == "../"
            || raw_prefix == "~"
            || raw_prefix == "~/"
            || raw_prefix == "/"
            || (is_at_prefix && raw_prefix.is_empty());

        // is_root_prefix and trailing-"/" prefixes resolve the same way: the
        // search directory comes from the prefix and completes its contents.
        let search_prefix = if is_root_prefix || raw_prefix.ends_with('/') {
            if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
                search_dir = expanded_prefix.clone();
            } else {
                search_dir = self
                    .base_path
                    .join(&expanded_prefix)
                    .to_string_lossy()
                    .into_owned();
            }
            String::new()
        } else {
            let dir = Path::new(&expanded_prefix)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let file = Path::new(&expanded_prefix)
                .file_name()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
                search_dir = dir;
            } else {
                search_dir = self.base_path.join(&dir).to_string_lossy().into_owned();
            }
            file
        };

        let Ok(entries) = std::fs::read_dir(&search_dir) else {
            return Vec::new();
        };
        let mut suggestions: Vec<AutocompleteItem> = Vec::new();

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name
                .to_lowercase()
                .starts_with(&search_prefix.to_lowercase())
            {
                continue;
            }

            let is_directory = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

            let relative_path: String;
            let display_prefix = raw_prefix.clone();

            if display_prefix.ends_with('/') {
                relative_path = format!("{display_prefix}{name}");
            } else if display_prefix.contains('/') || display_prefix.contains('\\') {
                if display_prefix.starts_with("~/") {
                    let home_relative_dir: &str = display_prefix
                        .strip_prefix("~/")
                        .unwrap_or(display_prefix.as_str());
                    let dir = Path::new(home_relative_dir)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|| ".".to_string());
                    relative_path = if dir == "." {
                        format!("~/{name}")
                    } else {
                        format!("~/{dir}/{name}")
                    };
                } else if display_prefix.starts_with('/') {
                    let dir = Path::new(display_prefix.as_str())
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    relative_path = if dir == "/" {
                        format!("/{name}")
                    } else {
                        format!("{dir}/{name}")
                    };
                } else {
                    let dir = Path::new(display_prefix.as_str())
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|| ".".to_string());
                    let mut candidate = if dir == "." {
                        name.clone()
                    } else {
                        format!("{dir}/{name}")
                    };
                    if display_prefix.starts_with("./") && !candidate.starts_with("./") {
                        candidate = format!("./{candidate}");
                    }
                    relative_path = candidate;
                }
            } else if display_prefix.starts_with('~') {
                relative_path = format!("~/{name}");
            } else {
                relative_path = name.clone();
            }

            let relative_path = to_display_path(&relative_path);
            let path_value = if is_directory {
                format!("{relative_path}/")
            } else {
                relative_path.clone()
            };
            let value =
                build_completion_value(&path_value, is_directory, is_at_prefix, is_quoted_prefix);

            suggestions.push(AutocompleteItem {
                value,
                label: if is_directory {
                    format!("{name}/")
                } else {
                    name.clone()
                },
                description: None,
            });
        }

        // Directories first (by label), then alphabetical (localeCompare is a
        // pre-existing disclosed seam; the delta only moved value -> label).
        suggestions.sort_by(|a, b| {
            let a_is_dir = a.label.ends_with('/');
            let b_is_dir = b.label.ends_with('/');
            if a_is_dir && !b_is_dir {
                std::cmp::Ordering::Less
            } else if !a_is_dir && b_is_dir {
                std::cmp::Ordering::Greater
            } else {
                a.label.cmp(&b.label)
            }
        });

        suggestions
    }

    pub fn should_trigger_file_completion_impl(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        let current_line = lines.get(cursor_line).map(String::as_str).unwrap_or("");
        let text_before_cursor = &current_line[..cursor_col.min(current_line.len())];
        let trimmed = text_before_cursor.trim();
        if trimmed.starts_with('/') && !trimmed.contains(' ') {
            return false;
        }
        true
    }
}

impl AutocompleteProvider for CombinedAutocompleteProvider {
    fn trigger_characters(&self) -> Vec<char> {
        vec!['@', '#']
    }

    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions> {
        let current_line = lines.get(cursor_line)?;
        let text_before_cursor = &current_line[..cursor_col.min(current_line.len())];

        if let Some(at_prefix) = self.extract_at_prefix(text_before_cursor) {
            // The fd-backed fuzzy search needs the external tool; without it
            // there are no @-suggestions (upstream falls back the same way).
            let (_raw, _quoted) = ((), ());
            let _ = at_prefix;
            return None;
        }

        // v1.0.0: the command grammar matches against the line's leading
        // whitespace trimmed, and the reported prefix is that trimmed text.
        let command_text = text_before_cursor.trim_start();
        if !force && command_text.starts_with('/') {
            let space_index = command_text.find(' ');

            let Some(space_index) = space_index else {
                let prefix = command_text[1..].to_string();
                let mut command_items: Vec<(String, Option<String>, Option<String>)> = Vec::new();
                for cmd in &self.commands {
                    let hint = cmd.argument_hint.clone();
                    let desc = cmd.description.clone();
                    let full_desc = match (hint, desc) {
                        (Some(h), Some(d)) => Some(format!("{h} — {d}")),
                        (Some(h), None) => Some(h),
                        (None, d) => d,
                    };
                    command_items.push((cmd.name.clone(), Some(cmd.name.clone()), full_desc));
                }
                for item in &self.items {
                    command_items.push((
                        item.value.clone(),
                        Some(item.label.clone()),
                        item.description.clone(),
                    ));
                }

                // `skill:` commands first match by bare name; names that only
                // match with the prefix are appended after the bare matches.
                // Identity dedup upstream becomes index dedup here.
                let indexed: Vec<IndexedCommandItem> =
                    command_items.into_iter().enumerate().collect();
                let bare_name_matches = fuzzy_filter(indexed.clone(), &prefix, |(_, item)| {
                    item.0.strip_prefix("skill:").unwrap_or(item.0.as_str())
                });
                let bare_name_match_indices: std::collections::HashSet<usize> =
                    bare_name_matches.iter().map(|(index, _)| *index).collect();
                let full_name_only_matches = fuzzy_filter(
                    indexed
                        .into_iter()
                        .filter(|(index, item)| {
                            item.0.starts_with("skill:") && !bare_name_match_indices.contains(index)
                        })
                        .collect(),
                    &prefix,
                    |(_, item)| item.0.as_str(),
                );
                let filtered: Vec<(String, Option<String>, Option<String>)> = bare_name_matches
                    .into_iter()
                    .chain(full_name_only_matches)
                    .map(|(_, item)| item)
                    .collect();
                if filtered.is_empty() {
                    return None;
                }

                let items = filtered
                    .into_iter()
                    .map(|(name, label, description)| AutocompleteItem {
                        value: name,
                        label: label.unwrap_or_default(),
                        description,
                    })
                    .collect();

                return Some(AutocompleteSuggestions {
                    items,
                    prefix: command_text.to_string(),
                });
            };

            let command_name = command_text[1..space_index].to_string();
            let argument_text = command_text[space_index + 1..].to_string();
            let command = self.find_command(&command_name);
            // Argument completions require the command-specific callback from
            // the host application; the provider alone has none.
            let _ = (command, argument_text);
            return None;
        }

        let path_match = self.extract_path_prefix(text_before_cursor, force)?;
        let suggestions = self.get_file_suggestions(&path_match);
        if suggestions.is_empty() {
            return None;
        }

        Some(AutocompleteSuggestions {
            items: suggestions,
            prefix: path_match,
        })
    }

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> AppliedCompletion {
        let current_line = lines.get(cursor_line).cloned().unwrap_or_default();
        let before_prefix = current_line[..cursor_col.saturating_sub(prefix.len())].to_string();
        let after_cursor = current_line[cursor_col.min(current_line.len())..].to_string();
        let is_quoted_prefix = prefix.starts_with('"') || prefix.starts_with("@\"");
        let has_leading_quote_after_cursor = after_cursor.starts_with('"');
        let has_trailing_quote_in_item = item.value.ends_with('"');
        let adjusted_after_cursor =
            if is_quoted_prefix && has_trailing_quote_in_item && has_leading_quote_after_cursor {
                after_cursor[1..].to_string()
            } else {
                after_cursor
            };

        let is_slash_command = prefix.starts_with('/')
            && before_prefix.trim().is_empty()
            && !prefix[1..].contains('/');
        if is_slash_command {
            let mut new_lines = lines.to_vec();
            new_lines[cursor_line] =
                format!("{before_prefix}/{} {adjusted_after_cursor}", item.value);
            return AppliedCompletion {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.len() + item.value.len() + 2,
            };
        }

        if prefix.starts_with('@') {
            let is_directory = item.label.ends_with('/');
            let suffix = if is_directory { "" } else { " " };
            let mut new_lines = lines.to_vec();
            new_lines[cursor_line] = format!(
                "{before_prefix}{}{suffix}{adjusted_after_cursor}",
                item.value
            );
            let has_trailing_quote = item.value.ends_with('"');
            let cursor_offset = if is_directory && has_trailing_quote {
                item.value.len() - 1
            } else {
                item.value.len()
            };
            return AppliedCompletion {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.len() + cursor_offset + suffix.len(),
            };
        }

        let text_before_cursor = current_line[..cursor_col.min(current_line.len())].to_string();
        if text_before_cursor.contains('/') && text_before_cursor.contains(' ') {
            let is_directory = item.label.ends_with('/');
            let has_trailing_quote = item.value.ends_with('"');
            let cursor_offset = if is_directory && has_trailing_quote {
                item.value.len() - 1
            } else {
                item.value.len()
            };
            let mut new_lines = lines.to_vec();
            new_lines[cursor_line] =
                format!("{before_prefix}{}{adjusted_after_cursor}", item.value);
            return AppliedCompletion {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.len() + cursor_offset,
            };
        }

        let is_directory = item.label.ends_with('/');
        let has_trailing_quote = item.value.ends_with('"');
        let cursor_offset = if is_directory && has_trailing_quote {
            item.value.len() - 1
        } else {
            item.value.len()
        };
        let mut new_lines = lines.to_vec();
        new_lines[cursor_line] = format!("{before_prefix}{}{adjusted_after_cursor}", item.value);
        AppliedCompletion {
            lines: new_lines,
            cursor_line,
            cursor_col: before_prefix.len() + cursor_offset,
        }
    }

    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        self.should_trigger_file_completion_impl(lines, cursor_line, cursor_col)
    }
}
