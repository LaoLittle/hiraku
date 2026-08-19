mod assets;
mod audio;
mod character;
mod data;
mod effect;
mod hks_capabilities;
pub mod hks_prelude;
mod proto;
pub mod render;
mod scene;
mod script;
mod state;
mod storage;
mod texture;
mod ui;
mod vfs;

use std::sync::Arc;

use assets::{BytesAsset, BytesAssetLoader, HdpArchive, HdpArchiveLoader};
use bevy::{
    app::PluginGroupBuilder,
    asset::{AssetApp, io::AssetSourceId},
    camera::ClearColorConfig,
    prelude::*,
    sprite_render::Material2dPlugin,
};
use effect::transition::RuleTransitionMaterial;
use effect::{blur::BlurEffectPlugin, custom::CustomScreenEffectMaterial};
use render::camera::assign_render_layers;
use scene::{
    advance_dialogue_on_input, animate_bgm_fades, animate_camera_shake, animate_camera_transition,
    animate_character_motion_effects, animate_custom_effects, animate_dialogue_text_reveal,
    animate_rule_transitions, animate_visual_tweens, apply_animation_cancellations,
    apply_live_audio_settings, bridge_ir_events, cleanup_stale_screen_ui, handle_choice_buttons,
    handle_choice_keyboard, handle_runtime_menu_buttons, handle_screen_buttons,
    handle_screen_image_buttons, poll_pending_character_shows, poll_voice_playback,
    process_script_commands, setup_frontend, setup_stage, sync_scene_snapshot,
    tick_animation_waits, tick_pending_waits, tick_script_batches,
    update_offscreen_ui_interactions,
};
pub use script::{
    IrChoiceOption, IrCommand, IrEvent, IrExpressionId, IrInstruction, IrProgram,
    IrValidationError, IrVm, IrVmSnapshot, IrVmStatus, IrWaitKind,
};
use script::{IrRuntime, compile_story_program, tick_ir_runtime};
use state::SceneSharedState;
use texture::{build_texture_atlases, texture_atlases_ready};
use vfs::{HDP_SOURCE_ID, HdpArchiveStore, VfsResource, hdp_asset_source_builder};

#[derive(Clone, Debug, Resource)]
pub struct RuntimeLaunchConfig {
    pub asset_root: String,
    pub settings_path: String,
    pub default_startup_script: String,
    pub window_title: String,
    /// Fixed logical resolution rendered by Hiraku before presentation by the host game.
    pub canvas_size: UVec2,
    pub camera_order: isize,
    pub camera_clear_color: ClearColorConfig,
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

impl Default for RuntimeLaunchConfig {
    fn default() -> Self {
        Self {
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
            Material2dPlugin::<CustomScreenEffectMaterial>::default(),
            Material2dPlugin::<RuleTransitionMaterial>::default(),
            BlurEffectPlugin,
        ));
        effect::custom::load_internal_shaders(app);
        effect::transition::load_internal_shaders(app);

        let archive_store = app.world().resource::<HdpArchiveStore>().clone();
        let archive_path = archive_path_from_config(app.world().resource::<RuntimeLaunchConfig>());

        app.init_asset::<HdpArchive>()
            .init_asset::<BytesAsset>()
            .init_asset::<TextureAtlasLayout>()
            .init_resource::<texture::TextureAtlasCatalog>()
            .init_resource::<IrRuntime>()
            .register_asset_loader(HdpArchiveLoader::new(archive_store))
            .init_asset_loader::<BytesAssetLoader>()
            .add_systems(
                Update,
                (setup_frontend, setup_stage)
                    .chain()
                    .run_if(hdp_archive_ready)
                    .run_if(runtime_not_initialized),
            )
            .add_systems(Update, build_texture_atlases)
            .add_systems(Update, assign_render_layers)
            .configure_sets(Update, HirakuRuntimeSystems.run_if(runtime_initialized))
            .add_systems(
                Update,
                boot_runtime
                    .after(build_texture_atlases)
                    .run_if(texture_atlases_ready),
            )
            .add_systems(Update, tick_ir_runtime.in_set(HirakuRuntimeSystems))
            .add_systems(
                Update,
                bridge_ir_events
                    .after(tick_ir_runtime)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                process_script_commands
                    .after(bridge_ir_events)
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
                cleanup_stale_screen_ui
                    .after(process_script_commands)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                update_offscreen_ui_interactions
                    .after(cleanup_stale_screen_ui)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                handle_screen_buttons
                    .after(update_offscreen_ui_interactions)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                handle_screen_image_buttons
                    .after(update_offscreen_ui_interactions)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                handle_choice_buttons
                    .after(update_offscreen_ui_interactions)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                handle_runtime_menu_buttons
                    .after(update_offscreen_ui_interactions)
                    .in_set(HirakuRuntimeSystems),
            )
            .add_systems(Update, handle_choice_keyboard.in_set(HirakuRuntimeSystems))
            .add_systems(
                Update,
                animate_dialogue_text_reveal.in_set(HirakuRuntimeSystems),
            )
            .add_systems(
                Update,
                advance_dialogue_on_input
                    .after(update_offscreen_ui_interactions)
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

        let archive = app.world().resource::<AssetServer>().load(archive_path);
        app.insert_resource(HdpArchiveHandle(archive));
    }
}

/// Prepare an app for Hiraku before adding Bevy's `DefaultPlugins`.
///
/// This registers Hiraku's custom asset sources, which Bevy requires before
/// `AssetPlugin` is built. After adding `DefaultPlugins`, add
/// [`HirakuPluginGroup`] to install Hiraku's materials and runtime systems.
pub fn configure_runtime_app(app: &mut App, config: RuntimeLaunchConfig) {
    let asset_root_path = std::path::PathBuf::from(&config.asset_root);
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
    app.add_plugins(HirakuAssetSourcePlugin);
}

fn archive_path_from_config(config: &RuntimeLaunchConfig) -> String {
    vfs::split_hdp_asset_path(&config.settings_path)
        .map(|(archive, _)| archive)
        .unwrap_or_else(|| "main.hdp".to_string())
}

fn hdp_archive_ready(archive_store: Res<HdpArchiveStore>) -> bool {
    archive_store.is_ready()
}

fn runtime_not_initialized(frontend: Option<Res<scene::FrontendState>>) -> bool {
    frontend.is_none()
}

fn runtime_initialized(frontend: Option<Res<scene::FrontendState>>) -> bool {
    frontend.is_some()
}

fn boot_runtime(vfs: Res<VfsResource>, mut ir_runtime: ResMut<IrRuntime>, mut booted: Local<bool>) {
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
                .and_then(|source| compile_story_program(&startup_script, &source))
                .and_then(|program| IrVm::new(program).map_err(|error| error.to_string()));
            match result {
                Ok(vm) => {
                    info!("starting startup script in HKS IR runtime");
                    ir_runtime.vm = Some(vm);
                    ir_runtime.current_script = Some(startup_script);
                }
                Err(error) => error!("failed to start HKS script `{startup_script}`: {error}"),
            }
            *booted = true;
        }
        Err(err) => {
            error!("failed to resolve startup script: {err}");
        }
    }
}
