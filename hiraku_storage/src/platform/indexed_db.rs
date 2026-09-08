//! IndexedDB bindings stay entirely inside the browser platform backend.
use crate::{AsyncByteStorage, StorageError, validate_key};
use js_sys::{Promise, Uint8Array};
use std::path::PathBuf;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Event, IdbDatabase, IdbRequest, IdbTransaction, IdbTransactionMode};

const STORE: &str = "bytes";

/// Namespace should include the project's stable ID and logical data domain.
#[derive(Clone, Debug)]
pub struct AsyncPlatformStorage {
    database: String,
}

impl AsyncPlatformStorage {
    pub(super) async fn write_generation(&self, records: &[crate::GenerationRecord]) -> Result<(), StorageError> {
        crate::validate_generation(records)?;
        let db = self.open().await?;
        let tx = db.0.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite).map_err(js_error)?;
        let completion = TransactionWait::new(tx.clone());
        let store = tx.object_store(STORE).map_err(js_error)?;
        for (index, record) in records.iter().enumerate() {
            let bytes = Uint8Array::from(record.payload.as_slice());
            if index + 1 == records.len() {
                store.put_with_key(&bytes, &record.key.as_str().into()).map_err(js_error)?;
            } else {
                store.add_with_key(&bytes, &record.key.as_str().into()).map_err(js_error)?;
            }
        }
        completion.wait().await
    }
    /// Import legacy records once, without overwriting any IndexedDB record.
    /// Data and marker commit in the same transaction; localStorage is retained.
    pub(super) async fn initialize_legacy(&self, namespace: &str) -> Result<std::collections::BTreeMap<String, Vec<u8>>, StorageError> {
        const MARKER: &str = "__hiraku_local_storage_import_v1";
        let mut data = self.read_all().await?;
        if data.remove(MARKER).is_some() { return Ok(data) }
        let window = web_sys::window().ok_or_else(|| error("window unavailable"))?;
        let legacy = window.local_storage().map_err(js_error)?;
        let mut imported = std::collections::BTreeMap::new();
        if let Some(legacy) = legacy {
            let prefix = format!("{namespace}.");
            for index in 0..legacy.length().map_err(js_error)? {
                let Some(full_key) = legacy.key(index).map_err(js_error)? else { continue };
                let Some(key) = full_key.strip_prefix(&prefix) else { continue };
                validate_key(key)?;
                if key == MARKER || data.contains_key(key) { continue }
                if let Some(payload) = legacy.get_item(&full_key).map_err(js_error)? {
                    imported.insert(key.to_owned(), super::wasm::decode_hex(&payload)?);
                }
            }
        }
        let db = self.open().await?;
        let tx = db.0.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite).map_err(js_error)?;
        let completion = TransactionWait::new(tx.clone());
        let store = tx.object_store(STORE).map_err(js_error)?;
        for (key, payload) in &imported {
            // A concurrent initializer can win this race. add aborts rather
            // than overwriting it; restarting safely repeats the migration.
            store.add_with_key(&Uint8Array::from(payload.as_slice()), &key.as_str().into()).map_err(js_error)?;
        }
        store.put_with_key(&Uint8Array::from(&b"1"[..]), &MARKER.into()).map_err(js_error)?;
        completion.wait().await?;
        data.extend(imported);
        Ok(data)
    }

    async fn read_all(&self) -> Result<std::collections::BTreeMap<String, Vec<u8>>, StorageError> {
        let db = self.open().await?;
        let tx = db.0.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readonly).map_err(js_error)?;
        let completion = TransactionWait::new(tx.clone());
        let store = tx.object_store(STORE).map_err(js_error)?;
        let keys = RequestWait::new(store.get_all_keys().map_err(js_error)?);
        let values = RequestWait::new(store.get_all().map_err(js_error)?);
        let keys = keys.wait().await;
        let values = values.wait().await;
        let committed = completion.wait().await;
        let keys = js_sys::Array::from(&keys?);
        let values = js_sys::Array::from(&values?);
        committed?;
        if keys.length() != values.length() { return Err(error("inconsistent IndexedDB snapshot")) }
        let mut data = std::collections::BTreeMap::new();
        for index in 0..keys.length() {
            let key = keys.get(index).as_string().ok_or_else(|| error("non-string storage key"))?;
            let value = values.get(index);
            if !value.is_instance_of::<Uint8Array>() { return Err(StorageError::Corrupt("non-binary storage record".into())) }
            data.insert(key, Uint8Array::new(&value).to_vec());
        }
        Ok(data)
    }

    pub fn new(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        let _ = (root.into(), extension.into());
        Self {
            database: format!("hiraku.idb.{}", namespace.into()),
        }
    }

    async fn open(&self) -> Result<Connection, StorageError> {
        // An IDB open request cannot be aborted. Keep its handlers alive even
        // when the caller drops its future; an unclaimed connection is closed.
        let (sender, receiver) = futures_channel::oneshot::channel();
        let store = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = store.open_connection().await;
            let _ = sender.send(result);
        });
        receiver
            .await
            .map_err(|_| error("IndexedDB connection task was cancelled"))?
    }

    async fn open_connection(&self) -> Result<Connection, StorageError> {
        let factory = web_sys::window()
            .ok_or_else(|| error("window is unavailable"))?
            .indexed_db()
            .map_err(js_error)?
            .ok_or_else(|| error("IndexedDB is unavailable"))?;
        let open = factory.open_with_u32(&self.database, 1).map_err(js_error)?;
        let request: IdbRequest = open.clone().unchecked_into();
        let upgrade_request = open.clone();
        let upgrade = Closure::<dyn FnMut(Event)>::new(move |_| {
            let result = upgrade_request
                .result()
                .and_then(|value| value.dyn_into::<IdbDatabase>())
                .and_then(|db| db.create_object_store(STORE).map(|_| ()));
            if result.is_err() {
                if let Some(transaction) = upgrade_request.transaction() {
                    let _ = transaction.abort();
                }
            }
        });
        open.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        let waiter = RequestWait::new(request);
        // Keep upgrade callbacks owned until the request finishes.
        // A blocked open remains pending until the other tab releases its DB.
        let guard = OpenHandlers {
            open: open.clone(),
            _upgrade: upgrade,
        };
        let result = waiter.wait().await;
        drop(guard);
        let database = result?.dyn_into::<IdbDatabase>().map_err(js_error)?;
        Ok(Connection(database))
    }
}

impl AsyncByteStorage for AsyncPlatformStorage {
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        validate_key(key)?;
        let db = self.open().await?;
        let tx =
            db.0.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readonly)
                .map_err(js_error)?;
        let completion = TransactionWait::new(tx.clone());
        let request = tx
            .object_store(STORE)
            .map_err(js_error)?
            .get(&key.into())
            .map_err(js_error)?;
        let value = RequestWait::new(request).wait().await;
        let committed = completion.wait().await;
        let value = value?;
        committed?;
        if value.is_undefined() {
            return Ok(None);
        }
        if !value.is_instance_of::<Uint8Array>() {
            return Err(StorageError::Corrupt(
                "IndexedDB value is not a byte array".into(),
            ));
        }
        Ok(Some(Uint8Array::new(&value).to_vec()))
    }
    async fn write(&self, key: &str, payload: &[u8]) -> Result<(), StorageError> {
        validate_key(key)?;
        let db = self.open().await?;
        let tx =
            db.0.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)
                .map_err(js_error)?;
        let completion = TransactionWait::new(tx.clone());
        let bytes = Uint8Array::from(payload);
        tx.object_store(STORE)
            .map_err(js_error)?
            .put_with_key(&bytes, &key.into())
            .map_err(js_error)?;
        completion.wait().await
    }
    async fn remove(&self, key: &str) -> Result<(), StorageError> {
        validate_key(key)?;
        let db = self.open().await?;
        let tx =
            db.0.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)
                .map_err(js_error)?;
        let completion = TransactionWait::new(tx.clone());
        tx.object_store(STORE)
            .map_err(js_error)?
            .delete(&key.into())
            .map_err(js_error)?;
        completion.wait().await
    }
}

fn error(message: &str) -> StorageError {
    StorageError::Browser(message.into())
}
fn js_error(value: JsValue) -> StorageError {
    error(&format!("{value:?}"))
}

struct Connection(IdbDatabase);
impl Drop for Connection {
    fn drop(&mut self) {
        self.0.close();
    }
}

struct OpenHandlers {
    open: web_sys::IdbOpenDbRequest,
    _upgrade: Closure<dyn FnMut(Event)>,
}
impl Drop for OpenHandlers {
    fn drop(&mut self) {
        self.open.set_onupgradeneeded(None);
    }
}

struct RequestWait {
    request: IdbRequest,
    promise: Promise,
    _success: Closure<dyn FnMut(Event)>,
    _error: Closure<dyn FnMut(Event)>,
}
impl RequestWait {
    fn new(request: IdbRequest) -> Self {
        let mut handlers = None;
        let promise = Promise::new(&mut |resolve, reject| {
            let success_request = request.clone();
            let failed_result = reject.clone();
            let success =
                Closure::<dyn FnMut(Event)>::new(move |_| match success_request.result() {
                    Ok(value) => {
                        let _ = resolve.call1(&JsValue::UNDEFINED, &value);
                    }
                    Err(value) => {
                        let _ = failed_result.call1(&JsValue::UNDEFINED, &value);
                    }
                });
            let failed_request = request.clone();
            let failure = Closure::<dyn FnMut(Event)>::new(move |_| {
                let cause = failed_request
                    .error()
                    .ok()
                    .flatten()
                    .map(JsValue::from)
                    .unwrap_or_else(|| JsValue::from_str("IndexedDB request failed"));
                let _ = reject.call1(&JsValue::UNDEFINED, &cause);
            });
            request.set_onsuccess(Some(success.as_ref().unchecked_ref()));
            request.set_onerror(Some(failure.as_ref().unchecked_ref()));
            handlers = Some((success, failure));
        });
        let (success, failure) = handlers.expect("Promise executor runs synchronously");
        Self {
            request,
            promise,
            _success: success,
            _error: failure,
        }
    }
    async fn wait(self) -> Result<JsValue, StorageError> {
        JsFuture::from(self.promise.clone()).await.map_err(js_error)
    }
}
impl Drop for RequestWait {
    fn drop(&mut self) {
        self.request.set_onsuccess(None);
        self.request.set_onerror(None);
    }
}

struct TransactionWait {
    finished: bool,
    tx: IdbTransaction,
    promise: Promise,
    _complete: Closure<dyn FnMut(Event)>,
    _abort: Closure<dyn FnMut(Event)>,
}
impl TransactionWait {
    fn new(tx: IdbTransaction) -> Self {
        let mut handlers = None;
        let promise = Promise::new(&mut |resolve, _reject| {
            let failed = resolve.clone();
            let complete = Closure::<dyn FnMut(Event)>::new(move |_| {
                let _ = resolve.call0(&JsValue::UNDEFINED);
            });
            let failed_tx = tx.clone();
            let abort = Closure::<dyn FnMut(Event)>::new(move |_| {
                let cause = failed_tx
                    .error()
                    .map(JsValue::from)
                    .unwrap_or_else(|| JsValue::from_str("IndexedDB transaction aborted"));
                // Resolve with an error payload; a dropped transaction observer
                // must not create an unhandled JS Promise rejection.
                let _ = failed.call1(&JsValue::UNDEFINED, &cause);
            });
            tx.set_oncomplete(Some(complete.as_ref().unchecked_ref()));
            tx.set_onabort(Some(abort.as_ref().unchecked_ref()));
            handlers = Some((complete, abort));
        });
        let (complete, abort) = handlers.expect("Promise executor runs synchronously");
        Self {
            tx,
            promise,
            finished: false,
            _complete: complete,
            _abort: abort,
        }
    }
    async fn wait(mut self) -> Result<(), StorageError> {
        let result = JsFuture::from(self.promise.clone())
            .await
            .map_err(js_error)?;
        self.finished = true;
        if result.is_undefined() {
            Ok(())
        } else {
            Err(js_error(result))
        }
    }
}
impl Drop for TransactionWait {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.tx.abort();
        }
        self.tx.set_oncomplete(None);
        self.tx.set_onabort(None);
    }
}
