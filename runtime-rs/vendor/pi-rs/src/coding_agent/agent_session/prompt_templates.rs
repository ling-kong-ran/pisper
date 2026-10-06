//! Vendored expansion subset of upstream `coding-agent/src/core/prompt-templates.ts`
//! (285 lines at migration): `parseCommandArgs`, `substituteArgs`, and
//! `expandPromptTemplate`.
//!
//! SEAM (prompt-templates slice): only the expansion half is vendored here —
//! the loading half is already ported
//! ([`crate::coding_agent::core::resource_loader::load_prompt_templates`] and
//! its [`crate::coding_agent::core::resource_loader::PromptTemplate`]). When
//! the prompt-templates slice lands, this module becomes a re-export.
//!
//! The four `substituteArgs` replaces run in the same order with the same
//! regexes; JS function replacers insert their return value verbatim, which
//! matches `regex` closure replacers (no `$` expansion).
//! Disclosed substitution: `${@:0:N}` reads `slice(-1, …)` from the array end
//! in JS (negative index); the port clamps to 0 (upstream tests and callers
//! never use a zero start).

use crate::coding_agent::core::resource_loader::prompt_templates::PromptTemplate;

/// Upstream `parseCommandArgs` (prompt-templates.ts:26-67): parse command
/// arguments on whitespace. NOTE: the upstream body carries an `inQuote`
/// closing branch but no branch that ever OPENS a quote (the bash-style
/// quoting in the doc comment is a dead feature upstream), so quote characters
/// travel inside tokens verbatim; the port reproduces that behavior.
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();

    for character in args_string.chars() {
        if character.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

/// Upstream `substituteArgs` (prompt-templates.ts:70-96): substitute argument
/// placeholders — `$1, $2, ...` positional, `${@:start:length}` ranges, then
/// `$ARGUMENTS` and `$@` for all args.
pub fn substitute_args(content: &str, args: &[String]) -> String {
    use regex::Regex;

    let positional = Regex::new(r"\$(\d+)").expect("valid regex");
    let ranges = Regex::new(r"\$\{@:(\d+):(\d+)\}").expect("valid regex");
    let all_arguments = Regex::new(r"\$ARGUMENTS").expect("valid regex");
    let all_at = Regex::new(r"\$@").expect("valid regex");

    let joined = args.join(" ");

    // Replace positional arguments $1, $2, ...
    let result = positional.replace_all(content, |captures: &regex::Captures| {
        let index: i64 = captures[1].parse().unwrap_or(0);
        let zero_based = index - 1;
        // JS: `index < args.length ? args[index] : match` — a negative index
        // (e.g. `$0`) reads past the array and stringifies as "undefined".
        if zero_based < 0 {
            return "undefined".to_string();
        }
        match args.get(zero_based as usize) {
            Some(value) => value.clone(),
            // JS: `return match` — the untouched placeholder text.
            None => captures[0].to_string(),
        }
    });

    // Replace argument ranges ${@:start:length}
    let result = ranges.replace_all(&result, |captures: &regex::Captures| {
        let start: usize = captures[1].parse().unwrap_or(0);
        let length: usize = captures[2].parse().unwrap_or(0);
        let start_index = start.saturating_sub(1);
        args.iter()
            .skip(start_index)
            .take(length)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    });

    // Replace $ARGUMENTS with all args
    let result = all_arguments.replace_all(&result, joined.clone());
    // Replace $@ with all args
    let result = all_at.replace_all(&result, joined);
    result.into_owned()
}

/// Upstream `expandPromptTemplate` (prompt-templates.ts:269-285): expand a
/// prompt template if it matches a template name; returns the expanded content
/// or the original text.
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
    if !text.starts_with('/') {
        return text.to_string();
    }

    // /^\/([^\s]+)(?:\s+([\s\S]*))?$/
    let (template_name, args_string) = match text.strip_prefix('/') {
        Some(rest) => match rest.find(char::is_whitespace) {
            Some(space_index) => (&rest[..space_index], rest[space_index..].trim_start()),
            None => (rest, ""),
        },
        None => return text.to_string(),
    };
    // JS `\s+` between the name and the args trims all leading whitespace.
    let args_string = args_string.trim_start();

    let template = templates
        .iter()
        .find(|template| template.name == template_name);
    if let Some(template) = template {
        let args = parse_command_args(args_string);
        return substitute_args(&template.content, &args);
    }

    text.to_string()
}
