//! Thread-affine browser cache. No JS objects or target checks escape platform.
use super::indexed_db::AsyncPlatformStorage;
use crate::{GenerationRecord, RuntimeStorageStatus, StorageError, WriteQueued, validate_key};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};

#[derive(Clone, Debug)]
pub struct BufferedStorage {
    namespace: String,
}
struct Job {
    namespace: String,
    records: Vec<GenerationRecord>,
}
struct State {
    status: RuntimeStorageStatus,
    project: String,
    cache: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    jobs: VecDeque<Job>,
}
thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        status: RuntimeStorageStatus::Uninitialized, project: String::new(),
        cache: BTreeMap::new(), jobs: VecDeque::new(),
    });
}
fn backend(project: &str, namespace: &str) -> AsyncPlatformStorage {
    AsyncPlatformStorage::new("", format!("{project}.{namespace}"), "")
}
fn failure(message: impl Into<String>) -> StorageError {
    StorageError::Browser(message.into())
}

impl BufferedStorage {
    pub fn new(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        let _ = (root.into(), extension.into());
        Self {
            namespace: namespace.into(),
        }
    }
    pub fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        validate_key(key)?;
        STATE.with(|state| {
            let state = state.borrow();
            if let RuntimeStorageStatus::Failed(error) = &state.status {
                return Err(failure(error));
            }
            let cache = state
                .cache
                .get(&self.namespace)
                .ok_or_else(|| failure("storage cache has not initialized"))?;
            Ok(cache.get(key).cloned())
        })
    }
    pub fn contains(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.read(key)?.is_some())
    }
    pub fn enqueue_write(&self, key: &str, payload: &[u8]) -> Result<WriteQueued, StorageError> {
        self.enqueue_generation(vec![GenerationRecord {
            key: key.into(),
            extension: "bin".into(),
            payload: payload.to_vec(),
        }])
    }
    pub fn enqueue_generation(
        &self,
        records: Vec<GenerationRecord>,
    ) -> Result<WriteQueued, StorageError> {
        crate::validate_generation(&records)?;
        let start = STATE.with(|state| {
            let mut state = state.borrow_mut();
            if !matches!(
                state.status,
                RuntimeStorageStatus::Ready | RuntimeStorageStatus::Writing
            ) {
                return Err(failure("storage is not available for writes"));
            }
            let cache = state
                .cache
                .get_mut(&self.namespace)
                .ok_or_else(|| failure("unknown storage domain"))?;
            // Read-your-writes within an invocation. A failed commit halts the
            // engine; this speculative cache is never reported as durable.
            if records[..records.len() - 1]
                .iter()
                .any(|record| cache.contains_key(&record.key))
            {
                return Err(failure("generation already exists"));
            }
            for record in &records {
                cache.insert(record.key.clone(), record.payload.clone());
            }
            state.jobs.push_back(Job {
                namespace: self.namespace.clone(),
                records,
            });
            let start = state.status == RuntimeStorageStatus::Ready;
            state.status = RuntimeStorageStatus::Writing;
            Ok(start)
        })?;
        if start {
            wasm_bindgen_futures::spawn_local(flush());
        }
        Ok(WriteQueued)
    }
}

pub fn initialize_runtime(project: &str, stores: Vec<BufferedStorage>) {
    let project = project.to_owned();
    let begin = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !matches!(state.status, RuntimeStorageStatus::Uninitialized) {
            return false;
        }
        state.project = project.clone();
        state.status = RuntimeStorageStatus::Loading;
        true
    });
    if !begin {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            let mut caches = BTreeMap::new();
            for store in stores {
                let data = backend(&project, &store.namespace)
                    .initialize_legacy(&store.namespace)
                    .await?;
                caches.insert(store.namespace, data);
            }
            Ok::<_, StorageError>(caches)
        }
        .await;
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            match result {
                Ok(caches) => {
                    state.cache = caches;
                    state.status = RuntimeStorageStatus::Ready;
                }
                Err(error) => state.status = RuntimeStorageStatus::Failed(error.to_string()),
            }
        });
    });
}

async fn flush() {
    loop {
        let job = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let job = state.jobs.pop_front();
            if job.is_none() {
                state.status = RuntimeStorageStatus::Ready;
            }
            job.map(|job| (state.project.clone(), job))
        });
        let Some((project, job)) = job else { return };
        if let Err(error) = backend(&project, &job.namespace)
            .write_generation(&job.records)
            .await
        {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.jobs.clear();
                state.status = RuntimeStorageStatus::Failed(format!("{}: {error}", job.namespace));
            });
            return;
        }
    }
}
pub fn runtime_status() -> RuntimeStorageStatus {
    STATE.with(|state| state.borrow().status.clone())
}
