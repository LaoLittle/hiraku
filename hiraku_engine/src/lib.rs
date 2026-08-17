mod assets;
mod character;
mod data;
mod effect;
mod proto;
mod scene;
mod script;
mod state;
mod storage;
mod texture;
mod ui;
mod vfs;

use std::sync::Arc;

use assets::{BytesAsset, BytesAssetLoader, RhaiScriptAsset, RhaiScriptAssetLoader};
use bevy::{
    app::PluginGroupBuilder,
    asset::{AssetApp, io::AssetSourceId},
    camera::{ClearColorConfig, RenderTarget},
    prelude::*,
    sprite_render::Material2dPlugin,
};
use effect::transition::RuleTransitionMaterial;
use effect::{blur::BlurEffectPlugin, custom::CustomScreenEffectMaterial};
use scene::{
    advance_dialogue_on_input, animate_bgm_fades, animate_camera_shake, animate_camera_transition,
    animate_character_motion_effects, animate_custom_effects, animate_dialogue_text_reveal,
    animate_rule_transitions, animate_visual_tweens, apply_animation_cancellations,
    apply_live_audio_settings, cleanup_stale_screen_ui, handle_choice_buttons,
    handle_choice_keyboard, handle_runtime_menu_buttons, handle_screen_buttons,
    handle_screen_image_buttons, poll_pending_character_shows, poll_voice_playback,
    process_script_commands, setup_frontend, setup_stage, sync_scene_snapshot,
    tick_animation_waits, tick_pending_waits, tick_script_batches,
};
#[cfg(target_arch = "wasm32")]
use script::drive_web_script_runtime;
use script::{ScriptBootstrap, spawn_script_runtime};
use state::SceneSharedState;
use texture::{build_texture_atlases, texture_atlases_ready};
use vfs::{
    ASSET_SOURCE_ID, HDP_SOURCE_ID, VfsResource, WORKSPACE_SOURCE_ID, file_asset_source_builder,
    hdp_asset_source_builder, workspace_base_path,
};

#[derive(Clone, Debug, Resource)]
pub struct RuntimeLaunchConfig {
    pub asset_root: String,
    pub settings_path: String,
    pub default_startup_script: String,
    pub window_title: String,
    pub render_target: RenderTarget,
    pub camera_order: isize,
    pub camera_clear_color: ClearColorConfig,
    pub embedded_asset_archive: Option<&'static [u8]>,
}

impl Default for RuntimeLaunchConfig {
    fn default() -> Self {
        Self {
            asset_root: vfs::DEFAULT_ASSET_ROOT.to_string(),
            settings_path: vfs::DEFAULT_SETTINGS_PATH.to_string(),
            default_startup_script: vfs::DEFAULT_STARTUP_SCRIPT.to_string(),
            window_title: "hiraku".to_string(),
            render_target: RenderTarget::Window(bevy::window::WindowRef::Primary),
            camera_order: 0,
            camera_clear_color: ClearColorConfig::Default,
            embedded_asset_archive: None,
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
        app.register_asset_source(
            AssetSourceId::Name(HDP_SOURCE_ID.into()),
            hdp_asset_source_builder(config.asset_root, config.embedded_asset_archive),
        );
        app.register_asset_source(
            AssetSourceId::Name(ASSET_SOURCE_ID.into()),
            file_asset_source_builder(workspace_base_path().join(vfs::DEFAULT_ASSET_ROOT)),
        );
        app.register_asset_source(
            AssetSourceId::Name(WORKSPACE_SOURCE_ID.into()),
            file_asset_source_builder(workspace_base_path()),
        );
    }
}

pub struct HirakuPlugin;

impl Plugin for HirakuPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            Material2dPlugin::<CustomScreenEffectMaterial>::default(),
            Material2dPlugin::<RuleTransitionMaterial>::default(),
            BlurEffectPlugin,
        ));
        effect::custom::load_internal_shaders(app);
        effect::transition::load_internal_shaders(app);

        app.init_asset::<RhaiScriptAsset>()
            .init_asset::<BytesAsset>()
            .init_asset::<TextureAtlasLayout>()
            .init_resource::<texture::TextureAtlasCatalog>()
            .init_resource::<script::InlineDialogueControlResource>()
            .init_asset_loader::<RhaiScriptAssetLoader>()
            .init_asset_loader::<BytesAssetLoader>()
            .add_systems(Startup, (setup_frontend, setup_stage).chain())
            .add_systems(Update, build_texture_atlases)
            .add_systems(
                Update,
                boot_runtime
                    .after(build_texture_atlases)
                    .run_if(texture_atlases_ready),
            )
            .add_systems(Update, process_script_commands)
            .add_systems(
                Update,
                animate_camera_transition
                    .after(process_script_commands)
                    .before(animate_camera_shake),
            )
            .add_systems(
                Update,
                apply_live_audio_settings.after(process_script_commands),
            )
            .add_systems(Update, cleanup_stale_screen_ui)
            .add_systems(Update, handle_screen_buttons)
            .add_systems(Update, handle_screen_image_buttons)
            .add_systems(Update, handle_choice_buttons)
            .add_systems(Update, handle_runtime_menu_buttons)
            .add_systems(Update, handle_choice_keyboard)
            .add_systems(Update, animate_dialogue_text_reveal)
            .add_systems(Update, advance_dialogue_on_input)
            .add_systems(Update, tick_pending_waits)
            .add_systems(
                Update,
                (
                    apply_animation_cancellations,
                    animate_visual_tweens,
                    animate_bgm_fades,
                    animate_custom_effects,
                    animate_rule_transitions,
                    animate_camera_shake,
                    animate_character_motion_effects,
                    poll_voice_playback,
                    poll_pending_character_shows,
                    tick_animation_waits,
                    tick_script_batches,
                    sync_scene_snapshot,
                ),
            );

        #[cfg(target_arch = "wasm32")]
        app.add_systems(
            Update,
            drive_web_script_runtime.before(process_script_commands),
        );
    }
}

/// Prepare an app for Hiraku before adding Bevy's `DefaultPlugins`.
///
/// This registers Hiraku's custom asset sources, which Bevy requires before
/// `AssetPlugin` is built. After adding `DefaultPlugins`, add
/// [`HirakuPluginGroup`] to install Hiraku's materials and runtime systems.
pub fn configure_runtime_app(app: &mut App, config: RuntimeLaunchConfig) {
    let base_path = workspace_base_path();
    let asset_root_path = base_path.join(&config.asset_root);
    let vfs = Arc::new(vfs::HdpVfs::new_with_config_and_archive(
        asset_root_path,
        config.settings_path.clone(),
        config.default_startup_script.clone(),
        config.embedded_asset_archive,
    ));

    app.insert_resource(config);
    app.insert_resource(VfsResource(vfs));
    app.insert_resource(SceneSharedState::default());
    app.insert_resource(ClearColor(Color::BLACK));
    app.add_plugins(HirakuAssetSourcePlugin);
}

fn boot_runtime(
    mut commands: Commands,
    vfs: Res<VfsResource>,
    scene_state: Res<SceneSharedState>,
    mut booted: Local<bool>,
) {
    if *booted {
        return;
    }
    match vfs.0.load_startup_script_path() {
        Ok(startup_script) => {
            info!("startup script: {startup_script}");
            spawn_script_runtime(
                &mut commands,
                vfs.0.clone(),
                scene_state.0.clone(),
                ScriptBootstrap::new(startup_script),
            );
            *booted = true;
        }
        Err(err) => {
            error!("failed to resolve startup script: {err}");
        }
    }
}
