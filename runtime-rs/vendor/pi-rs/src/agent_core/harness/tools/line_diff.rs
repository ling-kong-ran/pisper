//! Default line diff and FILE_HEADERS_ONLY unified patch semantics of jsdiff
//! 8.0.4 (upstream's exact dependency). Myers path selection/tie-breaking and
//! context grouping are preserved. See docs/migration/reference/jsdiff-LICENSE.
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Equal,
    Add,
    Remove,
}
#[derive(Debug, Clone)]
pub(super) struct Part {
    pub kind: Change,
    pub value: String,
}
struct Component {
    kind: Change,
    count: usize,
    previous: Option<Arc<Component>>,
}
#[derive(Clone)]
struct Path {
    old: i64,
    last: Option<Arc<Component>>,
}
fn add(path: &Path, kind: Change) -> Path {
    let (count, previous) = match &path.last {
        Some(last) if last.kind == kind => (last.count + 1, last.previous.clone()),
        _ => (1, path.last.clone()),
    };
    Path {
        old: path.old + i64::from(kind == Change::Remove),
        last: Some(Arc::new(Component {
            kind,
            count,
            previous,
        })),
    }
}
fn common(path: &mut Path, old: &[&str], new: &[&str], diagonal: i64) -> i64 {
    let mut new_pos = path.old - diagonal;
    let mut count = 0;
    while (new_pos + 1) < new.len() as i64
        && (path.old + 1) < old.len() as i64
        && old[(path.old + 1) as usize] == new[(new_pos + 1) as usize]
    {
        path.old += 1;
        new_pos += 1;
        count += 1;
    }
    if count > 0 {
        path.last = Some(Arc::new(Component {
            kind: Change::Equal,
            count,
            previous: path.last.clone(),
        }));
    }
    new_pos
}
fn values(path: Path, old: &[&str], new: &[&str]) -> Vec<Part> {
    let mut components = Vec::new();
    let mut last = path.last;
    while let Some(item) = last {
        components.push((item.kind, item.count));
        last = item.previous.clone();
    }
    components.reverse();
    let (mut old_pos, mut new_pos) = (0, 0);
    components
        .into_iter()
        .map(|(kind, count)| {
            let value = if kind == Change::Remove {
                let value = old[old_pos..old_pos + count].concat();
                old_pos += count;
                value
            } else {
                let value = new[new_pos..new_pos + count].concat();
                new_pos += count;
                if kind == Change::Equal {
                    old_pos += count;
                }
                value
            };
            Part { kind, value }
        })
        .collect()
}
pub(super) fn diff_lines(old: &str, new: &str) -> Vec<Part> {
    let old: Vec<_> = old.split_inclusive('\n').collect();
    let new: Vec<_> = new.split_inclusive('\n').collect();
    let mut seed = Path {
        old: -1,
        last: None,
    };
    let np = common(&mut seed, &old, &new, 0);
    if seed.old + 1 >= old.len() as i64 && np + 1 >= new.len() as i64 {
        return values(seed, &old, &new);
    }
    let mut best = HashMap::from([(0i64, seed)]);
    let (mut min_diagonal, mut max_diagonal) = (i64::MIN, i64::MAX);
    for distance in 1..=(old.len() + new.len()) as i64 {
        let mut diagonal = min_diagonal.max(-distance);
        while diagonal <= max_diagonal.min(distance) {
            let remove = best.remove(&(diagonal - 1));
            let add_path = best.get(&(diagonal + 1)).cloned();
            let can_add = add_path.as_ref().is_some_and(|path| {
                let np = path.old - diagonal;
                0 <= np && np < new.len() as i64
            });
            let can_remove = remove
                .as_ref()
                .is_some_and(|path| path.old + 1 < old.len() as i64);
            if !can_add && !can_remove {
                best.remove(&diagonal);
                diagonal += 2;
                continue;
            }
            let mut path = if !can_remove
                || (can_add && remove.as_ref().unwrap().old < add_path.as_ref().unwrap().old)
            {
                add(add_path.as_ref().unwrap(), Change::Add)
            } else {
                add(remove.as_ref().unwrap(), Change::Remove)
            };
            let np = common(&mut path, &old, &new, diagonal);
            if path.old + 1 >= old.len() as i64 && np + 1 >= new.len() as i64 {
                return values(path, &old, &new);
            }
            if path.old + 1 >= old.len() as i64 {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if np + 1 >= new.len() as i64 {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            best.insert(diagonal, path);
            diagonal += 2;
        }
    }
    unreachable!("unbounded edit graph always has a path")
}

pub fn generate_unified_patch(path: &str, old: &str, new: &str, context: usize) -> String {
    let mut parts = diff_lines(old, new);
    parts.push(Part {
        kind: Change::Equal,
        value: String::new(),
    });
    let lines: Vec<Vec<&str>> = parts
        .iter()
        .map(|part| part.value.split_inclusive('\n').collect())
        .collect();
    let (mut old_start, mut new_start, mut old_line, mut new_line) =
        (0usize, 0usize, 1usize, 1usize);
    let mut range = Vec::<String>::new();
    let mut output = vec![format!("--- {path}"), format!("+++ {path}")];
    for (i, part) in parts.iter().enumerate() {
        let current = &lines[i];
        if part.kind != Change::Equal {
            if old_start == 0 {
                old_start = old_line;
                new_start = new_line;
                if i > 0 {
                    let previous = &lines[i - 1];
                    let start = previous.len().saturating_sub(context);
                    range = previous[start..]
                        .iter()
                        .map(|line| format!(" {line}"))
                        .collect();
                    old_start -= range.len();
                    new_start -= range.len();
                }
            }
            let prefix = if part.kind == Change::Add { '+' } else { '-' };
            range.extend(current.iter().map(|line| format!("{prefix}{line}")));
            if part.kind == Change::Add {
                new_line += current.len();
            } else {
                old_line += current.len();
            }
        } else {
            if old_start != 0 {
                if current.len() <= context.saturating_mul(2) && i < parts.len().saturating_sub(2) {
                    range.extend(current.iter().map(|line| format!(" {line}")));
                } else {
                    let count = current.len().min(context);
                    range.extend(current[..count].iter().map(|line| format!(" {line}")));
                    let old_count = old_line - old_start + count;
                    let new_count = new_line - new_start + count;
                    output.push(format!(
                        "@@ -{},{} +{},{} @@",
                        old_start - usize::from(old_count == 0),
                        old_count,
                        new_start - usize::from(new_count == 0),
                        new_count
                    ));
                    for line in range.drain(..) {
                        if let Some(line) = line.strip_suffix('\n') {
                            output.push(line.to_string());
                        } else {
                            output.push(line);
                            output.push("\\ No newline at end of file".to_string());
                        }
                    }
                    old_start = 0;
                    new_start = 0;
                }
            }
            old_line += current.len();
            new_line += current.len();
        }
    }
    output.join("\n") + "\n"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayDiff {
    pub diff: String,
    pub first_changed_line: Option<usize>,
}
pub fn generate_diff_string(old: &str, new: &str, context: usize) -> DisplayDiff {
    let parts = diff_lines(old, new);
    let width = old
        .split('\n')
        .count()
        .max(new.split('\n').count())
        .to_string()
        .len();
    let (mut old_line, mut new_line) = (1usize, 1usize);
    let mut last_change = false;
    let mut first = None;
    let mut output = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let mut raw: Vec<_> = part.value.split('\n').collect();
        if raw.last() == Some(&"") {
            raw.pop();
        }
        if part.kind != Change::Equal {
            first.get_or_insert(new_line);
            for line in raw {
                if part.kind == Change::Add {
                    output.push(format!("+{new_line:>width$} {line}"));
                    new_line += 1;
                } else {
                    output.push(format!("-{old_line:>width$} {line}"));
                    old_line += 1;
                }
            }
            last_change = true;
        } else {
            let trailing = parts.get(i + 1).is_some_and(|p| p.kind != Change::Equal);
            let len = raw.len();
            let (head, tail) = match (last_change, trailing) {
                (true, true) if len <= context.saturating_mul(2) => (len, 0),
                (true, true) => (context, context),
                (true, false) => (len.min(context), 0),
                (false, true) => (0, len.min(context)),
                (false, false) => {
                    old_line += len;
                    new_line += len;
                    last_change = false;
                    continue;
                }
            };
            for line in &raw[..head] {
                output.push(format!(" {old_line:>width$} {line}"));
                old_line += 1;
                new_line += 1;
            }
            let skipped = len - head - tail;
            if skipped > 0 {
                output.push(format!(" {:>width$} ...", ""));
                old_line += skipped;
                new_line += skipped;
            }
            for line in &raw[len - tail..] {
                output.push(format!(" {old_line:>width$} {line}"));
                old_line += 1;
                new_line += 1;
            }
            last_change = false;
        }
    }
    DisplayDiff {
        diff: output.join("\n"),
        first_changed_line: first,
    }
}
