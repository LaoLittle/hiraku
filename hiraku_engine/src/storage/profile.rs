//! Project-owned durable data in one document, outside SaveGameData.
use hiraku_script::{Value, hson::HsonValue, symbol::SymbolManifest};
#[cfg(test)]
use hiraku_storage::ByteStorage;
use hiraku_storage::{BufferedStorage as PlatformStorage, StorageError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const DOCUMENT_KEY: &str = "profile";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    version: u32,
    entries: BTreeMap<String, HsonValue>,
}

impl ProfileDocument {
    fn decode(bytes: Option<&[u8]>) -> Result<Self, StorageError> {
        let Some(bytes) = bytes else {
            return Ok(Self {
                version: 1,
                entries: BTreeMap::new(),
            });
        };
        let document: Self = hiraku_script::bhson::from_slice(bytes)
            .map_err(|error| StorageError::Corrupt(error.to_string()))?;
        if document.version != 1 {
            return Err(StorageError::Corrupt(
                "unsupported profile document version".into(),
            ));
        }
        Ok(document)
    }

    fn encode(&self) -> Result<Vec<u8>, StorageError> {
        hiraku_script::bhson::to_vec(self).map_err(|error| StorageError::Corrupt(error.to_string()))
    }

    fn insert(&mut self, id: &str, value: HsonValue) -> Result<bool, StorageError> {
        let key = key(id)?;
        if self.entries.get(&key) == Some(&value) {
            return Ok(false);
        }
        self.entries.insert(key, value);
        Ok(true)
    }
}

fn read_record(id: &str) -> Result<Option<HsonValue>, StorageError> {
    let key = key(id)?;
    let mut document = ProfileDocument::decode(backend().read(DOCUMENT_KEY)?.as_deref())?;
    Ok(document.entries.remove(&key))
}

fn write_record(id: &str, value: HsonValue) -> Result<(), StorageError> {
    key(id)?;
    let store = backend();
    let mut document = ProfileDocument::decode(store.read(DOCUMENT_KEY)?.as_deref())?;
    if document.insert(id, value)? {
        // Browser cache publishes this write immediately, so subsequent updates
        // include pending changes even before the IndexedDB transaction finishes.
        store.enqueue_generation(vec![hiraku_storage::GenerationRecord {
            key: DOCUMENT_KEY.into(),
            extension: "bhson".into(),
            payload: document.encode()?,
        }])?;
    }
    Ok(())
}

pub(super) fn backend() -> PlatformStorage {
    PlatformStorage::new(
        super::workspace_base_path().join("profile"),
        "hiraku.profile.bhson",
        "bhson",
    )
}

fn key(id: &str) -> Result<String, StorageError> {
    if id.trim().is_empty() {
        return Err(StorageError::InvalidKey);
    }
    Ok(id.to_owned())
}

pub fn read_object(id: &str, symbols: &SymbolManifest) -> Result<Option<Value>, String> {
    read_record(id)
        .map_err(|error| error.to_string())?
        .map(|value| hiraku_script::persistence::from_value(value, symbols))
        .transpose()
}

pub fn write_object(id: &str, value: &Value, symbols: &SymbolManifest) -> Result<(), String> {
    // Fully validate before touching the existing record.
    let value = hiraku_script::persistence::to_value(value, symbols)?;
    write_record(id, value).map_err(|error| error.to_string())
}

pub fn read_bool(id: &str) -> Result<bool, StorageError> {
    decode_bool(read_record(id)?)
}

pub fn write_bool(id: &str, value: bool) -> Result<(), StorageError> {
    write_record(id, scalar_record(&Value::Bool(value))?)
}

/// Missing project-owned counters start at zero, independently of save slots.
pub fn read_int(id: &str) -> Result<i64, StorageError> {
    decode_int(read_record(id)?)
}

pub fn write_int(id: &str, value: i64) -> Result<(), StorageError> {
    write_record(id, scalar_record(&Value::Int(value))?)
}

fn scalar_record(value: &Value) -> Result<HsonValue, StorageError> {
    hiraku_script::persistence::to_value(value, &SymbolManifest::default())
        .map_err(StorageError::Corrupt)
}

fn decode_int(payload: Option<HsonValue>) -> Result<i64, StorageError> {
    let Some(payload) = payload else {
        return Ok(0);
    };
    match hiraku_script::persistence::from_value(payload, &SymbolManifest::default())
        .map_err(StorageError::Corrupt)?
    {
        Value::Int(value) => Ok(value),
        _ => Err(StorageError::Corrupt("profile value expects Int".into())),
    }
}

#[cfg(test)]
fn read_from(store: &impl ByteStorage, id: &str) -> Result<bool, StorageError> {
    let mut document = ProfileDocument::decode(store.read(DOCUMENT_KEY)?.as_deref())?;
    decode_bool(document.entries.remove(&key(id)?))
}

fn decode_bool(payload: Option<HsonValue>) -> Result<bool, StorageError> {
    let Some(payload) = payload else {
        return Ok(false);
    };
    match hiraku_script::persistence::from_value(payload, &SymbolManifest::default())
        .map_err(StorageError::Corrupt)?
    {
        Value::Bool(value) => Ok(value),
        _ => Err(StorageError::Corrupt("profile value expects Bool".into())),
    }
}

#[cfg(test)]
fn write_to(store: &impl ByteStorage, id: &str, value: bool) -> Result<(), StorageError> {
    if read_from(store, id)? == value {
        return Ok(());
    }
    let mut document = ProfileDocument::decode(store.read(DOCUMENT_KEY)?.as_deref())?;
    document.insert(id, scalar_record(&Value::Bool(value))?)?;
    store.write(DOCUMENT_KEY, &document.encode()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiraku_script::native::{NativeError, NativeRegistry};
    use hiraku_script::{LinkedVm, LinkedVmEvent, ScriptSource, Value};

    /// Exercise the public generic wrapper and real serialization without
    /// touching a user's profile or requiring game assets.
    fn run_objects(source: &str) -> Result<(), String> {
        #[derive(Default)]
        struct Context {
            symbols: hiraku_script::symbol::SymbolManifest,
            bytes: Option<Vec<u8>>,
        }
        let mut registry = NativeRegistry::<Context>::new();
        registry
            .register_fn(
                "profile.write",
                |context: &mut Context, key: String, value: Value| {
                    let value = hiraku_script::persistence::to_value(&value, &context.symbols)
                        .map_err(NativeError::message)?;
                    let mut document = ProfileDocument::decode(context.bytes.as_deref())
                        .map_err(|error| NativeError::message(error.to_string()))?;
                    document
                        .insert(&key, value)
                        .map_err(|error| NativeError::message(error.to_string()))?;
                    context.bytes = Some(
                        document
                            .encode()
                            .map_err(|error| NativeError::message(error.to_string()))?,
                    );
                    Ok(())
                },
            )
            .expect("register write");
        registry
            .register_fn(
                "profile.read_any",
                |context: &mut Context, key: String, fallback: Value| {
                    let mut document = ProfileDocument::decode(context.bytes.as_deref())
                        .map_err(|error| NativeError::message(error.to_string()))?;
                    match document.entries.remove(&key) {
                        Some(value) => {
                            hiraku_script::persistence::from_value(value, &context.symbols)
                                .map_err(NativeError::message)
                        }
                        None => Ok(fallback),
                    }
                },
            )
            .expect("register read");
        registry
            .register_fn("verify", |_: &mut Context, value: bool| {
                if value {
                    Ok(())
                } else {
                    Err(NativeError::message("verification failed"))
                }
            })
            .expect("register verify");
        let project = hiraku_script::compile_project(
            vec![
                ScriptSource {
                    path: "profile.hks".into(),
                    namespace: Some("profile".into()),
                    source: include_str!("../script/std/profile.hks").into(),
                },
                ScriptSource {
                    path: "main.hks".into(),
                    namespace: None,
                    source: source.into(),
                },
            ],
            &registry.manifest(),
        )
        .map_err(|error| format!("{error:?}"))?;
        let mut vm = LinkedVm::new(project.program, project.paths["main.hks"])
            .map_err(|error| error.to_string())?;
        let mut context = Context::default();
        let mut budget = 100_000;
        loop {
            match vm
                .step_with_budget(&mut budget)
                .map_err(|error| error.to_string())?
            {
                Some(LinkedVmEvent::Call(call)) => {
                    context.symbols = vm.symbols().clone();
                    let value = registry
                        .call(&mut context, &call)
                        .map_err(|error| error.to_string())?;
                    vm.resume(value).map_err(|error| error.to_string())?;
                }
                Some(LinkedVmEvent::BudgetExhausted) => {
                    return Err("test instruction budget exhausted".into());
                }
                Some(LinkedVmEvent::Completed(_)) | None => return Ok(()),
                Some(LinkedVmEvent::Statement(_)) => {}
            }
        }
    }

    #[test]
    fn object_profile_is_typed_and_independent_of_live_mutations() {
        run_objects(
            r#"
            struct Alice { name: String, scores: List<Int>, nickname: String? }
            enum Mood { happy(Int), sad }
            let alice = Alice.{ name: "alice", scores: [1, 2], nickname: null }
            profile.write("alice", alice)
            alice.name = "bob"
            let restored = profile.read("alice", alice)
            verify(restored.name == "alice")
            verify(restored.scores == [1, 2])
            verify(restored.nickname == null)
            verify(profile.read("missing", 42) == 42)
            let mood: Mood = .happy(7)
            profile.write("mood", mood)
            let loaded = profile.read("mood", mood)
            verify(when loaded { .happy(n) -> n == 7, .sad -> false })
        "#,
        )
        .expect("typed object round trip");
    }

    #[test]
    fn object_profile_rejects_wrong_schema_and_callbacks() {
        let error = run_objects(
            r#"
            profile.write("alice", "alice")
            let value: Int = profile.read("alice", 0)
        "#,
        )
        .expect_err("a string cannot be read as an integer");
        assert!(error.contains("cast"), "{error}");
        let error = run_objects(
            r#"
            let callback: () -> Int = { 7 }
            profile.write("alice", .{ callback: callback })
        "#,
        )
        .expect_err("a closure cannot be persisted");
        assert!(error.contains("$.callback"), "{error}");
    }
    #[derive(Clone, Default)]
    struct Memory(std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>);
    impl ByteStorage for Memory {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
            Ok(self.0.lock().expect("test store").get(key).cloned())
        }
        fn write(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
            self.0
                .lock()
                .expect("test store")
                .insert(key.into(), bytes.to_vec());
            Ok(())
        }
        fn remove(&self, key: &str) -> Result<(), StorageError> {
            self.0.lock().expect("test store").remove(key);
            Ok(())
        }
    }
    #[test]
    fn integer_profile_values_are_typed_and_preserve_full_range() {
        let store = Memory::default();
        let mut document = ProfileDocument::decode(None).expect("empty profile");
        assert_eq!(decode_int(None).expect("zero"), 0);
        for value in [i64::MIN, -1, 0, 1, i64::MAX] {
            document
                .insert(
                    "alice/progress",
                    scalar_record(&Value::Int(value)).expect("integer record"),
                )
                .expect("insert integer");
            store
                .write(DOCUMENT_KEY, &document.encode().expect("BHSON document"))
                .expect("write integer");
            let reopened = store.clone();
            let mut decoded =
                ProfileDocument::decode(reopened.read(DOCUMENT_KEY).expect("reopen").as_deref())
                    .expect("decode document");
            let payload = decoded.entries.remove("alice/progress");
            assert_eq!(decode_int(payload.clone()).expect("integer"), value);
            assert!(decode_bool(payload).is_err(), "an integer is not a boolean");
        }
        for value in [
            Value::Bool(true),
            Value::String("42".into()),
            Value::Number(42.0),
            Value::UInt(42),
            Value::Unit,
        ] {
            assert!(decode_int(Some(scalar_record(&value).expect("record"))).is_err());
        }
    }
    #[test]
    fn flags_survive_reopening_and_are_not_save_slots() {
        let profile = Memory::default();
        let saves = Memory::default();
        assert!(!read_from(&profile, "alice/cg/1").expect("missing flag"));
        write_to(&profile, "alice/cg/1", true).expect("unlock");
        write_to(&profile, "bob/cg/2", true).expect("second unlock");
        assert_eq!(
            profile.0.lock().expect("test store").len(),
            1,
            "all keys use a single record"
        );
        saves.write("bob", b"old story state").expect("save");
        saves.remove("bob").expect("remove save");
        let reopened = profile.clone();
        assert!(read_from(&reopened, "alice/cg/1").expect("persistent flag"));
        assert!(read_from(&reopened, "bob/cg/2").expect("second persistent flag"));
        let mut document = ProfileDocument::decode(
            reopened
                .read(DOCUMENT_KEY)
                .expect("read document")
                .as_deref(),
        )
        .expect("decode document");
        document
            .insert("corrupt", HsonValue::String("invalid record".into()))
            .expect("corrupt fixture entry");
        reopened
            .write(DOCUMENT_KEY, &document.encode().expect("fixture document"))
            .expect("corrupt fixture");
        assert!(read_from(&reopened, "corrupt").is_err());
        assert!(read_from(&reopened, "alice/cg/1").expect("other entries remain intact"));
    }

    #[test]
    fn mixed_profile_records_roundtrip_as_one_document() {
        let mut document = ProfileDocument::decode(None).expect("empty profile");
        document
            .insert(
                "alice/flag",
                scalar_record(&Value::Bool(true)).expect("flag record"),
            )
            .expect("flag");
        document
            .insert(
                "alice/count",
                scalar_record(&Value::Int(42)).expect("counter record"),
            )
            .expect("counter");
        let symbols = hiraku_script::symbol::SymbolManifest::default();
        let object = Value::Map([("name".into(), Value::String("bob".into()))].into());
        let payload =
            hiraku_script::persistence::to_value(&object, &symbols).expect("object payload");
        document
            .insert("bob/data", payload.clone())
            .expect("object");
        assert!(
            !document
                .insert("bob/data", payload)
                .expect("identical update")
        );
        assert!(document.insert(" ", HsonValue::Null).is_err());
        let bytes = document.encode().expect("one file");
        assert!(bytes.starts_with(hiraku_script::bhson::MAGIC));
        let reopened = ProfileDocument::decode(Some(&bytes)).expect("reopen");
        assert_eq!(reopened.entries.len(), 3);
        assert_eq!(
            decode_int(Some(reopened.entries["alice/count"].clone())).expect("counter"),
            42
        );
        assert_eq!(
            hiraku_script::persistence::from_value(reopened.entries["bob/data"].clone(), &symbols)
                .expect("object"),
            object
        );
        assert!(ProfileDocument::decode(Some(b"bad profile")).is_err());
        assert!(ProfileDocument::decode(Some(b".{ version: 1, entries: .{} }")).is_err());
        assert!(ProfileDocument::decode(Some(b"true")).is_err());
        assert!(ProfileDocument::decode(Some(b"int:42")).is_err());
    }
}
