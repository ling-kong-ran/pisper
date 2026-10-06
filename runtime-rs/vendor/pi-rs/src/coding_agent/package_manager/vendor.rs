//! Vendored library behavior required by the package-manager slice
//! (upstream imports the npm packages `semver` 7.8.5, `minimatch` 10.2.6,
//! and `ignore` 7.0.8, plus node's `fs.globSync`; new dependencies are
//! forbidden, so the exact surface used by `core/package-manager.ts` is
//! reimplemented here).
//!
//! - `semver`: [`semver_valid`], [`SemVer`], [`parse_range`],
//!   [`range_satisfies`], [`semver_gt`], [`semver_rcompare`],
//!   [`max_satisfying`]. Comparator semantics follow node-semver 7.x
//!   (`^`/`~`/x-ranges/hyphen ranges, `-0` upper bounds, and the
//!   "version prerelease must share a comparator tuple" gate).
//! - `minimatch`: [`minimatch`] — dotfile rules (`*`/`?` do not match a
//!   leading `.` unless the pattern segment starts with one), `**`
//!   globstar segments, per-segment regex translation. No brace
//!   expansion / extglob (not used by upstream here).
//! - `glob`: [`glob_sync`] — node `fs.globSync(pattern, { cwd })` for the
//!   used subset: `/`-split segments, `**` (zero or more directory levels,
//!   never crossing dot segments), `*`/`?` (no leading-dot match), literal
//!   segments (dotfiles match literally), trailing `/` restricts to
//!   directories. Returns resolved absolute paths sorted lexicographically;
//!   the caller applies its own dot-segment filter.
//! - `ignore`: [`IgnoreMatcher`] over the `ignore` crate's gitignore module
//!   (already in the dependency tree), reproducing the npm `ignore` package
//!   call shape `ignore().add(rules).ignores(path)`; upstream rule prefixing
//!   is [`prefix_ignore_pattern`], applied by the caller.
//!
//! All behavior is pinned against the real upstream libraries via
//! `tests/fixtures/pm_oracle/core.oracle.json` (captured under node from the
//! pinned versions); see the tests.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use ignore::gitignore::GitignoreBuilder;

// ===========================================================================
// semver
// ===========================================================================

/// A parsed semver version (node-semver strict parse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Prerelease identifiers.
    pub prerelease: Vec<PrereleaseId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrereleaseId {
    Numeric(u64),
    Alphanumeric(String),
}

fn parse_numeric_identifier(text: &str) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    // Strict semver: no leading zeros on numeric identifiers.
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

impl PartialOrd for PrereleaseId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrereleaseId {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (PrereleaseId::Numeric(a), PrereleaseId::Numeric(b)) => a.cmp(b),
            // Numeric identifiers always have lower precedence than
            // alphanumeric identifiers.
            (PrereleaseId::Numeric(_), PrereleaseId::Alphanumeric(_)) => Ordering::Less,
            (PrereleaseId::Alphanumeric(_), PrereleaseId::Numeric(_)) => Ordering::Greater,
            (PrereleaseId::Alphanumeric(a), PrereleaseId::Alphanumeric(b)) => a.cmp(b),
        }
    }
}

impl SemVer {
    /// node-semver strict parse (leading `v` allowed).
    pub fn parse(input: &str) -> Option<SemVer> {
        let rest = input.strip_prefix('v').unwrap_or(input);
        // Core is the leading digit/dot run; the first non-digit/dot
        // character must start a `-pre` or `+build` suffix.
        let core_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let (core, suffix) = rest.split_at(core_end);
        if !suffix.is_empty() && !suffix.starts_with('-') && !suffix.starts_with('+') {
            return None;
        }
        if let Some(pre) = suffix.strip_prefix('-') {
            let (pre, build) = split_build(pre)?;
            if pre.is_empty() {
                return None;
            }
            for identifier in pre.split('.') {
                if identifier.is_empty() {
                    return None;
                }
                let numeric_ok = parse_numeric_identifier(identifier).is_some();
                let alpha_ok = identifier
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-');
                if !numeric_ok && !alpha_ok {
                    return None;
                }
            }
            let _ = build;
        } else if let Some(build) = suffix.strip_prefix('+') {
            let _ = split_build(build)?;
        }

        let mut parts = core.split('.');
        let major = parts.next()?;
        if major.is_empty() {
            return None;
        }
        let minor = parts.next();
        let patch = parts.next();
        if parts.next().is_some() {
            return None;
        }
        let major = parse_numeric_identifier(major)?;
        let minor = parse_numeric_identifier(minor?)?;
        let patch = parse_numeric_identifier(patch?)?;
        let mut prerelease_ids = Vec::new();
        if let Some(pre) = suffix.strip_prefix('-') {
            let (pre, _) = split_build(pre)?;
            for identifier in pre.split('.') {
                if let Some(value) = parse_numeric_identifier(identifier) {
                    prerelease_ids.push(PrereleaseId::Numeric(value));
                } else {
                    prerelease_ids.push(PrereleaseId::Alphanumeric(identifier.to_string()));
                }
            }
        }
        Some(SemVer {
            major,
            minor,
            patch,
            prerelease: prerelease_ids,
        })
    }

    pub fn render(&self) -> String {
        let mut out = format!("{}.{}.{}", self.major, self.minor, self.patch);
        if !self.prerelease.is_empty() {
            out.push('-');
            out.push_str(
                &self
                    .prerelease
                    .iter()
                    .map(|id| match id {
                        PrereleaseId::Numeric(value) => value.to_string(),
                        PrereleaseId::Alphanumeric(text) => text.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("."),
            );
        }
        out
    }

    /// A version with a prerelease compares as smaller than the same version
    /// without one; otherwise identifiers compare component-wise.
    pub fn cmp_version(&self, other: &SemVer) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(
                || match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) => compare_prerelease(&self.prerelease, &other.prerelease),
                },
            )
    }
}

fn compare_prerelease(a: &[PrereleaseId], b: &[PrereleaseId]) -> Ordering {
    for (x, y) in a.iter().zip(b.iter()) {
        let ordering = x.cmp(y);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    // A larger set of identifiers has higher precedence when the shared
    // prefix is equal.
    a.len().cmp(&b.len())
}

/// Split a build suffix (`+build`), which is ignored for precedence but must
/// still be syntactically valid.
fn split_build(text: &str) -> Option<(&str, Option<&str>)> {
    match text.split_once('+') {
        Some((rest, build)) => {
            if build.is_empty()
                || !build.split('.').all(|part| {
                    !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                return None;
            }
            Some((rest, Some(build)))
        }
        None => Some((text, None)),
    }
}

/// node-semver `valid(input)`: the normalized version string, or `None`.
pub fn semver_valid(input: &str) -> Option<String> {
    SemVer::parse(input).map(|version| version.render())
}

/// One comparator within a range set.
#[derive(Debug, Clone)]
pub struct Comparator {
    pub operator: ComparatorOperator,
    pub version: SemVer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparatorOperator {
    Lt,
    LtEq,
    Gt,
    GtEq,
    Eq,
}

/// A parsed range: `||`-separated comparator sets (AND within a set).
#[derive(Debug, Clone)]
pub struct Range {
    pub sets: Vec<RangeSet>,
}

#[derive(Debug, Clone)]
pub struct RangeSet {
    pub comparators: Vec<Comparator>,
    /// `true` for a bare `*` / empty range: any (non-prerelease) version.
    pub any: bool,
}

/// node-semver `validRange(input)`: `Some` when the range parses. The
/// canonical re-rendering of the range is not observable on the ported
/// surface (ranges only feed `satisfies`/`maxSatisfying`), so only the parse
/// decision is produced here.
pub fn parse_range(input: &str) -> Option<Range> {
    if input.trim().is_empty() {
        return None;
    }
    let trimmed = input.trim();
    let sets_text = trimmed.split("||").map(str::trim).collect::<Vec<_>>();
    let mut sets = Vec::new();
    for set_text in sets_text {
        sets.push(parse_range_set(set_text)?);
    }
    if sets.is_empty() {
        return None;
    }
    Some(Range { sets })
}

fn parse_range_set(text: &str) -> Option<RangeSet> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(RangeSet {
            comparators: Vec::new(),
            any: true,
        });
    }

    // Hyphen ranges: `1.2.3 - 2.3.4` (spaces around the hyphen required).
    if let Some(position) = trimmed.find(" - ") {
        let left = trimmed[..position].trim();
        let right = trimmed[position + 3..].trim();
        return parse_hyphen_range(left, right);
    }

    let mut comparators = Vec::new();
    for token in split_comparators(trimmed)? {
        comparators.extend(parse_comparator(&token)?);
    }
    // Bare `*` / `x` produces no comparators (matches everything).
    let any = comparators.is_empty();
    Some(RangeSet { comparators, any })
}

/// Split on whitespace, tolerating whitespace directly after `<`/`>`/`=`/
/// `~`/`^` (node-semver normalizes `>= 1.2.3` to `>=1.2.3`).
fn split_comparators(text: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(character);
        if matches!(character, '<' | '>' | '=' | '~' | '^') {
            while let Some(&next) = chars.peek() {
                if next.is_whitespace() {
                    chars.next();
                } else {
                    break;
                }
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Some(tokens)
}

fn parse_hyphen_range(left: &str, right: &str) -> Option<RangeSet> {
    let (left_major, left_minor, left_patch, left_suffix) = split_version_with_suffix(left)?;
    let (right_major, right_minor, right_patch, right_suffix) = split_version_with_suffix(right)?;
    let mut comparators = Vec::new();
    let lower = SemVer::parse(&format!(
        "{}.{}.{}{}",
        zero_or(left_major)?,
        zero_or(left_minor)?,
        zero_or(left_patch)?,
        left_suffix
    ))?;
    comparators.push(Comparator {
        operator: ComparatorOperator::GtEq,
        version: lower,
    });
    match (right_major, right_minor, right_patch) {
        (Some(m), Some(n), Some(p)) => {
            let upper = SemVer::parse(&format!("{}.{}.{}{}", m, n, p, right_suffix))?;
            comparators.push(Comparator {
                operator: ComparatorOperator::LtEq,
                version: upper,
            });
        }
        (Some(m), Some(n), None) => {
            comparators.push(below_minor_upper(m, n)?);
        }
        (Some(m), None, None) => {
            comparators.push(below_major_upper(m)?);
        }
        _ => return None,
    }
    Some(RangeSet {
        comparators,
        any: false,
    })
}

fn zero_or(value: Option<&str>) -> Option<&str> {
    match value {
        None => Some("0"),
        Some(text) => parse_numeric_identifier(text).map(|_| text),
    }
}

fn below_minor_upper(major: &str, minor: &str) -> Option<Comparator> {
    let major = parse_numeric_identifier(major)?;
    let minor = parse_numeric_identifier(minor)?;
    Some(Comparator {
        operator: ComparatorOperator::Lt,
        version: SemVer {
            major,
            minor: minor + 1,
            patch: 0,
            prerelease: vec![PrereleaseId::Numeric(0)],
        },
    })
}

fn below_major_upper(major: &str) -> Option<Comparator> {
    let major = parse_numeric_identifier(major)?;
    Some(Comparator {
        operator: ComparatorOperator::Lt,
        version: SemVer {
            major: major + 1,
            minor: 0,
            patch: 0,
            prerelease: vec![PrereleaseId::Numeric(0)],
        },
    })
}

/// Parse one comparator token (`^1.0.0`, `~1.2`, `>=1.0.0`, `1.2.3`, `1.x`,
/// `*`, `<1.2`, ...). Produces one or two comparators.
fn parse_comparator(token: &str) -> Option<Vec<Comparator>> {
    let (operator_text, version_text) = if let Some(rest) = token.strip_prefix(">=") {
        (">=", rest)
    } else if let Some(rest) = token.strip_prefix("<=") {
        ("<=", rest)
    } else if let Some(rest) = token.strip_prefix('>') {
        (">", rest)
    } else if let Some(rest) = token.strip_prefix('<') {
        ("<", rest)
    } else if let Some(rest) = token.strip_prefix('^') {
        ("^", rest)
    } else if let Some(rest) = token.strip_prefix('~') {
        ("~", rest.strip_prefix('>').unwrap_or(rest))
    } else if let Some(rest) = token.strip_prefix('=') {
        ("=", rest)
    } else {
        ("", token)
    };

    match operator_text {
        "^" => parse_caret(version_text),
        "~" => parse_tilde(version_text),
        ">" | ">=" | "<" | "<=" | "=" | "" => parse_simple(operator_text, version_text),
        _ => None,
    }
}

fn upper_major(major: u64) -> SemVer {
    SemVer {
        major: major + 1,
        minor: 0,
        patch: 0,
        prerelease: vec![PrereleaseId::Numeric(0)],
    }
}

fn upper_minor(major: u64, minor: u64) -> SemVer {
    SemVer {
        major,
        minor: minor + 1,
        patch: 0,
        prerelease: vec![PrereleaseId::Numeric(0)],
    }
}

fn gte(version: SemVer) -> Comparator {
    Comparator {
        operator: ComparatorOperator::GtEq,
        version,
    }
}

fn lt(version: SemVer) -> Comparator {
    Comparator {
        operator: ComparatorOperator::Lt,
        version,
    }
}

fn parse_caret(text: &str) -> Option<Vec<Comparator>> {
    let (major, minor, patch, prerelease_suffix) = split_version_with_suffix(text)?;
    let major_value = parse_numeric_identifier(major?)?;
    let minor_value = minor.and_then(parse_numeric_identifier);
    let patch_value = patch.and_then(parse_numeric_identifier);
    if major_value == 0 {
        match (minor_value, patch_value) {
            (Some(minor), Some(patch)) => {
                // ^0.0.3 := >=0.0.3 <0.0.4-0
                let lower = SemVer::parse(&format!("0.{minor}.{patch}{prerelease_suffix}"))?;
                let mut upper = lower.clone();
                upper.patch += 1;
                upper.prerelease = vec![PrereleaseId::Numeric(0)];
                return Some(vec![gte(lower), lt(upper)]);
            }
            (Some(minor), None) => {
                // ^0.2 := >=0.2.0 <0.3.0-0
                return Some(vec![
                    gte(SemVer::parse(&format!("0.{minor}.0"))?),
                    lt(upper_minor(0, minor)),
                ]);
            }
            _ => {
                // ^0 / ^0.x := >=0.0.0 <1.0.0-0
                return Some(vec![gte(SemVer::parse("0.0.0")?), lt(upper_major(0))]);
            }
        }
    }
    // ^1.2.3 := >=1.2.3 <2.0.0-0 ; ^1.2 := >=1.2.0 <2.0.0-0 ; ^1 := >=1.0.0 <2.0.0-0
    let lower_text = format!(
        "{}.{}.{}{}",
        major_value,
        minor_value.unwrap_or(0),
        patch_value.unwrap_or(0),
        prerelease_suffix
    );
    let lower = SemVer::parse(&lower_text)?;
    Some(vec![gte(lower), lt(upper_major(major_value))])
}

fn parse_tilde(text: &str) -> Option<Vec<Comparator>> {
    let (major, minor, patch, prerelease_suffix) = split_version_with_suffix(text)?;
    let major_value = parse_numeric_identifier(major?)?;
    let minor_value = minor.and_then(parse_numeric_identifier);
    match minor_value {
        Some(minor) => {
            if patch.is_some() {
                // ~1.2.3 := >=1.2.3 <1.3.0-0
                let lower = SemVer::parse(&format!(
                    "{}.{}.{}{}",
                    major_value,
                    minor,
                    patch.and_then(parse_numeric_identifier).unwrap_or(0),
                    prerelease_suffix
                ))?;
                return Some(vec![gte(lower), lt(upper_minor(major_value, minor))]);
            }
            // ~1.2 := >=1.2.0 <1.3.0-0
            let lower = SemVer::parse(&format!("{}.{}.0", major_value, minor))?;
            Some(vec![gte(lower), lt(upper_minor(major_value, minor))])
        }
        None => {
            // ~1 := >=1.0.0 <2.0.0-0
            let lower = SemVer::parse(&format!("{}.0.0", major_value))?;
            Some(vec![gte(lower), lt(upper_major(major_value))])
        }
    }
}

fn parse_simple(operator: &str, text: &str) -> Option<Vec<Comparator>> {
    if text == "*" || text == "x" || text == "X" || text.is_empty() {
        return Some(Vec::new());
    }
    let (major, minor, patch, prerelease_suffix) = split_version_with_suffix(text)?;
    let major_value = parse_numeric_identifier(major?)?;
    let minor_value = minor.and_then(parse_numeric_identifier);
    let patch_value = patch.and_then(parse_numeric_identifier);
    match (minor_value, patch_value) {
        (Some(minor), Some(patch)) => {
            let version =
                SemVer::parse(&format!("{major_value}.{minor}.{patch}{prerelease_suffix}"))?;
            Some(vec![Comparator {
                operator: match operator {
                    ">" => ComparatorOperator::Gt,
                    ">=" => ComparatorOperator::GtEq,
                    "<" => ComparatorOperator::Lt,
                    "<=" => ComparatorOperator::LtEq,
                    _ => ComparatorOperator::Eq,
                },
                version,
            }])
        }
        (Some(minor), None) => match operator {
            // >1.2 := >=1.3.0 ; <=1.2 := <1.3.0-0 ; <1.2 := <1.2.0
            ">" => Some(vec![gte(SemVer {
                major: major_value,
                minor: minor + 1,
                patch: 0,
                prerelease: Vec::new(),
            })]),
            "<=" => Some(vec![lt(upper_minor(major_value, minor))]),
            "<" => Some(vec![lt(SemVer {
                major: major_value,
                minor,
                patch: 0,
                prerelease: Vec::new(),
            })]),
            // =1.2 / 1.2 / >=1.2 : the minor range
            _ => {
                let lower = SemVer {
                    major: major_value,
                    minor,
                    patch: 0,
                    prerelease: Vec::new(),
                };
                if operator == ">=" {
                    return Some(vec![gte(lower)]);
                }
                Some(vec![gte(lower), lt(upper_minor(major_value, minor))])
            }
        },
        (None, None) => match operator {
            // >1 := >=2.0.0 ; <=1 := <2.0.0-0 ; <1 := <1.0.0
            ">" => Some(vec![gte(SemVer {
                major: major_value + 1,
                minor: 0,
                patch: 0,
                prerelease: Vec::new(),
            })]),
            "<=" => Some(vec![lt(upper_major(major_value))]),
            "<" => Some(vec![lt(SemVer {
                major: major_value,
                minor: 0,
                patch: 0,
                prerelease: Vec::new(),
            })]),
            // =1 / 1 / >=1 : the whole major
            _ => {
                let lower = SemVer {
                    major: major_value,
                    minor: 0,
                    patch: 0,
                    prerelease: Vec::new(),
                };
                if operator == ">=" {
                    return Some(vec![gte(lower)]);
                }
                Some(vec![gte(lower), lt(upper_major(major_value))])
            }
        },
        _ => None,
    }
}

/// `M[.m[.p]]` split into numeric-or-x parts plus the raw version suffix.
type VersionParts<'a> = (Option<&'a str>, Option<&'a str>, Option<&'a str>, String);

/// Split `M[.m[.p]][-pre][+build]` into numeric-or-x parts plus the raw
/// suffix (validated by [`SemVer::parse`]).
fn split_version_with_suffix(text: &str) -> Option<VersionParts<'_>> {
    let core_end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'x' || c == 'X' || c == '*'))
        .unwrap_or(text.len());
    let (core, suffix) = text.split_at(core_end);
    if !suffix.is_empty() && !suffix.starts_with('-') && !suffix.starts_with('+') {
        return None;
    }
    if let Some(pre) = suffix.strip_prefix('-') {
        let (pre, _) = split_build(pre)?;
        if pre.is_empty() {
            return None;
        }
    } else if let Some(build) = suffix.strip_prefix('+') {
        let _ = split_build(build)?;
    }
    let mut parts = core.split('.');
    let major = parts.next()?;
    if major.is_empty() {
        return None;
    }
    let is_x = |value: &str| value == "x" || value == "X" || value == "*";
    let major_out = if is_x(major) { None } else { Some(major) };
    let minor_out = match parts.next() {
        Some(value) if is_x(value) => None,
        Some(value) => Some(value),
        None => None,
    };
    let patch_out = match parts.next() {
        Some(value) if is_x(value) => None,
        Some(value) => Some(value),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major_out, minor_out, patch_out, suffix.to_string()))
}

/// node-semver `gt(a, b)`.
pub fn semver_gt(a: &str, b: &str) -> bool {
    match (SemVer::parse(a), SemVer::parse(b)) {
        (Some(a), Some(b)) => a.cmp_version(&b) == Ordering::Greater,
        _ => false,
    }
}

/// node-semver `rcompare(a, b)`: descending order comparator.
pub fn semver_rcompare(a: &str, b: &str) -> Ordering {
    match (SemVer::parse(a), SemVer::parse(b)) {
        (Some(a), Some(b)) => b.cmp_version(&a),
        _ => Ordering::Equal,
    }
}

fn comparator_matches(version: &SemVer, comparator: &Comparator) -> bool {
    let ordering = version.cmp_version(&comparator.version);
    match comparator.operator {
        ComparatorOperator::Lt => ordering == Ordering::Less,
        ComparatorOperator::LtEq => ordering != Ordering::Greater,
        ComparatorOperator::Gt => ordering == Ordering::Greater,
        ComparatorOperator::GtEq => ordering != Ordering::Less,
        ComparatorOperator::Eq => ordering == Ordering::Equal,
    }
}

/// node-semver `satisfies(version, range)` (default options: no prerelease
/// widening, no loose parsing).
pub fn range_satisfies(version: &str, range: &Range) -> bool {
    let Some(parsed) = SemVer::parse(version) else {
        return false;
    };
    range.sets.iter().any(|set| set_matches(&parsed, set))
}

fn set_matches(version: &SemVer, set: &RangeSet) -> bool {
    if !set
        .comparators
        .iter()
        .all(|c| comparator_matches(version, c))
    {
        return false;
    }
    // Prerelease gate: a version carrying a prerelease only satisfies a set
    // when some comparator in the set has the same (major, minor, patch)
    // tuple and itself carries a prerelease.
    if !version.prerelease.is_empty() && !set.any {
        let allowed = set.comparators.iter().any(|comparator| {
            !comparator.version.prerelease.is_empty()
                && comparator.version.major == version.major
                && comparator.version.minor == version.minor
                && comparator.version.patch == version.patch
        });
        if !allowed {
            return false;
        }
    }
    true
}

/// node-semver `maxSatisfying(versions, range)`: the highest version in
/// `versions` satisfying `range` (when given), else the highest overall
/// (`[...versions].sort(rcompare)[0]`).
pub fn max_satisfying(versions: &[&str], range: Option<&Range>) -> Option<String> {
    let candidates: Vec<&str> = versions
        .iter()
        .copied()
        .filter(|version| match range {
            Some(range) => range_satisfies(version, range),
            None => SemVer::parse(version).is_some(),
        })
        .collect();
    // node-semver keeps the first version that compares strictly greater
    // than the current best.
    let mut best: Option<&str> = None;
    for candidate in candidates {
        let greater = match best {
            None => true,
            Some(best) => {
                matches!(
                    (SemVer::parse(candidate), SemVer::parse(best)),
                    (Some(a), Some(b)) if a.cmp_version(&b) == Ordering::Greater
                )
            }
        };
        if greater {
            best = Some(candidate);
        }
    }
    best.map(str::to_string)
}

// ===========================================================================
// minimatch
// ===========================================================================

/// minimatch pattern matching for the corpus used upstream: `/`-split glob
/// patterns with `*`, `?`, `**`, and literal segments; no brace expansion or
/// extglob.
pub fn minimatch(text: &str, pattern: &str) -> bool {
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let text_segments: Vec<&str> = text.split('/').collect();
    match_segments(&pattern_segments, &text_segments)
}

fn match_segments(pattern: &[&str], text: &[&str]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some(&"**") => {
            if pattern.len() == 1 {
                return true;
            }
            // `**` consumes zero or more segments.
            (0..=text.len()).any(|skip| match_segments(&pattern[1..], &text[skip..]))
        }
        Some(first) => match text.first() {
            Some(text_head) if segment_matches(first, text_head) => {
                match_segments(&pattern[1..], &text[1..])
            }
            _ => false,
        },
    }
}

/// Match one path segment against one pattern segment (dotfiles: `*`/`?` do
/// not match a leading `.` unless the pattern segment itself starts with
/// one, as in minimatch's `dot: false` default).
fn segment_matches(pattern: &str, text: &str) -> bool {
    if !pattern.starts_with('.') && text.starts_with('.') {
        return false;
    }
    let regex = segment_regex(pattern);
    regex.is_match(text)
}

fn segment_regex(pattern: &str) -> regex::Regex {
    let mut body = String::from("^");
    let mut characters = pattern.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '*' => {
                while characters.peek() == Some(&'*') {
                    characters.next();
                }
                body.push_str("[^/]*");
            }
            '?' => body.push_str("[^/]"),
            '\\' => {
                if let Some(&next) = characters.peek() {
                    body.push_str(&regex::escape(&next.to_string()));
                    characters.next();
                } else {
                    body.push_str(&regex::escape("\\"));
                }
            }
            other => body.push_str(&regex::escape(&other.to_string())),
        }
    }
    body.push('$');
    regex::Regex::new(&body).unwrap_or_else(|_| regex::Regex::new(r"\A\z").unwrap())
}

// ===========================================================================
// glob (node fs.globSync subset)
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Dir,
}

/// `globSync(pattern, { cwd: root })` for the used pattern subset; returns
/// absolute resolved paths (host separators) sorted lexicographically,
/// restricted to existing filesystem entries. `pattern` may start with
/// `./`; a trailing `/` restricts matches to directories.
pub fn glob_sync(pattern: &str, root: &str) -> Vec<String> {
    let mut normalized = pattern.replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("./") {
        normalized = rest.to_string();
    }
    let directory_only = normalized.ends_with('/');
    let trimmed = normalized.trim_end_matches('/').to_string();
    if trimmed.is_empty() || trimmed == "." {
        return Vec::new();
    }
    let segments: Vec<&str> = trimmed.split('/').collect();
    let mut matches = Vec::new();
    glob_walk(Path::new(root), &segments, directory_only, &mut matches);
    matches.sort();
    matches.dedup();
    matches
}

fn entry_kind(path: &Path) -> Option<EntryKind> {
    // node's readdir(withFileTypes) classifies junctions/symlinks as
    // symbolic (not directories); glob does not follow them.
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Some(EntryKind::Dir),
        Ok(_) => Some(EntryKind::File),
        Err(_) => None,
    }
}

fn push_match(path: &Path, kind: EntryKind, directory_only: bool, out: &mut Vec<String>) {
    if directory_only && kind != EntryKind::Dir {
        return;
    }
    out.push(path.to_string_lossy().into_owned());
}

fn glob_walk(dir: &Path, segments: &[&str], directory_only: bool, out: &mut Vec<String>) {
    let Some(first) = segments.first() else {
        return;
    };
    if *first == "**" {
        glob_walk_star_star(dir, &segments[1..], directory_only, out);
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let child = dir.join(&name);
        if segments.len() == 1 {
            let Some(kind) = entry_kind(&child) else {
                continue;
            };
            if segment_matches(first, &name) {
                push_match(&child, kind, directory_only, out);
            }
        } else if segment_matches(first, &name) && entry_kind(&child) == Some(EntryKind::Dir) {
            glob_walk(&child, &segments[1..], directory_only, out);
        }
    }
}

/// GLOBSTAR: `**` matches zero or more directory levels and never crosses a
/// dot segment.
fn glob_walk_star_star(dir: &Path, rest: &[&str], directory_only: bool, out: &mut Vec<String>) {
    glob_walk(dir, rest, directory_only, out);
    if rest.is_empty() {
        // Trailing `**` matches every descendant (but not the base itself).
        walk_all_descendants(dir, directory_only, out);
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        if name.starts_with('.') {
            continue;
        }
        let child = dir.join(&name);
        if entry_kind(&child) == Some(EntryKind::Dir) {
            glob_walk_star_star(&child, rest, directory_only, out);
        }
    }
}

fn walk_all_descendants(dir: &Path, directory_only: bool, out: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        if name.starts_with('.') {
            continue;
        }
        let child = dir.join(&name);
        let Some(kind) = entry_kind(&child) else {
            continue;
        };
        push_match(&child, kind, directory_only, out);
        if kind == EntryKind::Dir {
            walk_all_descendants(&child, directory_only, out);
        }
    }
}

// ===========================================================================
// ignore (npm `ignore` package call shape)
// ===========================================================================

/// A gitignore matcher equivalent to `ignore().add(rules)` with paths tested
/// relative to a collection root.
pub struct IgnoreMatcher {
    root: PathBuf,
    matcher: Option<ignore::gitignore::Gitignore>,
}

impl IgnoreMatcher {
    /// Build from gitignore rules (already prefixed by the caller via
    /// [`prefix_ignore_pattern`]); rules are added in order, with the last
    /// matching rule deciding the outcome (gitignore semantics).
    pub fn from_rules(root: &str, rules: &[String]) -> IgnoreMatcher {
        let mut builder = GitignoreBuilder::new(root);
        let _ = builder.case_insensitive(false);
        for rule in rules {
            let _ = builder.add_line(None, rule);
        }
        let matcher = builder.build().ok();
        IgnoreMatcher {
            root: PathBuf::from(root),
            matcher,
        }
    }

    /// npm `ignore().ignores(path)`: `true` when the path is ignored. An
    /// excluded parent directory excludes descendants (gitignore semantics
    /// implemented by the npm package), so ancestors participate.
    pub fn ignores(&self, absolute_path: &str, is_dir: bool) -> bool {
        let Some(matcher) = &self.matcher else {
            return false;
        };
        // Upstream probes directories as `${relPath}/` (upstream
        // `collectSkillEntries`: `ig.ignores(`${relPath}/`)`), and npm
        // `ignore` is pure JS — a trailing slash does not change segment
        // matching. The `ignore` crate's gitignore `strip` is byte-wise on
        // unix and keeps the trailing slash in the match candidate, so
        // `**/venv` would never match `<root>/venv/` there (on windows the
        // component-based strip drops it). Trim it; directory-ness stays in
        // `is_dir`.
        let trimmed = absolute_path.trim_end_matches('/');
        let absolute_path: &str = if trimmed.is_empty() {
            absolute_path
        } else {
            trimmed
        };
        let path = Path::new(absolute_path);
        // An excluded parent directory excludes everything beneath it (the
        // npm `ignore` package implements git's no-descent rule): scan
        // ancestors shallowest-first; the first decisive verdict wins.
        let ancestors: Vec<&Path> = path
            .ancestors()
            .skip(1)
            .take_while(|ancestor| ancestor.starts_with(&self.root) && *ancestor != self.root)
            .collect();
        for ancestor in ancestors.iter().rev() {
            match matcher.matched(ancestor, true) {
                ignore::Match::Ignore(_) => return true,
                ignore::Match::Whitelist(_) => continue,
                ignore::Match::None => {}
            }
        }
        matches!(matcher.matched(path, is_dir), ignore::Match::Ignore(_))
    }
}

/// Upstream `prefixIgnorePattern(line, prefix)`: scope an ignore-file rule to
/// its directory (posix-prefixed relative to the collection root). Returns
/// `None` for blank lines and comments.
pub fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }

    let mut pattern = line.to_string();
    let mut negated = false;

    if pattern.starts_with('!') {
        negated = true;
        pattern = pattern[1..].to_string();
    } else if let Some(rest) = pattern.strip_prefix("\\!") {
        pattern = rest.to_string();
    }

    if pattern.starts_with('/') {
        pattern = pattern[1..].to_string();
    }

    let prefixed = if prefix.is_empty() {
        pattern
    } else {
        format!("{}{}", prefix, pattern)
    };
    Some(if negated {
        format!("!{}", prefixed)
    } else {
        prefixed
    })
}
