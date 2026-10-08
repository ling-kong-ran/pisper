//! Port of upstream `packages/tui/src/undo-stack.ts`: a generic undo stack
//! with clone-on-push semantics. Popped snapshots are returned directly
//! (they are already detached).

/// Upstream `UndoStack<S>`.
#[derive(Clone, Debug)]
pub struct UndoStack<S: Clone> {
    stack: Vec<S>,
}

impl<S: Clone> Default for UndoStack<S> {
    fn default() -> Self {
        Self { stack: Vec::new() }
    }
}

impl<S: Clone> UndoStack<S> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a clone of the given state onto the stack.
    pub fn push(&mut self, state: &S) {
        self.stack.push(state.clone());
    }

    /// Pop and return the most recent snapshot, if any.
    pub fn pop(&mut self) -> Option<S> {
        self.stack.pop()
    }

    /// Remove all snapshots.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}
