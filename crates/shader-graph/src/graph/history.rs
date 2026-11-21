/// Undo and redo stack sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryCounts {
    pub undo: usize,
    pub redo: usize,
}

/// Linear undo/redo history of opaque states (project content snapshots).
///
/// `record` pushes a new state (dropping the redo branch); recording the
/// current state again is a no-op, so callers can record eagerly.
#[derive(Clone, Debug)]
pub struct History {
    past: Vec<String>,
    future: Vec<String>,
    current: Option<String>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(200)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self { past: Vec::new(), future: Vec::new(), current: None, limit }
    }

    pub fn counts(&self) -> HistoryCounts {
        HistoryCounts { undo: self.past.len(), redo: self.future.len() }
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Forget everything and start from `state`.
    pub fn reset(&mut self, state: String) {
        self.past.clear();
        self.future.clear();
        self.current = Some(state);
    }

    /// Record `state` as the newest step. Returns false when it equals the current state.
    pub fn record(&mut self, state: String) -> bool {
        if self.current.as_ref() == Some(&state) {
            return false;
        }
        if let Some(current) = self.current.take() {
            self.past.push(current);
            if self.past.len() > self.limit {
                self.past.remove(0);
            }
        }
        self.current = Some(state);
        self.future.clear();
        true
    }

    /// Replace the current state without adding a step.
    pub fn replace_current(&mut self, state: String) {
        self.current = Some(state);
    }

    /// Step back; returns the state to restore, if any.
    pub fn undo(&mut self) -> Option<String> {
        let previous = self.past.pop()?;
        let current = self.current.replace(previous.clone())?;
        self.future.push(current);
        Some(previous)
    }

    /// Step forward; returns the state to restore, if any.
    pub fn redo(&mut self) -> Option<String> {
        let next = self.future.pop()?;
        let current = self.current.replace(next.clone())?;
        self.past.push(current);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undoes_and_redoes() {
        let mut h = History::default();
        h.reset("a".into());
        assert!(h.record("b".into()));
        assert!(!h.record("b".into()));
        assert!(h.record("c".into()));
        assert_eq!(h.undo().as_deref(), Some("b"));
        assert_eq!(h.undo().as_deref(), Some("a"));
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo().as_deref(), Some("b"));
        assert!(h.record("d".into()));
        assert!(!h.can_redo());
    }
}
