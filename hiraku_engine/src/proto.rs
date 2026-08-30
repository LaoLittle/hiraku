use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct SaveGameData {
    #[prost(uint32, tag = "1")]
    pub version: u32,
    #[prost(string, tag = "2")]
    pub resume_script: String,
    #[prost(uint64, tag = "3")]
    pub random_seed: u64,
    #[prost(int64, tag = "4")]
    pub time_seed: i64,
    #[prost(message, optional, tag = "5")]
    pub checkpoint: Option<SaveCheckpoint>,
    #[prost(string, repeated, tag = "6")]
    pub script_stack: Vec<String>,
    #[prost(message, repeated, tag = "7")]
    pub globals: Vec<StoredEntry>,
    #[prost(message, repeated, tag = "8")]
    pub scope: Vec<StoredEntry>,
    #[prost(message, repeated, tag = "9")]
    pub input_log: Vec<SavedInput>,
    #[prost(message, optional, tag = "10")]
    pub scene: Option<SceneSnapshot>,
    #[prost(message, optional, tag = "11")]
    pub rng_state: Option<RngState>,
    #[prost(bytes = "vec", tag = "12")]
    pub vm_snapshot_hson: Vec<u8>,
    #[prost(string, optional, tag = "14")]
    pub pending_ui_screen: Option<String>,
    #[prost(bytes = "vec", tag = "15")]
    pub script_call_stack_hson: Vec<u8>,
    #[prost(bytes = "vec", tag = "16")]
    pub ui_registry_hson: Vec<u8>,
    #[prost(bytes = "vec", tag = "17")]
    pub mounted_ui_overlays_hson: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RngState {
    #[prost(uint64, tag = "1")]
    pub state: u64,
    #[prost(uint64, tag = "2")]
    pub stream: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct SaveCheckpoint {
    #[prost(string, tag = "1")]
    pub script: String,
    #[prost(uint64, tag = "2")]
    pub ordinal: u64,
    #[prost(string, tag = "3")]
    pub kind: String,
    #[prost(string, optional, tag = "4")]
    pub label: Option<String>,
    #[prost(message, optional, tag = "5")]
    pub position: Option<ScriptPosition>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScriptPosition {
    #[prost(uint64, optional, tag = "1")]
    pub line: Option<u64>,
    #[prost(uint64, optional, tag = "2")]
    pub column: Option<u64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SavedInput {
    #[prost(message, optional, tag = "1")]
    pub checkpoint: Option<SaveCheckpoint>,
    #[prost(message, optional, tag = "2")]
    pub value: Option<StoredValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct StoredEntry {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(message, optional, tag = "2")]
    pub value: Option<StoredValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct StoredArray {
    #[prost(message, repeated, tag = "1")]
    pub values: Vec<StoredValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct StoredMap {
    #[prost(message, repeated, tag = "1")]
    pub entries: Vec<StoredEntry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct StoredValue {
    #[prost(oneof = "stored_value::Kind", tags = "1, 2, 3, 4, 5, 6")]
    pub kind: Option<stored_value::Kind>,
}

pub mod stored_value {
    use prost::Oneof;

    use super::{StoredArray, StoredMap};

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Kind {
        #[prost(bool, tag = "1")]
        Bool(bool),
        #[prost(int64, tag = "2")]
        Int(i64),
        #[prost(double, tag = "3")]
        Float(f64),
        #[prost(string, tag = "4")]
        String(String),
        #[prost(message, tag = "5")]
        Array(StoredArray),
        #[prost(message, tag = "6")]
        Map(StoredMap),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct SceneSnapshot {
    #[prost(message, optional, tag = "1")]
    pub background: Option<ImageLayerSnapshot>,
    #[prost(message, repeated, tag = "2")]
    pub sprites: Vec<SpriteSnapshot>,
    #[prost(message, repeated, tag = "3")]
    pub character_positions: Vec<CharacterPosition>,
    #[prost(float, tag = "4")]
    pub overlay_alpha: f32,
    #[prost(message, optional, tag = "5")]
    pub bgm: Option<AudioSnapshot>,
    #[prost(message, optional, tag = "6")]
    pub dialogue: Option<DialogueSnapshot>,
    #[prost(message, optional, tag = "7")]
    pub text_effect: Option<TextEffectSnapshot>,
    #[prost(message, optional, tag = "8")]
    pub camera: Option<CameraSnapshot>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CameraSnapshot {
    #[prost(float, tag = "1")]
    pub blur: f32,
    #[prost(float, tag = "2")]
    pub zoom: f32,
    #[prost(float, repeated, tag = "3")]
    pub offset: Vec<f32>,
    #[prost(float, repeated, tag = "4")]
    pub rotation: Vec<f32>,
    #[prost(string, tag = "5")]
    pub projection: String,
    #[prost(string, tag = "6")]
    pub scope: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ImageLayerSnapshot {
    #[prost(string, tag = "1")]
    pub path: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct SpriteSnapshot {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub path: String,
    #[prost(float, tag = "3")]
    pub x: f32,
    #[prost(float, tag = "4")]
    pub y: f32,
    #[prost(float, tag = "5")]
    pub layer: f32,
    #[prost(float, tag = "6")]
    pub scale: f32,
    #[prost(float, tag = "7")]
    pub alpha: f32,
    #[prost(float, repeated, tag = "8")]
    pub rect: Vec<f32>,
    #[prost(bool, tag = "9")]
    pub focused: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct CharacterPosition {
    #[prost(string, tag = "1")]
    pub actor_id: String,
    #[prost(float, tag = "2")]
    pub x: f32,
    #[prost(float, tag = "3")]
    pub y: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct AudioSnapshot {
    #[prost(string, tag = "1")]
    pub path: String,
    #[prost(float, tag = "2")]
    pub volume: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct DialogueSnapshot {
    #[prost(string, tag = "1")]
    pub speaker: String,
    #[prost(string, tag = "2")]
    pub text: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextEffectSnapshot {
    #[prost(string, tag = "1")]
    pub mode: String,
    #[prost(float, tag = "2")]
    pub cps: f32,
    #[prost(float, tag = "3")]
    pub fade_seconds: f32,
}
