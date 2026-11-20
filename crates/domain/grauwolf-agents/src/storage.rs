//! Agent storage and the hook that saves progress (ADR-0044, ADR-0052).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use grauwolf_graph::NodeError;
use serde_json::Value;

use crate::checkpoints::{ContextSnapshot, ProgressSaver};

/// Named values of the run and the optional save hook of the running checkpoint.
pub(crate) struct Storage {
    /// Agent storage: named values of the run (ADR-0044).
    values: Mutex<BTreeMap<String, Value>>,
    /// Saves progress under the running checkpoint (ADR-0052).
    progress: Option<ProgressSaver>,
}

impl Storage {
    /// Storage with `values` and no save hook.
    pub(crate) fn new(values: BTreeMap<String, Value>) -> Self {
        Self {
            values: Mutex::new(values),
            progress: None,
        }
    }

    /// Storage of a fork: a copy of the values; branches save no progress of their own, the
    /// caller does that.
    pub(crate) fn fork(&self) -> Self {
        Self::new(self.values())
    }

    /// Copy of the values.
    pub(crate) fn values(&self) -> BTreeMap<String, Value> {
        self.lock().clone()
    }

    /// Replaces all values.
    pub(crate) fn replace(&mut self, values: BTreeMap<String, Value>) {
        *self.values_mut() = values;
    }

    /// The values, given up.
    pub(crate) fn into_values(self) -> BTreeMap<String, Value> {
        self.values
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The values under their lock, for a caller that changes them within one operation.
    pub(crate) fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Value>> {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn values_mut(&mut self) -> &mut BTreeMap<String, Value> {
        self.values
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Removes the value under `key`.
    pub(crate) fn remove(&mut self, key: &str) {
        self.values_mut().remove(key);
    }

    /// Stores `value` under `key` (ADR-0044).
    pub(crate) fn store(
        &mut self,
        key: impl Into<String>,
        value: &impl serde::Serialize,
    ) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(value)?;
        self.values_mut().insert(key.into(), value);
        Ok(())
    }

    /// Value under `key` as `T`; `None` if nothing is stored, error on the wrong type.
    pub(crate) fn load<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, serde_json::Error> {
        self.values()
            .get(key)
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
    }

    /// Connects the storage to the save hook of a checkpoint observer (ADR-0052).
    pub(crate) fn set_progress_saver(&mut self, saver: Option<ProgressSaver>) {
        self.progress = saver;
    }

    /// Hands the snapshot made by `snapshot` to the save hook; without a hook it does
    /// nothing and makes no snapshot (ADR-0052).
    pub(crate) async fn save_progress(
        &self,
        snapshot: impl FnOnce() -> ContextSnapshot,
    ) -> Result<(), NodeError> {
        match &self.progress {
            Some(saver) => saver(snapshot()).await,
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use grauwolf_prompt::{LlModel, Prompt};
    use serde_json::json;

    use super::*;

    fn snapshot() -> ContextSnapshot {
        ContextSnapshot {
            prompt: Prompt::builder("p").build(),
            model: LlModel::new("test", "m"),
            storage: BTreeMap::new(),
            self_tools: Vec::new(),
            usage: None,
            cost: None,
            tool_calls: 0,
        }
    }

    #[test]
    fn stored_value_is_loaded_as_its_type() {
        let mut storage = Storage::new(BTreeMap::new());
        storage.store("numbers", &vec![1, 2, 3]).unwrap();
        assert_eq!(
            storage.load::<Vec<i32>>("numbers").unwrap(),
            Some(vec![1, 2, 3])
        );
        assert_eq!(storage.values()["numbers"], json!([1, 2, 3]));
    }

    #[test]
    fn missing_key_loads_as_none_and_wrong_type_is_an_error() {
        let mut storage = Storage::new(BTreeMap::new());
        assert_eq!(storage.load::<String>("nothing").unwrap(), None);
        storage.store("text", &"hello").unwrap();
        assert!(storage.load::<u32>("text").is_err());
    }

    #[test]
    fn removed_value_is_gone() {
        let mut storage = Storage::new(BTreeMap::from([("a".to_owned(), json!(1))]));
        storage.remove("a");
        storage.remove("never-there");
        assert!(storage.values().is_empty());
    }

    #[test]
    fn fork_copies_the_values_and_drops_the_save_hook() {
        let mut storage = Storage::new(BTreeMap::new());
        storage.store("a", &1).unwrap();
        storage.set_progress_saver(Some(Arc::new(|_| Box::pin(async { Ok(()) }))));
        let mut fork = storage.fork();
        assert_eq!(fork.load::<i32>("a").unwrap(), Some(1));
        fork.store("b", &2).unwrap();
        assert_eq!(storage.load::<i32>("b").unwrap(), None);
        assert!(fork.progress.is_none());
    }

    #[test]
    fn replace_and_into_values_give_the_whole_map() {
        let mut storage = Storage::new(BTreeMap::from([("a".to_owned(), json!(1))]));
        storage.replace(BTreeMap::from([("b".to_owned(), json!(2))]));
        assert_eq!(
            storage.into_values(),
            BTreeMap::from([("b".to_owned(), json!(2))])
        );
    }

    #[test]
    fn save_progress_without_a_hook_makes_no_snapshot() {
        let storage = Storage::new(BTreeMap::new());
        let made = AtomicUsize::new(0);
        futures_executor::block_on(storage.save_progress(|| {
            made.fetch_add(1, Ordering::SeqCst);
            snapshot()
        }))
        .unwrap();
        assert_eq!(made.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn save_progress_hands_the_snapshot_to_the_hook() {
        let saved = Arc::new(AtomicUsize::new(0));
        let mut storage = Storage::new(BTreeMap::new());
        let counter = Arc::clone(&saved);
        storage.set_progress_saver(Some(Arc::new(move |snapshot| {
            counter.fetch_add(snapshot.tool_calls as usize + 1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        })));
        futures_executor::block_on(storage.save_progress(snapshot)).unwrap();
        assert_eq!(saved.load(Ordering::SeqCst), 1);
    }
}
