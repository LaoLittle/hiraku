mod assets;
mod audio;
mod character;
mod data;
pub mod dependencies;
mod effect;
pub use effect::composition::{CompositionBlend, CompositionCrossfadeMaterial};
pub use effect::post_process::{EffectParameters, PostProcessSettings};
pub use effect::program::{EffectInstance, EffectShader, EffectUniform, LayerEffectShaders};
mod glossary;
pub mod input;
pub mod memory;
mod movie;
mod proto;
mod redraw;
pub mod render;
mod scene;
pub use scene::DialogueHistoryState;
pub use scene::clock::SceneClock;
mod script;
pub mod stage;
mod state;
mod storage;
pub use storage::{PreferenceChange, UserSettings};
mod rich_text;
mod texture;
mod ui;
mod vfs;

pub use audio::{EngineAudioLoader, EngineAudioSource};
pub use script::{UiContext, UiIntent};
pub use state::StoredValue;
pub use ui::UiModels;

/// Type-check a standalone story source against the engine capability schema.
/// This does not launch Bevy, access assets, or execute script/native functions.
pub fn validate_story_source(path: &str, source: &str) -> Result<(), String> {
    script::compile_story_bytecode(path, source).map(|_| ())
}

/// Compile and link the project's story modules without starting the game.
pub fn validate_story_project(
    root: &std::path::Path,
    settings: &str,
    entry: &str,
) -> Result<(), String> {
    let vfs = vfs::HdpVfs::new_with_config(root, settings, entry);
    let source = vfs.read_text(entry).map_err(|error| error.to_string())?;
    script::compile_story_program(&vfs, entry, &source).map(|_| ())
}

/// Parse project audio descriptors using the runtime catalog loader, without
/// starting playback. Audio file decoding is a separate validation step.
pub fn validate_audio_catalog(root: &std::path::Path, settings: &str) -> Result<(), String> {
    let vfs = vfs::HdpVfs::new_with_config(root, settings, "startup.hks");
    audio::load_audio_catalog(&vfs)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Resolve character parts and expressions against the texture catalog without
/// spawning entities or uploading textures.
pub fn validate_character_catalog(root: &std::path::Path, settings: &str) -> Result<(), String> {
    let vfs = vfs::HdpVfs::new_with_config(root, settings, "startup.hks");
    character::load_character_catalog(&vfs)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Type-check a UI source with the standard widget module, without rendering.
pub fn validate_ui_source(path: &str, source: &str) -> Result<(), String> {
    script::validate_ui_source(path, source)
}

/// Build a declarative UI document against loose project descriptors, without
/// rendering or dispatching its effects. Also checks textures and callbacks.
/// Standard UI models are supplied as empty data; project-specific globals
/// must be checked through the context-taking variant.
pub fn validate_ui_document(
    root: &std::path::Path,
    settings: &str,
    path: &str,
) -> Result<(), String> {
    use state::StoredValue as V;
    use std::collections::BTreeMap;
    let context = UiContext::new(BTreeMap::from([
        (
            "dialogue".into(),
            V::Map(BTreeMap::from([
                ("speaker".into(), V::String(String::new())),
                ("text".into(), V::String(String::new())),
                ("visible".into(), V::Bool(false)),
                ("revealedCharacters".into(), V::Int(0)),
                ("canAdvance".into(), V::Bool(false)),
                ("autoEnabled".into(), V::Bool(false)),
                ("fastForwardEnabled".into(), V::Bool(false)),
            ])),
        ),
        (
            "history".into(),
            V::Map(BTreeMap::from([
                ("text".into(), V::String(String::new())),
                ("entries".into(), V::Array(Vec::new())),
            ])),
        ),
        (
            "choice".into(),
            V::Map(BTreeMap::from([
                ("prompt".into(), V::String(String::new())),
                ("options".into(), V::Array(vec![V::String("Option".into())])),
                ("enabled".into(), V::Array(vec![V::Bool(true)])),
            ])),
        ),
    ]));
    validate_ui_document_with_context(root, settings, path, context)
}

/// Offline UI validation with explicitly supplied model data.
pub fn validate_ui_document_with_context(
    root: &std::path::Path,
    settings: &str,
    path: &str,
    context: UiContext,
) -> Result<(), String> {
    validate_ui_document_with_arguments(root, settings, path, context, &[])
}

/// Offline construction of a typed UI entry, without mounting a scene/window.
pub fn validate_ui_document_with_arguments(
    root: &std::path::Path,
    settings: &str,
    path: &str,
    context: UiContext,
    arguments: &[state::StoredValue],
) -> Result<(), String> {
    let vfs = vfs::HdpVfs::new_with_config(root, settings, "startup.hks");
    let source = vfs.read_text(path).map_err(|error| error.to_string())?;
    let textures = texture::load_texture_catalog(&vfs).map_err(|error| error.to_string())?;
    let terms = glossary::load_term_catalog(&vfs).map_err(|error| error.to_string())?;
    script::evaluate_ui_component_named_with_args(
        path, &source, context, &textures, &terms, arguments,
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

use std::sync::Arc;

use assets::{
    BytesAsset, BytesAssetLoader, HdpArchive, HdpArchiveLoader, HdpVolumeLoads,
    stream_requested_hdp_volumes,
};
use bevy::{
    app::PluginGroupBuilder,
    asset::{AssetApp, io::AssetSourceId},
    audio::AddAudioSource,
    camera::ClearColorConfig,
    pbr::MaterialPlugin,
    prelude::*,
};
use effect::transition::RuleTransitionMaterial;
use effect::{custom::CustomScreenEffectMaterial, post_process::PostProcessPlugin};
use render::camera::{animate_camera_shake, animate_camera_transition, assign_render_layers};
use render::character_part::{AlphaMaskMaterial, MultiplyMaterial};
use scene::{
    advance_dialogue_on_input, animate_audio_fades, animate_character_motion_effects,
    animate_custom_effects, animate_dialogue_text_reveal, animate_rule_transitions,
    animate_screen_ui, animate_visual_tweens, apply_animation_cancellations,
    apply_live_audio_settings, cleanup_stale_screen_ui, complete_movie_waits, drive_story_runtime,
    handle_choice_action_input, handle_choice_buttons, handle_runtime_menu_buttons,
    handle_screen_buttons, handle_screen_image_buttons, handle_screen_scroll,
    handle_screen_toggles, poll_pending_character_shows, poll_voice_playback, prepare_bgm_preludes,
    process_script_commands, process_ui_effects, reconcile_restored_bgm,
    reconcile_restored_characters, setup_frontend, setup_stage, sync_scene_snapshot,
    tick_animation_waits, tick_pending_waits, update_builtin_ui_models,
    update_runtime_menu_button_visuals, update_ui_reactive_bindings, update_ui_text_bindings,
};
use script::{ScriptResponseMessage, ScriptRuntimeState, StoryRuntime};
use state::SceneSharedState;
use vfs::{HDP_SOURCE_ID, HdpArchiveStore, VfsResource, hdp_asset_source_builder};

#[derive(Clone, Debug, Resource)]
pub struct RuntimeLaunchConfig {
    /// Where runtime content is loaded from. Packaged games normally use HDP,
    /// while examples and development tools can read an ordinary directory.
    pub asset_mode: RuntimeAssetMode,
    pub asset_root: String,
    pub settings_path: String,
    pub default_startup_script: String,
    pub window_title: String,
    /// Stable project ID for browser storage, independent of window title.
    pub storage_namespace: String,
    /// Fallback logical resolution. Project `settings.hson`'s `canvasSize`
    /// takes precedence before the canvas and its cameras are created.
    pub canvas_size: UVec2,
    pub camera_order: isize,
    pub camera_clear_color: ClearColorConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeAssetMode {
    #[default]
    Hdp,
    Directory,
}

/// Fixed-resolution image produced by Hiraku's scene and UI camera.
///
/// Host games present this image in their own window camera, which keeps the engine independent
/// from window sizing and platform-specific letterboxing.
#[derive(Clone, Debug, Resource)]
pub struct HirakuCanvas {
    pub image: Handle<Image>,
    pub size: UVec2,
}

#[derive(Clone, Debug, Resource)]
pub(crate) struct HirakuInputTarget(pub Handle<Image>);

impl Default for RuntimeLaunchConfig {
    fn default() -> Self {
        Self {
            asset_mode: RuntimeAssetMode::Hdp,
            asset_root: vfs::DEFAULT_ASSET_ROOT.to_string(),
            settings_path: vfs::DEFAULT_SETTINGS_PATH.to_string(),
            default_startup_script: vfs::DEFAULT_STARTUP_SCRIPT.to_string(),
            window_title: "hiraku".to_string(),
            storage_namespace: "hiraku".to_string(),
            canvas_size: UVec2::new(1920, 1080),
            camera_order: -1,
            camera_clear_color: ClearColorConfig::Default,
        }
    }
}

impl RuntimeLaunchConfig {
    /// Creates a development configuration that reads loose files directly
    /// from `asset_root` instead of waiting for or loading an HDP archive.
    pub fn directory(asset_root: impl Into<String>) -> Self {
        Self {
            asset_mode: RuntimeAssetMode::Directory,
            asset_root: asset_root.into(),
            settings_path: "settings.hson".to_string(),
            default_startup_script: "startup.hks".to_string(),
            ..Self::default()
        }
    }
}

pub struct HirakuPluginGroup;

impl PluginGroup for HirakuPluginGroup {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>().add(HirakuPlugin)
    }
}

pub struct HirakuAssetSourcePlugin;

impl Plugin for HirakuAssetSourcePlugin {
    fn build(&self, app: &mut App) {
        let config = app.world().resource::<RuntimeLaunchConfig>().clone();
        let archive_store = app.world().resource::<HdpArchiveStore>().clone();
        app.register_asset_source(
            AssetSourceId::Name(HDP_SOURCE_ID.into()),
            hdp_asset_source_builder(config.asset_root, archive_store),
        );
    }
}

pub struct HirakuPlugin;

#[derive(Resource)]
#[expect(
    dead_code,
    reason = "keeps the archive loaded for the HDP asset source"
)]
struct HdpArchiveHandle(Handle<HdpArchive>);

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct HirakuRuntimeSystems;

impl Plugin for HirakuPlugin {
    fn build(&self, app: &mut App) {
        scene::screen_ui::configure_screen_ui_phases(app);
        app.add_plugins(stage::StagePlugin);
        render::ui_quad::register(app);
        app.add_plugins(texture::ArtworkPlugin);
        effect::custom::load_internal_shaders(app);
        effect::transition::load_internal_shaders(app);
        effect::composition::load_internal_shaders(app);
        render::character_part::load_internal_shaders(app);
        app.add_plugins((
            hiraku_uastc::UastcPlugin,
            hiraku_video::HirakuVideoPlugin,
            MaterialPlugin::<CustomScreenEffectMaterial>::default(),
            MaterialPlugin::<RuleTransitionMaterial>::default(),
            MaterialPlugin::<CompositionCrossfadeMaterial>::default(),
            MaterialPlugin::<AlphaMaskMaterial>::default(),
            MaterialPlugin::<MultiplyMaterial>::default(),
            PostProcessPlugin,
        ));
        render::world_sprite::install(app);
        scene::character_composite::install(app);

        let archive_path = archive_path_from_config(app.world().resource::<RuntimeLaunchConfig>());
        let archive_store = app.world().resource::<HdpArchiveStore>().clone();
        let archive_root =
            std::path::PathBuf::from(&app.world().resource::<RuntimeLaunchConfig>().asset_root);

        app.init_asset::<HdpArchive>()
            .init_asset::<BytesAsset>()
            .init_asset::<TextureAtlasLayout>()
            .add_audio_source::<audio::PreludeLoopAudio>()
            .add_audio_source::<audio::EngineAudioSource>()
            .init_asset_loader::<audio::EngineAudioLoader>()
            .init_resource::<HdpVolumeLoads>()
            .add_message::<input::HirakuPointerInput>()
            .add_message::<input::HirakuScrollInput>()
            .add_message::<input::HirakuActionInput>()
            .add_systems(
                First,
                input::bridge_virtual_pointers.before(bevy::picking::PickingSystems::Input),
            )
            .add_systems(Last, input::cleanup_touch_pointers)
            .add_systems(Last, input::request_input_redraw)
            .add_systems(PreStartup, storage::initialize_runtime_storage)
            .add_systems(First, storage::poll_runtime_storage)
            .init_resource::<ScriptRuntimeState>()
            .init_resource::<dependencies::ScriptDependencies>()
            .add_systems(PostUpdate, dependencies::update_resource_window)
            .init_resource::<memory::MemoryDiagnostics>()
            .add_systems(PostUpdate, memory::sample)
            .init_resource::<dependencies::LoadingScreen>()
            .add_systems(PostUpdate, dependencies::loading_screen)
            .add_systems(PostUpdate, scene::loading::sync)
            .init_resource::<UiModels>()
            .init_resource::<scene::PendingMovieWaits>()
            .init_resource::<scene::playback::FastForward>()
            .add_systems(
                PreUpdate,
                scene::playback::update_fast_forward.in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                scene::playback::skip_voices
                    .after(scene::process_script_commands)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_message::<ScriptResponseMessage>()
            .add_message::<scene::UiEffectMessage>()
            .register_asset_loader(HdpArchiveLoader::new(archive_store, archive_root))
            .init_asset_loader::<BytesAssetLoader>()
            .add_systems(Update, stream_requested_hdp_volumes)
            .init_resource::<ProjectCanvasStatus>()
            .add_systems(Update, prepare_project_canvas.run_if(runtime_content_ready))
            .add_systems(
                Update,
                (setup_frontend, setup_stage)
                    .chain()
                    .after(prepare_project_canvas)
                    .run_if(project_canvas_ready)
                    .run_if(runtime_content_ready)
                    .run_if(storage::storage_ready)
                    .run_if(runtime_not_initialized),
            )
            .add_systems(Update, assign_render_layers.after(process_script_commands))
            .configure_sets(PreUpdate, HirakuRuntimeSystems.run_if(runtime_initialized))
            .configure_sets(Update, HirakuRuntimeSystems.run_if(runtime_initialized))
            .configure_sets(PostUpdate, HirakuRuntimeSystems.run_if(runtime_initialized))
            .add_systems(
                Update,
                boot_runtime
                    .run_if(runtime_initialized)
                    .run_if(storage::storage_ready),
            )
            .add_systems(
                Update,
                reconcile_restored_characters
                    .before(drive_story_runtime)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                reconcile_restored_bgm
                    .before(drive_story_runtime)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_message::<scene::widgets::UiCallbackRequest>()
            .init_resource::<Time<scene::clock::SceneClock>>()
            .add_systems(
                PostUpdate,
                (scene::ui_visuals::tick, scene::ui_visuals::apply)
                    .chain()
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                PreUpdate,
                scene::clock::advance.in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                scene::clock::pause_voices.in_set(HirakuRuntimeSystems),
            )
            .add_message::<input::HirakuTextInput>()
            .init_resource::<input::HirakuTextFocus>()
            .init_resource::<scene::save_preview::SavePreview>()
            .add_systems(
                Update,
                scene::widgets::input_events
                    .after(cleanup_stale_screen_ui)
                    .before(handle_runtime_menu_buttons)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                scene::ui_timers::tick
                    .after(cleanup_stale_screen_ui)
                    .after(scene::recompose_screen_ui)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                (
                    scene::widgets::sync_inputs,
                    scene::widgets::sync_toggles,
                    scene::recompose_screen_ui,
                )
                    .chain()
                    .after(handle_runtime_menu_buttons)
                    .after(update_builtin_ui_models)
                    .in_set(scene::screen_ui::ScreenUiPhase::Structure)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(Update, drive_story_runtime.in_set(HirakuRuntimeSystems))
            .add_systems(
                Update,
                scene::sync_movie_clock
                    .after(scene::clock::advance)
                    .after(process_script_commands)
                    .before(hiraku_video::VideoPlaybackSystems)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                complete_movie_waits
                    .before(drive_story_runtime)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                process_script_commands
                    .after(drive_story_runtime)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                update_builtin_ui_models
                    .after(process_script_commands)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                (
                    update_ui_text_bindings,
                    update_ui_reactive_bindings,
                    scene::rich_text::update,
                    animate_screen_ui,
                    scene::screen_ui::sync_allowed_overlays,
                    scene::ui_keyframes::tick,
                    scene::ui_hover::animate_hover,
                )
                    .chain()
                    .after(process_script_commands)
                    .in_set(scene::screen_ui::ScreenUiPhase::Content)
                    .after(scene::recompose_screen_ui)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                animate_camera_transition
                    .after(process_script_commands)
                    .before(animate_camera_shake)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                apply_live_audio_settings
                    .after(process_script_commands)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                prepare_bgm_preludes
                    .after(process_script_commands)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                cleanup_stale_screen_ui
                    .after(process_script_commands)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                handle_screen_buttons
                    .after(cleanup_stale_screen_ui)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                handle_screen_image_buttons
                    .after(cleanup_stale_screen_ui)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                process_ui_effects
                    .after(handle_screen_buttons)
                    .after(handle_screen_image_buttons)
                    .after(handle_runtime_menu_buttons)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                (handle_screen_scroll, handle_screen_toggles)
                    .after(cleanup_stale_screen_ui)
                    .before(handle_runtime_menu_buttons)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                PostUpdate,
                scene::screen_ui::initialize_scroll_anchors.after(bevy::ui::UiSystems::Layout),
            )
            .add_systems(
                PostUpdate,
                scene::expire_overlays.in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                PostUpdate,
                (
                    scene::fit_screen_text,
                    scene::rich_text::reveal_glyphs,
                    scene::rich_text::position_ruby,
                    scene::text_visibility::sync,
                )
                    .chain()
                    .after(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
                    .after(bevy::ui::widget::text_system)
                    .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
            )
            .add_systems(
                Update,
                handle_choice_buttons
                    .after(cleanup_stale_screen_ui)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                update_runtime_menu_button_visuals
                    .after(cleanup_stale_screen_ui)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                handle_runtime_menu_buttons
                    .after(cleanup_stale_screen_ui)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                handle_choice_action_input.in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                animate_dialogue_text_reveal
                    .after(process_script_commands)
                    .before(update_builtin_ui_models)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                advance_dialogue_on_input
                    .after(cleanup_stale_screen_ui)
                    .after(handle_runtime_menu_buttons)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(Update, tick_pending_waits.in_set(HirakuRuntimeSystems))
            .add_systems(
                Update,
                (
                    apply_animation_cancellations.in_set(HirakuRuntimeSystems),
                    animate_visual_tweens
                        .after(poll_pending_character_shows)
                        .in_set(HirakuRuntimeSystems),
                    scene::curtain::update_curtains
                        .after(process_script_commands)
                        .before(animate_visual_tweens)
                        .in_set(HirakuRuntimeSystems),
                    scene::curtain::complete_curtain_waits
                        .after(animate_visual_tweens)
                        .after(scene::curtain::update_curtains)
                        .in_set(HirakuRuntimeSystems),
                    animate_audio_fades.in_set(HirakuRuntimeSystems),
                    scene::poll_sfx_playback.in_set(HirakuRuntimeSystems),
                    animate_custom_effects.in_set(HirakuRuntimeSystems),
                    scene::pictures::sync_pictures
                        .before(hiraku_video::VideoPlaybackSystems)
                        .after(animate_camera_transition)
                        .after(animate_camera_shake)
                        .after(scene::process_script_commands)
                        .in_set(HirakuRuntimeSystems),
                    animate_rule_transitions.in_set(HirakuRuntimeSystems),
                    animate_camera_shake.in_set(HirakuRuntimeSystems),
                    animate_character_motion_effects
                        .after(poll_pending_character_shows)
                        .in_set(HirakuRuntimeSystems),
                    scene::actor_motion::animate
                        .after(animate_character_motion_effects)
                        .in_set(HirakuRuntimeSystems),
                    scene::effect_wait::complete
                        .after(process_script_commands)
                        .after(poll_pending_character_shows)
                        .after(scene::pictures::sync_pictures)
                        .in_set(HirakuRuntimeSystems),
                    poll_voice_playback.in_set(HirakuRuntimeSystems),
                    poll_pending_character_shows
                        .after(reconcile_restored_characters)
                        .after(process_script_commands)
                        .in_set(HirakuRuntimeSystems),
                    tick_animation_waits.in_set(HirakuRuntimeSystems),
                    sync_scene_snapshot
                        .after(scene::actor_motion::animate)
                        .after(poll_pending_character_shows)
                        .after(animate_character_motion_effects)
                        .after(animate_visual_tweens)
                        .after(reconcile_restored_bgm)
                        .in_set(HirakuRuntimeSystems),
                ),
            );

        if app.world().resource::<RuntimeLaunchConfig>().asset_mode == RuntimeAssetMode::Hdp {
            let archive = app.world().resource::<AssetServer>().load(archive_path);
            app.insert_resource(HdpArchiveHandle(archive));
        }
    }
}

/// Prepare an app for Hiraku before adding Bevy's `DefaultPlugins`.
///
/// This registers Hiraku's custom asset sources, which Bevy requires before
/// `AssetPlugin` is built. After adding `DefaultPlugins`, add
/// [`HirakuPluginGroup`] to install Hiraku's materials and runtime systems.
pub fn configure_runtime_app(app: &mut App, config: RuntimeLaunchConfig) {
    let asset_root_path = std::path::PathBuf::from(&config.asset_root);
    let asset_mode = config.asset_mode;
    let archive_store = vfs::HdpArchiveStore::default();
    let vfs = Arc::new(vfs::HdpVfs::new_with_config_and_store(
        asset_root_path,
        config.settings_path.clone(),
        config.default_startup_script.clone(),
        archive_store.clone(),
    ));

    app.insert_resource(config);
    app.insert_resource(archive_store);
    app.insert_resource(VfsResource(vfs));
    app.insert_resource(SceneSharedState::default());
    app.insert_resource(ClearColor(Color::BLACK));
    if asset_mode == RuntimeAssetMode::Hdp {
        app.add_plugins(HirakuAssetSourcePlugin);
    }
}

fn archive_path_from_config(config: &RuntimeLaunchConfig) -> String {
    vfs::split_hdp_asset_path(&config.settings_path)
        .map(|(archive, _)| archive)
        .unwrap_or_else(|| "main.hdp".to_string())
}

fn hdp_archive_ready(archive_store: Res<HdpArchiveStore>) -> bool {
    archive_store.is_ready()
}

fn runtime_content_ready(
    config: Res<RuntimeLaunchConfig>,
    archive_store: Res<HdpArchiveStore>,
) -> bool {
    config.asset_mode == RuntimeAssetMode::Directory || hdp_archive_ready(archive_store)
}

fn runtime_not_initialized(frontend: Option<Res<scene::FrontendState>>) -> bool {
    frontend.is_none()
}

/// Wait for the package bootstrap on every platform; do not read the desktop
/// filesystem in the presentation host or resize an already-created target.
#[derive(Resource, Default)]
struct ProjectCanvasStatus(Option<bool>);

fn project_canvas_ready(status: Res<ProjectCanvasStatus>) -> bool {
    status.0 == Some(true)
}

fn prepare_project_canvas(
    vfs: Res<VfsResource>,
    archives: Res<HdpArchiveStore>,
    mut config: ResMut<RuntimeLaunchConfig>,
    mut status: ResMut<ProjectCanvasStatus>,
) {
    if status.0.is_some() {
        return;
    }
    if config.asset_mode == RuntimeAssetMode::Hdp && !archives.is_ready() {
        return;
    }
    match vfs.0.load_canvas_size() {
        Ok(size) => {
            if let Some([width, height]) = size {
                config.canvas_size = UVec2::new(width, height);
            }
            status.0 = Some(true);
        }
        Err(error) => {
            script::emit_script_diagnostic(
                "failed to configure project canvas",
                &error.to_string(),
            );
            status.0 = Some(false);
        }
    }
}

#[cfg(test)]
mod project_canvas_tests {
    use super::*;

    #[test]
    fn waits_for_package_settings_before_consumers_observe_project_dimensions() {
        #[derive(Resource)]
        struct Observed(UVec2);
        for (settings, expected, valid) in [
            (".{canvasSize: (800, 1200)}", UVec2::new(800, 1200), true),
            (".{}", UVec2::new(640, 480), true),
            (".{canvasSize: (0, 1200)}", UVec2::new(640, 480), false),
        ] {
            let store = HdpArchiveStore::default();
            let vfs = vfs::HdpVfs::new_with_config_and_store(
                "unused-fixture-root",
                "hdp://fixture.hdp/settings.hson",
                "startup.hks",
                store.clone(),
            );
            let mut app = App::new();
            app.insert_resource(VfsResource(Arc::new(vfs)))
                .insert_resource(store.clone())
                .insert_resource(RuntimeLaunchConfig {
                    canvas_size: UVec2::new(640, 480),
                    ..default()
                })
                .init_resource::<ProjectCanvasStatus>()
                .add_systems(Update, prepare_project_canvas)
                .add_systems(
                    Update,
                    (|mut commands: Commands, config: Res<RuntimeLaunchConfig>| {
                        commands.insert_resource(Observed(config.canvas_size));
                    })
                    .after(prepare_project_canvas)
                    .run_if(project_canvas_ready),
                );
            app.update();
            assert!(
                !app.world().contains_resource::<Observed>(),
                "must wait for asynchronous package availability"
            );
            let mut package = hiraku_hdp::PackageBuilder::new();
            package
                .add_file("settings.hson", settings.as_bytes())
                .expect("settings fixture");
            let packed = package
                .build(hiraku_hdp::PackOptions::default())
                .expect("fixture package");
            let archive =
                hiraku_hdp::Archive::from_bytes(Arc::<[u8]>::from(packed.volumes[0].clone()))
                    .expect("archive");
            store
                .publish(Arc::new(archive), "fixture.hdp".into())
                .expect("publish");
            app.update();
            assert_eq!(
                app.world().resource::<RuntimeLaunchConfig>().canvas_size,
                expected
            );
            if valid {
                assert_eq!(app.world().resource::<Observed>().0, expected);
            } else {
                assert!(
                    !app.world().contains_resource::<Observed>(),
                    "invalid configuration must not create a canvas"
                );
            }
        }
    }
}

fn runtime_initialized(frontend: Option<Res<scene::FrontendState>>) -> bool {
    frontend.is_some()
}

fn boot_runtime(
    vfs: Res<VfsResource>,
    assets: Res<AssetServer>,
    mut dependencies: ResMut<dependencies::ScriptDependencies>,
    user_settings: Res<storage::UserSettings>,
    mut script_runtime: ResMut<ScriptRuntimeState>,
    mut booted: Local<bool>,
) {
    if *booted {
        return;
    }
    match vfs.0.load_startup_script_path() {
        Ok(startup_script) => {
            match dependencies.prepare(&vfs.0, &assets, std::slice::from_ref(&startup_script)) {
                Ok(false) => return,
                Err(error) => {
                    script::emit_script_diagnostic("failed to preload startup:", &error);
                    *booted = true;
                    return;
                }
                Ok(true) => (),
            }
            info!("startup script: {startup_script}");
            let result = vfs
                .0
                .read_text(&startup_script)
                .map_err(|error| error.to_string())
                .and_then(|source| script::compile_story_program(&vfs.0, &startup_script, &source))
                .and_then(|bytecode| {
                    StoryRuntime::new(bytecode).map_err(|error| error.to_string())
                });
            match result {
                Ok(mut story) => {
                    story.set_globals(script::capabilities::engine_globals(&user_settings));
                    script_runtime.story = Some(story);
                    script_runtime.current_script = Some(startup_script);
                }
                Err(error) => script::emit_script_diagnostic(
                    &format!("failed to start HKS script `{startup_script}`:"),
                    &error,
                ),
            }
            *booted = true;
        }
        Err(err) => {
            script::emit_script_diagnostic("failed to resolve startup script:", &err.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_config_uses_loose_default_document_paths() {
        let config = RuntimeLaunchConfig::directory("example-assets");
        assert_eq!(config.asset_mode, RuntimeAssetMode::Directory);
        assert_eq!(config.asset_root, "example-assets");
        assert_eq!(config.settings_path, "settings.hson");
        assert_eq!(config.default_startup_script, "startup.hks");
    }
}
