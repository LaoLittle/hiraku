//! Optional script-defined loading presentation. It is not a modal story call
//! and never participates in the story's response/wait protocol.
use super::*;

#[derive(Component)]
pub(crate) struct LoadingUi;

#[allow(clippy::too_many_arguments)]
pub(crate) fn sync(
    mut commands: Commands,
    state: Res<crate::dependencies::ScriptDependencies>,
    runtime: Res<ScriptRuntimeState>,
    vfs: Option<Res<VfsResource>>,
    preferences: Option<Res<UserSettings>>,
    textures: Option<Res<TextureCatalog>>,
    terms: Option<Res<TermCatalog>>,
    fonts: Option<Res<UiFonts>>,
    style: Option<Res<UiStyle>>,
    assets: Res<AssetServer>,
    roots: Query<Entity, With<LoadingUi>>,
    mut attempted: Local<Option<(String, u32)>>,
) {
    if !state.loading {
        for entity in &roots {
            commands.entity(entity).try_despawn();
        }
        *attempted = None;
        return;
    }
    let Some(path) = runtime.ui_registry.get("loading") else {
        return;
    };
    let fraction = state.progress(&assets);
    let key = (path.clone(), (fraction * 100.0) as u32);
    if attempted.as_ref() == Some(&key) {
        return;
    }
    let (Some(vfs), Some(preferences), Some(fonts), Some(style)) = (vfs, preferences, fonts, style)
    else {
        return;
    };
    *attempted = Some(key);
    match command_runtime::evaluate_ui_at_with_arguments(
        path,
        &runtime,
        &vfs,
        &preferences,
        textures.as_deref(),
        terms.as_deref(),
        BTreeMap::new(),
        &[StoredValue::Float(fraction)],
    ) {
        Ok(screen) => {
            for entity in &roots {
                commands.entity(entity).try_despawn();
            }
            let spawned = spawn_screen_ui(&mut commands, &assets, &fonts, &style, &screen, false);
            commands.entity(spawned.root).insert((
                LoadingUi,
                Visibility::Inherited,
                GlobalZIndex(i32::MAX),
            ));
        }
        Err(error) => crate::script::emit_script_diagnostic(
            "failed to render loading UI (using black fallback):",
            &error,
        ),
    }
}
