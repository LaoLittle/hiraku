mod assets;
mod character;
mod effect;
mod scene;
mod script;
mod state;
mod storage;
mod transition;
mod ui;
mod vfs;

use std::sync::Arc;

use assets::{BytesAsset, BytesAssetLoader, RhaiScriptAsset, RhaiScriptAssetLoader};
use bevy::{
    asset::{io::AssetSourceId, AssetApp, AssetMetaCheck, AssetPlugin},
    prelude::*,
    sprite_render::Material2dPlugin,
    window::WindowPlugin,
};
use effect::CustomScreenEffectMaterial;
use scene::{
    advance_dialogue_on_input, animate_bgm_fades, animate_camera_shake,
    animate_dialogue_text_reveal,
    apply_animation_cancellations,
    animate_character_motion_effects, animate_custom_effects, animate_rule_transitions,
    animate_visual_tweens, handle_choice_buttons,
    handle_choice_keyboard, handle_screen_buttons, poll_pending_character_shows, poll_voice_playback,
    process_script_commands, setup_frontend, setup_stage, sync_scene_snapshot,
    tick_animation_waits, tick_pending_waits, tick_script_batches,
};
use script::{ScriptBootstrap, spawn_script_runtime};
use state::SceneSharedState;
use transition::RuleTransitionMaterial;
use vfs::{VfsResource, default_asset_source_builder};

#[derive(Clone, Debug)]
pub struct RuntimeLaunchConfig {
    pub asset_root: String,
    pub settings_path: String,
    pub default_startup_script: String,
    pub window_title: String,
}

impl Default for RuntimeLaunchConfig {
    fn default() -> Self {
        Self {
            asset_root: vfs::DEFAULT_ASSET_ROOT.to_string(),
            settings_path: vfs::DEFAULT_SETTINGS_PATH.to_string(),
            default_startup_script: vfs::DEFAULT_STARTUP_SCRIPT.to_string(),
            window_title: "hiraku".to_string(),
        }
    }
}

pub fn run_app(config: RuntimeLaunchConfig) {
    let base_path = bevy::asset::io::file::FileAssetReader::get_base_path();
    let asset_root_path = base_path.join(&config.asset_root);
    let vfs = Arc::new(vfs::HdpVfs::new_with_config(
        asset_root_path,
        config.settings_path.clone(),
        config.default_startup_script.clone(),
    ));

    let mut app = App::new();

    app.register_asset_source(
        AssetSourceId::Default,
        default_asset_source_builder(config.asset_root.clone()),
    );
    app.insert_resource(VfsResource(vfs));
    app.insert_resource(SceneSharedState::default());
    app.insert_resource(ClearColor(Color::BLACK));
    app.add_plugins((
        DefaultPlugins
            .set(AssetPlugin {
                file_path: config.asset_root.clone(),
                meta_check: AssetMetaCheck::Never,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: config.window_title,
                    present_mode: bevy::window::PresentMode::AutoVsync,
                    ..default()
                }),
                ..default()
            }),
        Material2dPlugin::<CustomScreenEffectMaterial>::default(),
        Material2dPlugin::<RuleTransitionMaterial>::default(),
    ));

    effect::load_internal_shaders(&mut app);
    transition::load_internal_shaders(&mut app);

    app.init_asset::<RhaiScriptAsset>()
        .init_asset::<BytesAsset>()
        .init_asset_loader::<RhaiScriptAssetLoader>()
        .init_asset_loader::<BytesAssetLoader>()
        .add_systems(Startup, (setup_frontend, setup_stage, boot_runtime).chain())
        .add_systems(
            Update,
            (
                process_script_commands,
                handle_screen_buttons,
                handle_choice_buttons,
                handle_choice_keyboard,
                animate_dialogue_text_reveal,
                advance_dialogue_on_input,
                tick_pending_waits,
            ),
        )
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
        )
        .run();
}

fn boot_runtime(mut commands: Commands, vfs: Res<VfsResource>, scene_state: Res<SceneSharedState>) {
    match vfs.0.load_startup_script_path() {
        Ok(startup_script) => {
            info!("startup script: {startup_script}");
            spawn_script_runtime(
                &mut commands,
                vfs.0.clone(),
                scene_state.0.clone(),
                ScriptBootstrap::new(startup_script),
            );
        }
        Err(err) => {
            error!("failed to resolve startup script: {err}");
        }
    }
}
