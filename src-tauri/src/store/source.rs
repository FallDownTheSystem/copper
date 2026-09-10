//! Deferred actions identify both the file and the document they came from.

use std::path::PathBuf;

use serde::Deserialize;

use copper_core::store::error::{Result, StoreError};
use copper_core::store::model::Space;
use copper_core::store::{lock, SharedStore, Store};

#[derive(Clone, Debug, Deserialize)]
pub struct DocumentSource {
	pub path: String,
	pub id: String,
}

impl DocumentSource {
	pub fn snapshot(&self, state: &SharedStore) -> Result<(PathBuf, Space)> {
		let store = lock(state);
		let path = self.require_path(&store)?;
		let space = store.active_space()?;
		self.require_document(&space)?;
		Ok((path, space))
	}

	pub fn require_path(&self, store: &Store) -> Result<PathBuf> {
		let path = store.require_active_path()?;
		if path.to_str() != Some(self.path.as_str()) {
			return Err(changed());
		}
		Ok(path)
	}

	pub fn require_document(&self, space: &Space) -> Result<()> {
		if space.id != self.id {
			return Err(changed());
		}
		Ok(())
	}
}

fn changed() -> StoreError {
	StoreError::Invalid("the open space changed; select the items again".into())
}

#[cfg(test)]
mod tests {
	use super::*;
	use copper_core::store::events::NullSink;
	use std::sync::{Arc, Mutex};

	fn copied_note() -> (tempfile::TempDir, SharedStore, DocumentSource, String) {
		let dir = tempfile::tempdir().unwrap();
		let store = copper_core::store::bootstrap_store(dir.path(), Arc::new(NullSink)).unwrap();
		let source = DocumentSource {
			path: store
				.require_active_path()
				.unwrap()
				.to_str()
				.unwrap()
				.into(),
			id: store.active_space().unwrap().id,
		};
		let shared = Arc::new(Mutex::new(store));
		let added = crate::store::commands::add(&shared, "copied note", None).unwrap();
		(dir, shared, source, added.note_id)
	}

	#[test]
	fn completion_cannot_mutate_another_file_with_identical_ids() {
		let (dir, shared, source, id) = copied_note();
		let other = dir.path().join("copy.copper");
		std::fs::write(&other, lock(&shared).on_disk_text().unwrap()).unwrap();
		copper_core::store::open_space(&shared, &other).unwrap();
		let result = crate::store::commands::set_done(&shared, &[id.clone()], true, Some(&source));
		assert!(result.is_err());
		assert!(
			!lock(&shared)
				.active_space()
				.unwrap()
				.note(&id)
				.unwrap()
				.done
		);
	}

	#[test]
	fn completion_checks_document_identity_again_when_the_store_rebases() {
		let (_dir, shared, source, id) = copied_note();
		let mut replacement = lock(&shared).active_space().unwrap();
		replacement.id = "external-replacement".into();
		let text = copper_core::store::format::to_git_json(&replacement).unwrap();
		std::fs::write(&source.path, &text).unwrap();
		let result = crate::store::commands::set_done(&shared, &[id], true, Some(&source));
		assert!(result.is_err());
		assert_eq!(std::fs::read_to_string(&source.path).unwrap(), text);
	}

	#[test]
	fn completion_updates_the_same_document_and_remains_undoable() {
		let (_dir, shared, source, id) = copied_note();
		let updated =
			crate::store::commands::set_done(&shared, &[id.clone()], true, Some(&source)).unwrap();
		assert!(updated.note(&id).unwrap().done);
		assert!(lock(&shared).status().can_undo);
	}

	#[test]
	fn the_same_path_cannot_substitute_a_different_document() {
		let dir = tempfile::tempdir().unwrap();
		let store = copper_core::store::bootstrap_store(dir.path(), Arc::new(NullSink)).unwrap();
		let source = DocumentSource {
			path: store
				.require_active_path()
				.unwrap()
				.to_str()
				.unwrap()
				.into(),
			id: store.active_space().unwrap().id,
		};
		let shared = Arc::new(Mutex::new(store));
		assert!(source.snapshot(&shared).is_ok());
		lock(&shared)
			.mutate(|doc| {
				doc.id = "replacement-document".into();
				Ok(())
			})
			.unwrap();
		assert!(source.snapshot(&shared).is_err());
	}
}
