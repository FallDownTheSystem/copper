//! Session-scoped, in-memory undo. Never persisted.
//!
//! Whole-document snapshots rather than inverse operations. A structural op
//! touches ordering across a whole section and sometimes across two, so an
//! inverse would have to be written and tested per operation; a snapshot is one
//! mechanism for all of them and a `.copper` document is tens of kilobytes.
//!
//! The cost of that choice is stated in spec 4.3 and is real: a structural undo
//! restores a whole document, so it can revert a body edit that happened after
//! the snapshot was taken. Text editing lives in the browser's native undo
//! precisely because merging the two histories is not worth the machinery.

use std::collections::VecDeque;

use super::model::Space;

/// Spec 4.1. Fifty is a session's worth of structural work, and fifty documents
/// is a few megabytes at worst.
const CAPACITY: usize = 50;

#[derive(Default, Debug)]
pub struct UndoStack {
	undo: VecDeque<Space>,
	redo: Vec<Space>,
}

impl UndoStack {
	/// Records the pre-operation document. Any new structural operation
	/// invalidates the redo branch (spec 4.4).
	pub fn push(&mut self, doc: Space) {
		if self.undo.len() == CAPACITY {
			self.undo.pop_front();
		}
		self.undo.push_back(doc);
		self.redo.clear();
	}

	/// The document `undo` would restore, without consuming it.
	///
	/// Peeking rather than popping is what makes spec 4.7 possible: the write has
	/// to succeed before the stack moves, so a failed undo leaves the user
	/// exactly where they were and able to try again.
	pub fn peek_undo(&self) -> Option<&Space> {
		self.undo.back()
	}

	pub fn peek_redo(&self) -> Option<&Space> {
		self.redo.last()
	}

	/// Commits an undo that has already been written: drops the restored entry
	/// and files `current` — the document as it was before the undo — under redo.
	pub fn commit_undo(&mut self, current: Space) {
		self.undo.pop_back();
		self.redo.push(current);
	}

	/// The mirror of `commit_undo`.
	pub fn commit_redo(&mut self, current: Space) {
		self.redo.pop();
		if self.undo.len() == CAPACITY {
			self.undo.pop_front();
		}
		self.undo.push_back(current);
	}

	/// Both stacks go on an external reload (spec 4.6) — the document they
	/// describe is no longer the document on disk.
	pub fn clear(&mut self) {
		self.undo.clear();
		self.redo.clear();
	}

	pub fn can_undo(&self) -> bool {
		!self.undo.is_empty()
	}

	pub fn can_redo(&self) -> bool {
		!self.redo.is_empty()
	}

	pub fn undo_depth(&self) -> usize {
		self.undo.len()
	}

	pub fn redo_depth(&self) -> usize {
		self.redo.len()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn doc(name: &str) -> Space {
		Space {
			id: "spc_00000001".into(),
			name: name.into(),
			active_section: "sec_00000001".into(),
			sections: Vec::new(),
			notes: Vec::new(),
		}
	}

	#[test]
	fn sixty_pushes_keep_the_newest_fifty() {
		let mut stack = UndoStack::default();
		for index in 0..60 {
			stack.push(doc(&index.to_string()));
		}
		assert_eq!(stack.undo_depth(), CAPACITY);
		assert_eq!(stack.peek_undo().unwrap().name, "59");
		// The oldest ten are gone: entry 9 is now the front.
		assert_eq!(stack.undo.front().unwrap().name, "10");
	}

	#[test]
	fn an_empty_stack_restores_nothing() {
		let stack = UndoStack::default();
		assert!(stack.peek_undo().is_none());
		assert!(stack.peek_redo().is_none());
		assert!(!stack.can_undo());
		assert!(!stack.can_redo());
	}

	#[test]
	fn undo_and_redo_alternate() {
		let mut stack = UndoStack::default();
		stack.push(doc("a"));
		stack.push(doc("b"));

		// Current document is "c"; undoing restores "b".
		assert_eq!(stack.peek_undo().unwrap().name, "b");
		stack.commit_undo(doc("c"));
		assert_eq!(stack.peek_undo().unwrap().name, "a");
		assert_eq!(stack.peek_redo().unwrap().name, "c");

		stack.commit_undo(doc("b"));
		assert!(!stack.can_undo());
		assert_eq!(stack.redo_depth(), 2);

		assert_eq!(stack.peek_redo().unwrap().name, "b");
		stack.commit_redo(doc("a"));
		assert_eq!(stack.peek_redo().unwrap().name, "c");
		assert_eq!(stack.peek_undo().unwrap().name, "a");
	}

	#[test]
	fn a_new_operation_clears_redo() {
		let mut stack = UndoStack::default();
		stack.push(doc("a"));
		stack.commit_undo(doc("b"));
		assert!(stack.can_redo());

		stack.push(doc("c"));
		assert!(!stack.can_redo());
	}

	#[test]
	fn clear_empties_both_stacks() {
		let mut stack = UndoStack::default();
		stack.push(doc("a"));
		stack.commit_undo(doc("b"));

		stack.clear();
		assert!(!stack.can_undo());
		assert!(!stack.can_redo());
	}

	#[test]
	fn redo_respects_the_cap() {
		let mut stack = UndoStack::default();
		for index in 0..CAPACITY {
			stack.push(doc(&index.to_string()));
		}
		stack.commit_undo(doc("current"));
		stack.commit_redo(doc("restored"));
		assert_eq!(stack.undo_depth(), CAPACITY);
	}
}
