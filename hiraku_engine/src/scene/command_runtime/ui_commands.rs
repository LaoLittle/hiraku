use super::*;

pub(super) fn dispatch_ui_command(
    command: UiCommand,
    commands: &mut Commands,
    asset_server: &AssetServer,
    images: &Assets<Image>,
    texture_atlases: &TextureAtlasCatalog,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    screen_state: &mut ScreenUiState,
    overlay_state: &mut OverlayUiState,
) {
    match command {
        UiCommand::ShowScreen { screen, done } => {
            let spawned = spawn_screen_ui(
                commands,
                asset_server,
                texture_atlases,
                ui_fonts,
                ui_style,
                &screen,
            );
            let root = spawned.root;
            let previous = screen_state.active_root.take();
            let images_ready = screen_images_ready(images, &spawned.image_handles);
            if previous.is_none() && images_ready {
                commands
                    .entity(root)
                    .insert((Visibility::Inherited, GlobalZIndex(SCREEN_MODAL_ACTIVE_Z)));
                screen_state.active_root = Some(root);
                screen_state.waiting = done;
            } else {
                commands
                    .entity(root)
                    .insert((Visibility::Hidden, GlobalZIndex(SCREEN_MODAL_PENDING_Z)));
                screen_state.pending_root = Some(crate::ui::PendingScreenRoot {
                    entity: root,
                    previous,
                    wait_images: spawned.image_handles,
                    ready_frames_remaining: SCREEN_READY_FRAMES,
                    done,
                });
                screen_state.waiting = None;
            }
        }
        UiCommand::ShowOverlay { name, screen } => {
            if let Some(root) = overlay_state.roots.remove(&name) {
                commands.entity(root).try_despawn();
            }
            let spawned = spawn_screen_ui(
                commands,
                asset_server,
                texture_atlases,
                ui_fonts,
                ui_style,
                &screen,
            );
            commands
                .entity(spawned.root)
                .insert((Visibility::Inherited, GlobalZIndex(SCREEN_ACTIVE_Z + 10)));
            overlay_state.roots.insert(name, spawned.root);
        }
        UiCommand::HideOverlay { name } => {
            if let Some(root) = overlay_state.roots.remove(&name) {
                commands.entity(root).try_despawn();
            }
        }
    }
}
