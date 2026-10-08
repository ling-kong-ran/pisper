//! Immutable render output used by viewport layout. Sparse rows preserve the
//! upstream array length without allocating a String for every absent element.
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum RenderedLines {
    Dense(Arc<Vec<String>>),
    Sparse {
        length: usize,
        lines: Arc<BTreeMap<usize, String>>,
    },
}

impl RenderedLines {
    pub fn dense(lines: Vec<String>) -> Self {
        Self::Dense(Arc::new(lines))
    }
    /// Missing rows are absent (not the empty string), as in a sparse JS array.
    pub fn sparse(length: usize, lines: BTreeMap<usize, String>) -> Self {
        assert!(
            lines.keys().all(|&row| row < length),
            "sparse row outside logical length"
        );
        Self::Sparse {
            length,
            lines: Arc::new(lines),
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Dense(lines) => lines.len(),
            Self::Sparse { length, .. } => *length,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn get(&self, row: usize) -> Option<&str> {
        match self {
            Self::Dense(lines) => lines.get(row).map(String::as_str),
            Self::Sparse { lines, .. } => lines.get(&row).map(String::as_str),
        }
    }
    pub fn present(&self) -> Box<dyn Iterator<Item = (usize, &str)> + '_> {
        match self {
            Self::Dense(lines) => Box::new(lines.iter().enumerate().map(|(i, s)| (i, s.as_str()))),
            Self::Sparse { lines, .. } => Box::new(lines.iter().map(|(&i, s)| (i, s.as_str()))),
        }
    }
    /// Equivalent to scanning backwards across empty/absent rows, but bounded by
    /// stored data rather than logical length for sparse transcripts.
    pub fn last_nonempty_before(&self, row: usize) -> Option<(usize, &str)> {
        match self {
            Self::Dense(lines) => lines[..row.min(lines.len())]
                .iter()
                .enumerate()
                .rev()
                .find(|(_, s)| !s.is_empty())
                .map(|(i, s)| (i, s.as_str())),
            Self::Sparse { lines, .. } => lines
                .range(..row)
                .rev()
                .find(|(_, s)| !s.is_empty())
                .map(|(&i, s)| (i, s.as_str())),
        }
    }
    pub fn map_present(&self, mut map: impl FnMut(&str) -> String) -> Self {
        match self {
            Self::Dense(lines) => Self::dense(lines.iter().map(|s| map(s)).collect()),
            Self::Sparse { length, lines } => {
                Self::sparse(*length, lines.iter().map(|(&i, s)| (i, map(s))).collect())
            }
        }
    }
}
