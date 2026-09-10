use super::*;

pub(super) fn dispatch_ui_command(
    command: UiCommand,
    commands: &mut Commands,
    asset_server: &AssetServer,
    images: &Assets<Image>,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    screen_state: &mut ScreenUiState,
    overlay_state: &mut OverlayUiState,
    canvas: &crate::HirakuCanvas,
    preview: &mut crate::scene::save_preview::SavePreview,
) {
    match command {
        UiCommand::ShowScreen { screen, done, push } => {
            if screen_state.active_root.is_none()
                && screen_state.pending_root.is_none()
                && screen_state.stack.is_empty()
            {
                crate::scene::save_preview::capture(commands, canvas, preview);
            }
            let spawned =
                spawn_screen_ui(commands, asset_server, ui_fonts, ui_style, &screen, true);
            let root = spawned.root;
            let mut previous = screen_state.active_root.take();
            if push {
                if let Some(root) = previous.take() {
                    screen_state.stack.push((root, screen_state.waiting.take()));
                }
                for (index, (root, _)) in screen_state.stack.iter().enumerate() {
                    commands
                        .entity(*root)
                        .insert(GlobalZIndex(SCREEN_MODAL_ACTIVE_Z + index as i32 * 3));
                }
            }
            let depth_offset = screen_state.stack.len() as i32 * 3;
            let images_ready = screen_images_ready(images, &spawned.image_handles);
            if previous.is_none() && images_ready && preview.capture.is_none() {
                commands.entity(root).insert((
                    Visibility::Inherited,
                    GlobalZIndex(SCREEN_MODAL_ACTIVE_Z + depth_offset),
                ));
                screen_state.active_root = Some(root);
                screen_state.waiting = done;
            } else {
                commands.entity(root).insert((
                    Visibility::Hidden,
                    GlobalZIndex(SCREEN_MODAL_PENDING_Z + depth_offset),
                ));
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
        UiCommand::ShowOverlay { name, screen, lifetime } => {
            if let Some(root) = overlay_state.roots.remove(&name) {
                commands.entity(root).try_despawn();
            }
            let spawned =
                spawn_screen_ui(commands, asset_server, ui_fonts, ui_style, &screen, false);
            commands
                .entity(spawned.root)
                .insert((Visibility::Inherited, GlobalZIndex(SCREEN_ACTIVE_Z + 10)));
            if let Some(seconds) = lifetime {
                commands.entity(spawned.root).insert(super::super::screen_ui::OverlayLifetime(
                    Timer::from_seconds(seconds, TimerMode::Once),
                ));
            }
            overlay_state.roots.insert(name, spawned.root);
        }
        UiCommand::HideOverlay { name } => {
            if let Some(root) = overlay_state.roots.remove(&name) {
                commands.entity(root).try_despawn();
            }
        }
    }
}
