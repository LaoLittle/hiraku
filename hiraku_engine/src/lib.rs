mod assets;
mod audio;
mod character;
mod data;
mod effect;
mod glossary;
pub mod input;
mod proto;
pub mod render;
mod scene;
mod script;
mod state;
mod storage;
mod texture;
mod ui;
mod vfs;

pub use script::{UiContext, UiIntent};
pub use ui::UiModels;

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
use effect::{blur::BlurEffectPlugin, custom::CustomScreenEffectMaterial};
use render::camera::{animate_camera_shake, animate_camera_transition, assign_render_layers};
use render::character_part::{AlphaMaskMaterial, MultiplyMaterial};
use scene::{
    advance_dialogue_on_input, animate_bgm_fades, animate_character_motion_effects,
    animate_custom_effects, animate_dialogue_text_reveal, animate_rule_transitions,
    animate_screen_ui, animate_visual_tweens, apply_animation_cancellations,
    apply_live_audio_settings, bridge_story_events, cleanup_stale_screen_ui,
    handle_choice_action_input, handle_choice_buttons, handle_runtime_menu_buttons,
    handle_screen_buttons, handle_screen_image_buttons, poll_pending_character_shows,
    poll_voice_playback, prepare_bgm_preludes, process_script_commands, setup_frontend,
    setup_stage, sync_scene_snapshot, tick_animation_waits, tick_pending_waits,
    tick_script_batches, update_builtin_ui_models, update_ui_reactive_bindings,
    update_ui_text_bindings,
};
use script::{
    ScriptResponseMessage, ScriptRuntimeState, StoryRuntime, compile_story_bytecode,
    tick_script_runtime,
};
use state::SceneSharedState;
use texture::{build_texture_atlases, texture_atlases_ready};
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
    /// Fixed logical resolution rendered by Hiraku before presentation by the host game.
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
        app.add_plugins((
            MaterialPlugin::<CustomScreenEffectMaterial>::default(),
            MaterialPlugin::<RuleTransitionMaterial>::default(),
            MaterialPlugin::<AlphaMaskMaterial>::default(),
            MaterialPlugin::<MultiplyMaterial>::default(),
            BlurEffectPlugin,
        ));
        effect::custom::load_internal_shaders(app);
        effect::transition::load_internal_shaders(app);
        render::character_part::load_internal_shaders(app);
        render::world_sprite::install(app);

        let archive_path = archive_path_from_config(app.world().resource::<RuntimeLaunchConfig>());
        let archive_store = app.world().resource::<HdpArchiveStore>().clone();

        app.init_asset::<HdpArchive>()
            .init_asset::<BytesAsset>()
            .init_asset::<TextureAtlasLayout>()
            .add_audio_source::<audio::PreludeLoopAudio>()
            .init_resource::<HdpVolumeLoads>()
            .init_resource::<texture::TextureAtlasCatalog>()
            .add_message::<input::HirakuPointerInput>()
            .add_message::<input::HirakuActionInput>()
            .add_systems(
                First,
                input::bridge_virtual_pointers.before(bevy::picking::PickingSystems::Input),
            )
            .init_resource::<ScriptRuntimeState>()
            .init_resource::<UiModels>()
            .add_message::<ScriptResponseMessage>()
            .register_asset_loader(HdpArchiveLoader::new(archive_store))
            .init_asset_loader::<BytesAssetLoader>()
            .add_systems(Update, stream_requested_hdp_volumes)
            .add_systems(
                Update,
                (setup_frontend, setup_stage)
                    .chain()
                    .run_if(runtime_content_ready)
                    .run_if(runtime_not_initialized),
            )
            .add_systems(Update, build_texture_atlases)
            .add_systems(Update, assign_render_layers.after(process_script_commands))
            .configure_sets(Update, HirakuRuntimeSystems.run_if(runtime_initialized))
            .add_systems(
                Update,
                boot_runtime
                    .after(build_texture_atlases)
                    .run_if(texture_atlases_ready),
            )
            .add_systems(Update, tick_script_runtime.in_set(HirakuRuntimeSystems))
            .add_systems(
                Update,
                bridge_story_events
                    .after(tick_script_runtime)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                process_script_commands
                    .after(bridge_story_events)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                (
                    update_builtin_ui_models,
                    update_ui_text_bindings,
                    update_ui_reactive_bindings,
                    animate_screen_ui,
                )
                    .chain()
                    .after(process_script_commands)
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
                handle_choice_buttons
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
                    animate_visual_tweens.in_set(HirakuRuntimeSystems),
                    animate_bgm_fades.in_set(HirakuRuntimeSystems),
                    animate_custom_effects.in_set(HirakuRuntimeSystems),
                    animate_rule_transitions.in_set(HirakuRuntimeSystems),
                    animate_camera_shake.in_set(HirakuRuntimeSystems),
                    animate_character_motion_effects.in_set(HirakuRuntimeSystems),
                    poll_voice_playback.in_set(HirakuRuntimeSystems),
                    poll_pending_character_shows.in_set(HirakuRuntimeSystems),
                    tick_animation_waits.in_set(HirakuRuntimeSystems),
                    tick_script_batches.in_set(HirakuRuntimeSystems),
                    sync_scene_snapshot.in_set(HirakuRuntimeSystems),
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

fn runtime_initialized(frontend: Option<Res<scene::FrontendState>>) -> bool {
    frontend.is_some()
}

fn boot_runtime(
    vfs: Res<VfsResource>,
    user_settings: Res<storage::UserSettings>,
    mut script_runtime: ResMut<ScriptRuntimeState>,
    mut booted: Local<bool>,
) {
    if *booted {
        return;
    }
    match vfs.0.load_startup_script_path() {
        Ok(startup_script) => {
            info!("startup script: {startup_script}");
            let result = vfs
                .0
                .read_text(&startup_script)
                .map_err(|error| error.to_string())
                .and_then(|source| compile_story_bytecode(&startup_script, &source))
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
            error!("failed to resolve startup script: {err}");
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
