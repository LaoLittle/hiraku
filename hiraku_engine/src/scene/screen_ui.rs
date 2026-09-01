use super::*;
use crate::ui::UiEffect;
use bevy::{
    input::mouse::MouseScrollUnit,
    picking::events::Scroll,
    ui::{VisualBox, widget::NodeImageMode},
};

#[derive(Clone, Debug, Message)]
pub struct UiEffectMessage(pub UiEffect);

pub(super) fn clear_screen_ui(commands: &mut Commands, screen_state: &mut ScreenUiState) {
    if let Some(root) = screen_state.active_root.take() {
        commands.entity(root).try_despawn();
    }
    if let Some(pending) = screen_state.pending_root.take() {
        commands.entity(pending.entity).try_despawn();
        if let Some(previous) = pending.previous {
            commands.entity(previous).try_despawn();
        }
    }
    for stale in screen_state.stale_roots.drain(..) {
        commands.entity(stale.entity).try_despawn();
    }
}

pub(super) fn clear_overlay_ui(commands: &mut Commands, overlay_state: &mut OverlayUiState) {
    for (_, root) in overlay_state.roots.drain() {
        commands.entity(root).try_despawn();
    }
}

pub fn cleanup_stale_screen_ui(
    mut commands: Commands,
    images: Res<Assets<Image>>,
    mut screen_state: ResMut<ScreenUiState>,
) {
    if let Some(mut pending) = screen_state.pending_root.take() {
        if screen_images_ready(&images, &pending.wait_images) && pending.ready_frames_remaining == 0
        {
            commands
                .entity(pending.entity)
                .insert((Visibility::Inherited, GlobalZIndex(SCREEN_MODAL_ACTIVE_Z)));
            if let Some(previous) = pending.previous {
                commands
                    .entity(previous)
                    .insert(GlobalZIndex(SCREEN_MODAL_STALE_Z));
                screen_state.stale_roots.push(StaleScreenRoot {
                    entity: previous,
                    frames_remaining: 2,
                    wait_images: Vec::new(),
                });
            }
            screen_state.active_root = Some(pending.entity);
            screen_state.waiting = pending.done;
        } else {
            if screen_images_ready(&images, &pending.wait_images) {
                commands
                    .entity(pending.entity)
                    .insert((Visibility::Inherited, GlobalZIndex(SCREEN_MODAL_PENDING_Z)));
                pending.ready_frames_remaining = pending.ready_frames_remaining.saturating_sub(1);
            }
            screen_state.pending_root = Some(pending);
        }
    }

    let mut survivors = Vec::new();
    for mut stale in screen_state.stale_roots.drain(..) {
        stale.frames_remaining = stale.frames_remaining.saturating_sub(1);
        let images_ready = stale
            .wait_images
            .iter()
            .all(|handle| images.contains(handle));
        if stale.frames_remaining == 0 && images_ready {
            commands.entity(stale.entity).try_despawn();
        } else {
            survivors.push(stale);
        }
    }
    screen_state.stale_roots = survivors;
}

pub(super) fn screen_images_ready(images: &Assets<Image>, handles: &[Handle<Image>]) -> bool {
    handles.iter().all(|handle| images.contains(handle))
}

pub(super) struct SpawnedScreenUi {
    pub(super) root: Entity,
    pub(super) image_handles: Vec<Handle<Image>>,
}

pub(super) fn spawn_screen_ui(
    commands: &mut Commands,
    asset_server: &AssetServer,
    texture_atlases: &TextureAtlasCatalog,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    screen: &ScreenSpec,
) -> SpawnedScreenUi {
    let root = commands
        .spawn((
            ScreenUiRoot,
            ScreenUiNode,
            Pickable::IGNORE,
            UiTransform::IDENTITY,
            GlobalZIndex(SCREEN_ACTIVE_Z),
            screen_root_node(screen),
            screen_root_background(screen),
        ))
        .id();

    let mut image_handles = Vec::new();
    let children = build_screen_ui_children(
        commands,
        root,
        asset_server,
        texture_atlases,
        ui_fonts,
        ui_style,
        screen,
        &mut image_handles,
    );
    commands.entity(root).add_children(&children);

    SpawnedScreenUi {
        root,
        image_handles,
    }
}

fn build_screen_ui_children(
    commands: &mut Commands,
    root: Entity,
    asset_server: &AssetServer,
    texture_atlases: &TextureAtlasCatalog,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    screen: &ScreenSpec,
    image_handles: &mut Vec<Handle<Image>>,
) -> Vec<Entity> {
    let mut top_level = Vec::new();

    if let Some(texture) = screen.background_texture.as_ref() {
        let image = texture_atlases
            .resolve(&texture.path, texture.rect)
            .map(|texture| texture.image.clone())
            .unwrap_or_else(|| asset_server.load(texture.path.clone()));
        image_handles.push(image.clone());
        let background = commands
            .spawn((
                ScreenUiNode,
                Pickable::IGNORE,
                image_node(image, texture_atlases.resolve(&texture.path, texture.rect)),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0.0),
                    right: px(0.0),
                    top: px(0.0),
                    bottom: px(0.0),
                    ..default()
                },
            ))
            .id();
        top_level.push(background);
    }

    if !screen.panel {
        for child in &screen.children {
            let child_entity = spawn_screen_node_entity(
                commands,
                root,
                asset_server,
                texture_atlases,
                ui_fonts,
                ui_style,
                child,
                image_handles,
            );
            top_level.push(child_entity);
        }
        return top_level;
    }

    let panel = commands
        .spawn((
            ScreenUiNode,
            Pickable::IGNORE,
            Node {
                width: screen.width.map(px).unwrap_or(percent(72.0)),
                max_width: percent(92.0),
                padding: UiRect::all(px(screen.padding.max(0.0))),
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(18.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(screen.gap.max(0.0)),
                ..default()
            },
            BackgroundColor(
                screen
                    .background
                    .map(color_from_rgba)
                    .unwrap_or(ui_style.choice_panel_bg),
            ),
            BorderColor::all(
                screen
                    .border
                    .map(color_from_rgba)
                    .unwrap_or(ui_style.choice_button_border),
            ),
        ))
        .id();

    let mut panel_children = Vec::new();
    if let Some(title) = screen.title.as_ref() {
        panel_children.push(
            commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    Text::new(title.clone()),
                    ui_text_font(ui_fonts, 34.0),
                    TextColor(ui_style.speaker_color),
                    default_text_outline(),
                ))
                .id(),
        );
    }
    for child in &screen.children {
        panel_children.push(spawn_screen_node_entity(
            commands,
            root,
            asset_server,
            texture_atlases,
            ui_fonts,
            ui_style,
            child,
            image_handles,
        ));
    }
    commands.entity(panel).add_children(&panel_children);

    top_level.push(panel);
    top_level
}

fn screen_root_node(screen: &ScreenSpec) -> Node {
    Node {
        position_type: PositionType::Absolute,
        width: percent(100.0),
        height: percent(100.0),
        left: px(0.0),
        right: px(0.0),
        top: px(0.0),
        bottom: px(0.0),
        justify_content: justify_from_align(screen.yalign),
        align_items: align_items_from_align(screen.xalign),
        padding: UiRect::all(px(if screen.panel { 24.0 } else { 0.0 })),
        ..default()
    }
}

fn screen_root_background(screen: &ScreenSpec) -> BackgroundColor {
    BackgroundColor(
        screen
            .overlay
            .map(color_from_rgba)
            .unwrap_or(Color::BLACK.with_alpha(0.35)),
    )
}

fn apply_screen_layout(node: &mut Node, layout: &ScreenLayout) {
    if let Some(width) = layout.width {
        node.width = px(width);
    }
    if let Some(width) = layout.width_percent {
        node.width = vw(width);
    }
    if let Some(height) = layout.height {
        node.height = px(height);
    }
    if let Some(height) = layout.height_percent {
        node.height = vh(height);
    }
    if let Some(min_width) = layout.min_width {
        node.min_width = px(min_width);
    }

    if layout.left.is_some()
        || layout.left_percent.is_some()
        || layout.right.is_some()
        || layout.right_percent.is_some()
        || layout.top.is_some()
        || layout.top_percent.is_some()
        || layout.bottom.is_some()
        || layout.bottom_percent.is_some()
    {
        node.position_type = PositionType::Absolute;
    }
    if let Some(left) = layout.left {
        node.left = px(left);
    }
    if let Some(left) = layout.left_percent {
        node.left = vw(left);
    }
    if let Some(right) = layout.right {
        node.right = px(right);
    }
    if let Some(right) = layout.right_percent {
        node.right = vw(right);
    }
    if let Some(top) = layout.top {
        node.top = px(top);
    }
    if let Some(top) = layout.top_percent {
        node.top = vh(top);
    }
    if let Some(bottom) = layout.bottom {
        node.bottom = px(bottom);
    }
    if let Some(bottom) = layout.bottom_percent {
        node.bottom = vh(bottom);
    }
}

fn spawn_screen_node_entity(
    commands: &mut Commands,
    root: Entity,
    asset_server: &AssetServer,
    texture_atlases: &TextureAtlasCatalog,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    node: &ScreenNode,
    image_handles: &mut Vec<Handle<Image>>,
) -> Entity {
    match node {
        ScreenNode::Text(TextNode {
            text,
            binding,
            reactive_text,
            size,
            color,
            align,
            layout,
        }) => {
            let mut node = Node::default();
            apply_screen_layout(&mut node, layout);
            let entity = commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    node,
                    Text::new(text.clone()),
                    ui_text_font(ui_fonts, *size),
                    TextLayout::new(
                        justify_text_from_align(align.unwrap_or(0.0)),
                        LineBreak::AnyCharacter,
                    ),
                    TextColor(color.map(color_from_rgba).unwrap_or(ui_style.line_color)),
                    default_text_outline(),
                ))
                .id();
            if let Some(template) = binding {
                commands.entity(entity).insert(UiTextBinding {
                    template: template.clone(),
                    rendered_revision: u64::MAX,
                });
            }
            if let Some(expression) = reactive_text {
                commands.entity(entity).insert(UiReactiveTextBinding {
                    expression: expression.clone(),
                    rendered_revision: u64::MAX,
                });
            }
            apply_live_layout_bindings(commands, entity, layout);
            entity
        }
        ScreenNode::Button(ButtonNode {
            text,
            value,
            action,
            click_effects,
            enabled,
            enabled_binding,
            reactive_enabled,
            size,
            color,
            hovered_color,
            pressed_color,
            insensitive_color,
            background,
            border,
            hovered_background,
            pressed_background,
            background_texture,
            hovered_background_texture,
            hover_scale,
            press_scale,
            align,
            padding_x,
            padding_y,
            border_width,
            radius,
            layout,
        }) => {
            let normal_texture_resolved = background_texture
                .as_ref()
                .and_then(|texture| texture_atlases.resolve(&texture.path, texture.rect));
            let normal_texture = background_texture.as_ref().map(|texture| {
                normal_texture_resolved
                    .map(|texture| texture.image.clone())
                    .unwrap_or_else(|| asset_server.load(texture.path.clone()))
            });
            let hovered_texture_resolved = hovered_background_texture
                .as_ref()
                .and_then(|texture| texture_atlases.resolve(&texture.path, texture.rect));
            let hovered_texture = hovered_background_texture.as_ref().map(|texture| {
                hovered_texture_resolved
                    .map(|texture| texture.image.clone())
                    .unwrap_or_else(|| asset_server.load(texture.path.clone()))
            });
            image_handles.extend(normal_texture.iter().cloned());
            image_handles.extend(hovered_texture.iter().cloned());
            // A textured button uses its image as the complete visual surface.
            // Keep the Bevy UI background transparent unless the script
            // explicitly layers a color behind it.
            let textured = background_texture.is_some();
            let normal_background = background.map(color_from_rgba).unwrap_or_else(|| {
                if textured {
                    Color::NONE
                } else {
                    ui_style.choice_button_bg
                }
            });
            let hovered_background = hovered_background.map(color_from_rgba).unwrap_or_else(|| {
                if textured {
                    Color::NONE
                } else {
                    ui_style.choice_button_hovered
                }
            });
            let pressed_background = pressed_background.map(color_from_rgba).unwrap_or_else(|| {
                if textured {
                    Color::NONE
                } else {
                    ui_style.choice_button_pressed
                }
            });
            let insensitive_background = normal_background;
            let normal_text_color = color
                .map(color_from_rgba)
                .unwrap_or(ui_style.choice_text_color);
            let hovered_text_color = hovered_color
                .map(color_from_rgba)
                .unwrap_or(normal_text_color);
            let pressed_text_color = pressed_color
                .map(color_from_rgba)
                .unwrap_or(hovered_text_color);
            let insensitive_text_color = insensitive_color
                .map(color_from_rgba)
                .unwrap_or(normal_text_color.with_alpha(0.45));
            let mut button_node = Node {
                width: percent(100.0),
                border: UiRect::all(px(border_width.unwrap_or(1.0).max(0.0))),
                padding: UiRect::axes(
                    px(padding_x.unwrap_or(18.0).max(0.0)),
                    px(padding_y.unwrap_or(14.0).max(0.0)),
                ),
                justify_content: justify_from_align(align.unwrap_or(0.5)),
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(px(radius.unwrap_or(14.0).max(0.0))),
                ..default()
            };
            apply_screen_layout(&mut button_node, layout);
            let initial_background = if *enabled {
                normal_background
            } else {
                insensitive_background
            };
            let initial_text_color = if *enabled {
                normal_text_color
            } else {
                insensitive_text_color
            };
            let text = commands
                .spawn((
                    ScreenUiNode,
                    ScreenUiButtonText,
                    Pickable::IGNORE,
                    Text::new(text.clone()),
                    ui_text_font(ui_fonts, *size),
                    TextColor(initial_text_color),
                    default_text_outline(),
                ))
                .id();
            let button = commands
                .spawn((
                    ScreenUiNode,
                    Button,
                    button_node,
                    UiTransform::IDENTITY,
                    BackgroundColor(initial_background),
                    BorderColor::all(border.map(color_from_rgba).unwrap_or_else(|| {
                        if textured {
                            Color::NONE
                        } else {
                            ui_style.choice_button_border
                        }
                    })),
                ))
                .id();
            if let Some(image) = normal_texture.clone() {
                commands
                    .entity(button)
                    .insert(stretched_image_node(image, normal_texture_resolved));
            }
            commands.entity(button).insert(ScreenUiButton {
                root,
                value: value.clone(),
                click_effects: click_effects.clone(),
                enabled: *enabled,
                text_entity: text,
                normal_background,
                hovered_background,
                pressed_background,
                insensitive_background,
                normal_text_color,
                hovered_text_color,
                pressed_text_color,
                insensitive_text_color,
                hover_scale: *hover_scale,
                press_scale: *press_scale,
                normal_texture: normal_texture.clone(),
                normal_atlas: normal_texture_resolved.map(|texture| texture.atlas.clone()),
                normal_rect: normal_texture_resolved
                    .is_none()
                    .then(|| {
                        background_texture
                            .as_ref()
                            .and_then(|texture| texture.rect)
                            .map(texture_rect)
                    })
                    .flatten(),
                hovered_texture,
                hovered_atlas: hovered_texture_resolved.map(|texture| texture.atlas.clone()),
                hovered_rect: hovered_texture_resolved
                    .is_none()
                    .then(|| {
                        hovered_background_texture
                            .as_ref()
                            .and_then(|texture| texture.rect)
                            .map(texture_rect)
                    })
                    .flatten(),
            });
            if let Some(signal) = enabled_binding {
                commands.entity(button).insert(UiEnabledBinding {
                    signal: signal.clone(),
                    rendered_revision: u64::MAX,
                });
            }
            if let Some(expression) = reactive_enabled {
                commands.entity(button).insert(UiReactiveEnabledBinding {
                    expression: expression.clone(),
                    rendered_revision: u64::MAX,
                });
            }
            if *enabled
                && let Some(action) = action
                    .as_ref()
                    .and_then(|action| parse_ui_action_route(action))
            {
                commands.entity(button).insert(RuntimeMenuButton {
                    action,
                    screen_root: Some(root),
                });
            }
            commands.entity(button).add_child(text);
            apply_live_layout_bindings(commands, button, layout);
            button
        }
        ScreenNode::Image(ScreenImageNode { texture, layout }) => {
            let resolved = texture_atlases.resolve(&texture.path, texture.rect);
            let image = resolved
                .map(|texture| texture.image.clone())
                .unwrap_or_else(|| asset_server.load(texture.path.clone()));
            image_handles.push(image.clone());
            let mut node = Node::default();
            apply_screen_layout(&mut node, layout);
            let entity = commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    image_node(image, resolved),
                    node,
                ))
                .id();
            apply_live_layout_bindings(commands, entity, layout);
            entity
        }
        ScreenNode::ImageButton(ScreenImageButtonNode {
            texture,
            hovered_texture,
            hovered_layout,
            hover_scale,
            press_scale,
            value,
            action,
            click_effects,
            enabled,
            enabled_binding,
            reactive_enabled,
            hovered_when_disabled,
            layout,
        }) => {
            let resolved = texture_atlases.resolve(&texture.path, texture.rect);
            let image = resolved
                .map(|texture| texture.image.clone())
                .unwrap_or_else(|| asset_server.load(texture.path.clone()));
            image_handles.push(image.clone());
            let hovered_resolved = hovered_texture
                .as_ref()
                .and_then(|texture| texture_atlases.resolve(&texture.path, texture.rect));
            let hovered_image = hovered_texture.as_ref().map(|texture| {
                let image = hovered_resolved
                    .map(|texture| texture.image.clone())
                    .unwrap_or_else(|| asset_server.load(texture.path.clone()));
                image_handles.push(image.clone());
                image
            });
            let mut node = Node::default();
            apply_screen_layout(&mut node, layout);
            let normal_node = node.clone();
            let hovered_node = hovered_layout.as_ref().map(|layout| {
                let mut node = Node::default();
                apply_screen_layout(&mut node, layout);
                node
            });
            let normal_rect = resolved
                .is_none()
                .then(|| texture.rect.map(texture_rect))
                .flatten();
            let entity = commands
                .spawn((
                    ScreenUiNode,
                    Button,
                    BackgroundColor(Color::NONE),
                    stretched_image_node(image.clone(), resolved),
                    node,
                    UiTransform::IDENTITY,
                    ScreenUiImageButton {
                        root,
                        value: value.clone(),
                        click_effects: click_effects.clone(),
                        enabled: *enabled,
                        hovered_when_disabled: *hovered_when_disabled,
                        normal_rect,
                        normal_texture: image,
                        normal_atlas: resolved.map(|texture| texture.atlas.clone()),
                        hovered_rect: hovered_resolved
                            .is_none()
                            .then(|| {
                                hovered_texture
                                    .as_ref()
                                    .and_then(|texture| texture.rect)
                                    .map(texture_rect)
                            })
                            .flatten(),
                        hovered_texture: hovered_image,
                        hovered_atlas: hovered_resolved.map(|texture| texture.atlas.clone()),
                        hovered_node,
                        normal_node,
                        hover_scale: *hover_scale,
                        press_scale: *press_scale,
                    },
                ))
                .id();
            if let Some(action) = action.as_deref().and_then(parse_ui_action_route) {
                commands.entity(entity).insert(RuntimeMenuButton {
                    action,
                    screen_root: Some(root),
                });
            }
            if let Some(signal) = enabled_binding {
                commands.entity(entity).insert(UiEnabledBinding {
                    signal: signal.clone(),
                    rendered_revision: u64::MAX,
                });
            }
            if let Some(expression) = reactive_enabled {
                commands.entity(entity).insert(UiReactiveEnabledBinding {
                    expression: expression.clone(),
                    rendered_revision: u64::MAX,
                });
            }
            apply_live_layout_bindings(commands, entity, layout);
            entity
        }
        ScreenNode::Bar(BarNode {
            value,
            binding,
            reactive_value,
            min,
            max,
            width,
            height,
            background,
            fill,
            border,
            layout,
        }) => {
            let span = (*max - *min).max(f32::EPSILON);
            let progress = ((*value - *min) / span).clamp(0.0, 1.0);

            let mut bar_node = Node {
                width: px(*width),
                height: px(*height),
                border: UiRect::all(px(1.0)),
                align_items: AlignItems::Stretch,
                ..default()
            };
            apply_screen_layout(&mut bar_node, layout);
            let bar = commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    bar_node,
                    BackgroundColor(
                        background
                            .map(color_from_rgba)
                            .unwrap_or(Color::BLACK.with_alpha(0.28)),
                    ),
                    BorderColor::all(
                        border
                            .map(color_from_rgba)
                            .unwrap_or(ui_style.choice_button_border),
                    ),
                ))
                .id();
            let fill = commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    Node {
                        width: percent(progress * 100.0),
                        height: percent(100.0),
                        ..default()
                    },
                    BackgroundColor(
                        fill.map(color_from_rgba)
                            .unwrap_or(ui_style.choice_button_pressed),
                    ),
                ))
                .id();
            commands.entity(bar).add_child(fill);
            if let Some(signal) = binding {
                commands.entity(fill).insert(UiProgressBinding {
                    signal: signal.clone(),
                    min: *min,
                    max: *max,
                    rendered_revision: u64::MAX,
                });
            }
            if let Some(expression) = reactive_value {
                commands.entity(fill).insert(UiReactiveProgressBinding {
                    expression: expression.clone(),
                    min: *min,
                    max: *max,
                    rendered_revision: u64::MAX,
                });
            }
            apply_live_layout_bindings(commands, bar, layout);
            bar
        }
        ScreenNode::Row(ContainerNode {
            gap,
            padding,
            background,
            border,
            justify,
            align_items,
            layout,
            children,
        }) => {
            let mut node = Node {
                width: percent(100.0),
                column_gap: px(*gap),
                padding: UiRect::all(px((*padding).max(0.0))),
                border: UiRect::all(px(if border.is_some() { 1.0 } else { 0.0 })),
                justify_content: justify_content_from_option(justify),
                align_items: align_items_from_option(align_items),
                ..default()
            };
            apply_screen_layout(&mut node, layout);
            let container = commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    node,
                    BackgroundColor(
                        background
                            .map(color_from_rgba)
                            .unwrap_or(Color::BLACK.with_alpha(0.0)),
                    ),
                    BorderColor::all(
                        border
                            .map(color_from_rgba)
                            .unwrap_or(Color::BLACK.with_alpha(0.0)),
                    ),
                ))
                .id();
            let children = children
                .iter()
                .map(|child| {
                    spawn_screen_node_entity(
                        commands,
                        root,
                        asset_server,
                        texture_atlases,
                        ui_fonts,
                        ui_style,
                        child,
                        image_handles,
                    )
                })
                .collect::<Vec<_>>();
            commands.entity(container).add_children(&children);
            apply_live_layout_bindings(commands, container, layout);
            container
        }
        ScreenNode::Column(ContainerNode {
            gap,
            padding,
            background,
            border,
            justify,
            align_items,
            layout,
            children,
        }) => {
            let mut node = Node {
                width: percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: px(*gap),
                padding: UiRect::all(px((*padding).max(0.0))),
                border: UiRect::all(px(if border.is_some() { 1.0 } else { 0.0 })),
                justify_content: justify_content_from_option(justify),
                align_items: align_items_from_option(align_items),
                ..default()
            };
            apply_screen_layout(&mut node, layout);
            let container = commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    node,
                    BackgroundColor(
                        background
                            .map(color_from_rgba)
                            .unwrap_or(Color::BLACK.with_alpha(0.0)),
                    ),
                    BorderColor::all(
                        border
                            .map(color_from_rgba)
                            .unwrap_or(Color::BLACK.with_alpha(0.0)),
                    ),
                ))
                .id();
            let children = children
                .iter()
                .map(|child| {
                    spawn_screen_node_entity(
                        commands,
                        root,
                        asset_server,
                        texture_atlases,
                        ui_fonts,
                        ui_style,
                        child,
                        image_handles,
                    )
                })
                .collect::<Vec<_>>();
            commands.entity(container).add_children(&children);
            apply_live_layout_bindings(commands, container, layout);
            container
        }
        ScreenNode::Scrollable(scrollable) => {
            let mut node = Node {
                width: percent(100.0),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::scroll_y(),
                ..default()
            };
            apply_screen_layout(&mut node, &scrollable.layout);
            let container = commands
                .spawn((
                    ScreenUiNode,
                    ScreenUiScrollable {
                        speed: scrollable.speed,
                    },
                    ScrollPosition::default(),
                    node,
                ))
                .id();
            let children = scrollable
                .children
                .iter()
                .map(|child| {
                    spawn_screen_node_entity(
                        commands,
                        root,
                        asset_server,
                        texture_atlases,
                        ui_fonts,
                        ui_style,
                        child,
                        image_handles,
                    )
                })
                .collect::<Vec<_>>();
            commands.entity(container).add_children(&children);
            apply_live_layout_bindings(commands, container, &scrollable.layout);
            container
        }
        ScreenNode::Toggle(toggle) => {
            let unchecked = &toggle.unchecked.texture;
            let checked = &toggle.checked.texture;
            let unchecked_resolved = texture_atlases.resolve(&unchecked.path, unchecked.rect);
            let checked_resolved = texture_atlases.resolve(&checked.path, checked.rect);
            let unchecked_image = unchecked_resolved
                .map(|texture| texture.image.clone())
                .unwrap_or_else(|| asset_server.load(unchecked.path.clone()));
            let checked_image = checked_resolved
                .map(|texture| texture.image.clone())
                .unwrap_or_else(|| asset_server.load(checked.path.clone()));
            image_handles.extend([unchecked_image.clone(), checked_image.clone()]);

            let mut node = Node::default();
            apply_screen_layout(&mut node, &toggle.unchecked.layout);
            let (initial_image, initial_resolved) = if toggle.value {
                (checked_image.clone(), checked_resolved)
            } else {
                (unchecked_image.clone(), unchecked_resolved)
            };
            let entity = commands
                .spawn((
                    ScreenUiNode,
                    Button,
                    BackgroundColor(Color::NONE),
                    stretched_image_node(initial_image, initial_resolved),
                    node,
                    ScreenUiToggle {
                        checked: toggle.value,
                        unchecked_texture: unchecked_image,
                        unchecked_atlas: unchecked_resolved.map(|texture| texture.atlas.clone()),
                        unchecked_rect: unchecked_resolved
                            .is_none()
                            .then(|| unchecked.rect.map(texture_rect))
                            .flatten(),
                        checked_texture: checked_image,
                        checked_atlas: checked_resolved.map(|texture| texture.atlas.clone()),
                        checked_rect: checked_resolved
                            .is_none()
                            .then(|| checked.rect.map(texture_rect))
                            .flatten(),
                    },
                ))
                .id();
            apply_live_layout_bindings(commands, entity, &toggle.unchecked.layout);
            entity
        }
        ScreenNode::Spacer(SpacerNode {
            width,
            height,
            layout,
        }) => {
            let mut node = Node {
                width: px(*width),
                height: px(*height),
                ..default()
            };
            apply_screen_layout(&mut node, layout);
            let entity = commands.spawn((ScreenUiNode, Pickable::IGNORE, node)).id();
            apply_live_layout_bindings(commands, entity, layout);
            entity
        }
    }
}

fn apply_live_layout_bindings(commands: &mut Commands, entity: Entity, layout: &ScreenLayout) {
    commands.entity(entity).insert(if layout.hidden {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    });
    if let Some(signal) = &layout.visible_binding {
        commands.entity(entity).insert(UiVisibilityBinding {
            signal: signal.clone(),
            rendered_revision: u64::MAX,
        });
    }
    if let Some(expression) = &layout.reactive_visibility {
        commands.entity(entity).insert(UiReactiveVisibilityBinding {
            expression: expression.clone(),
            rendered_revision: u64::MAX,
        });
    }
    if let Some(timeline) = &layout.phase_animation {
        commands.entity(entity).insert((
            UiTransform::IDENTITY,
            UiAnimationPlayer {
                spec: timeline.spec,
                elapsed: 0.0,
                phases: Some(timeline.phases.clone()),
                continuous_rotation: timeline.continuous_rotation,
            },
        ));
    } else if let Some(spec) = layout.animation {
        commands.entity(entity).insert((
            UiTransform::IDENTITY,
            UiAnimationPlayer {
                spec,
                elapsed: 0.0,
                phases: None,
                continuous_rotation: false,
            },
        ));
    }
}

/// Advances embedding-owned UI timelines. The HKS VM only constructs the
/// serializable spec; Bevy owns clocks and presentation state.
pub fn animate_screen_ui(
    mut commands: Commands,
    time: Res<Time>,
    mut players: Query<(Entity, &mut UiAnimationPlayer, &mut UiTransform)>,
) {
    for (entity, mut player, mut transform) in &mut players {
        player.elapsed += time.delta_secs();
        let duration = player.spec.duration().max(f32::EPSILON);
        let raw = player.elapsed / duration;
        if let Some(phases) = &player.phases {
            let last_segment = phases.len().saturating_sub(1);
            let (from_index, to_index, local, finished) = if player.continuous_rotation {
                (0, 1, raw.fract(), false)
            } else if player.spec.repeats() {
                let segment = raw.floor() as usize % phases.len();
                (segment, (segment + 1) % phases.len(), raw.fract(), false)
            } else if raw >= last_segment as f32 {
                (last_segment, last_segment, 1.0, true)
            } else {
                let segment = raw.floor() as usize;
                (segment, segment + 1, raw.fract(), false)
            };
            let local = player.spec.sample(local);
            let (from_rotation, from_scale, from_x, from_y) = phases[from_index].values();
            let (to_rotation, to_scale, to_x, to_y) = phases[to_index].values();
            transform.rotation = Rot2::degrees(from_rotation.lerp(to_rotation, local));
            transform.scale = Vec2::splat(from_scale.lerp(to_scale, local));
            transform.translation =
                Val2::new(px(from_x.lerp(to_x, local)), px(from_y.lerp(to_y, local)));
            if finished {
                commands.entity(entity).try_remove::<UiAnimationPlayer>();
            }
            continue;
        }
        let progress = if player.spec.repeats() {
            let cycle = raw.rem_euclid(2.0);
            if cycle <= 1.0 { cycle } else { 2.0 - cycle }
        } else {
            raw.min(1.0)
        };
        let progress = player.spec.sample(progress);
        let scale = 0.96 + progress * 0.04;
        transform.scale = Vec2::splat(scale);
        if !player.spec.repeats() && raw >= 1.0 {
            transform.scale = Vec2::ONE;
            commands.entity(entity).try_remove::<UiAnimationPlayer>();
        }
    }
}

fn image_node(image: Handle<Image>, atlas: Option<&crate::texture::AtlasTexture>) -> ImageNode {
    if let Some(atlas) = atlas {
        ImageNode::from_atlas_image(image, atlas.atlas.clone())
    } else {
        ImageNode::new(image)
    }
}

fn stretched_image_node(
    image: Handle<Image>,
    atlas: Option<&crate::texture::AtlasTexture>,
) -> ImageNode {
    let mut node = image_node(image, atlas).with_mode(NodeImageMode::Stretch);
    node.visual_box = VisualBox::BorderBox;
    node
}

fn texture_rect(rect: [f32; 4]) -> Rect {
    Rect::from_corners(
        Vec2::new(rect[0], rect[1]),
        Vec2::new(rect[0] + rect[2], rect[1] + rect[3]),
    )
}

fn justify_from_align(value: f32) -> JustifyContent {
    if value <= 0.25 {
        JustifyContent::FlexStart
    } else if value >= 0.75 {
        JustifyContent::FlexEnd
    } else {
        JustifyContent::Center
    }
}

fn justify_text_from_align(value: f32) -> Justify {
    if value <= 0.25 {
        Justify::Left
    } else if value >= 0.75 {
        Justify::Right
    } else {
        Justify::Center
    }
}

/// Default contrast treatment for every screen-UI glyph, including text
/// nodes, terms and button labels. Bevy renders this in the same text pass,
/// avoiding extra UI entities and keeping reactive text updates atomic.
fn default_text_outline() -> TextShadow {
    TextShadow {
        offset: Vec2::splat(1.5),
        color: Color::srgba(0.0, 0.0, 0.0, 0.92),
    }
}

fn justify_content_from_option(value: &Option<String>) -> JustifyContent {
    match value.as_deref() {
        Some("start") | Some("left") | Some("top") => JustifyContent::FlexStart,
        Some("end") | Some("right") | Some("bottom") => JustifyContent::FlexEnd,
        Some("center") => JustifyContent::Center,
        Some("between") => JustifyContent::SpaceBetween,
        Some("around") => JustifyContent::SpaceAround,
        Some("evenly") => JustifyContent::SpaceEvenly,
        _ => JustifyContent::Default,
    }
}

fn align_items_from_option(value: &Option<String>) -> AlignItems {
    match value.as_deref() {
        Some("start") | Some("left") | Some("top") => AlignItems::FlexStart,
        Some("end") | Some("right") | Some("bottom") => AlignItems::FlexEnd,
        Some("center") => AlignItems::Center,
        Some("stretch") => AlignItems::Stretch,
        _ => AlignItems::Default,
    }
}
pub fn handle_screen_buttons(
    mut screen_state: ResMut<ScreenUiState>,
    mut effects: MessageWriter<UiEffectMessage>,
    mut responses: MessageWriter<ScriptResponseMessage>,
    mut clicks: MessageReader<Pointer<Click>>,
    mut interaction_query: Query<
        (
            &PickingInteraction,
            &mut BackgroundColor,
            &mut UiTransform,
            Option<&mut ImageNode>,
            &ScreenUiButton,
        ),
        Changed<PickingInteraction>,
    >,
    button_query: Query<&ScreenUiButton>,
    mut text_query: Query<&mut TextColor, With<ScreenUiButtonText>>,
) {
    for (interaction, mut color, mut transform, image, button) in &mut interaction_query {
        if Some(button.root) != screen_state.active_root {
            continue;
        }

        if !button.enabled {
            transform.scale = Vec2::ONE;
            *color = button.insensitive_background.into();
            if let Ok(mut text_color) = text_query.get_mut(button.text_entity) {
                *text_color = button.insensitive_text_color.into();
            }
            apply_screen_button_image(image, button, false);
            continue;
        }

        match *interaction {
            PickingInteraction::Pressed => {
                transform.scale = Vec2::splat(button.press_scale);
                *color = button.pressed_background.into();
                if let Ok(mut text_color) = text_query.get_mut(button.text_entity) {
                    *text_color = button.pressed_text_color.into();
                }
                apply_screen_button_image(image, button, true);
            }
            PickingInteraction::Hovered => {
                transform.scale = Vec2::splat(button.hover_scale);
                *color = button.hovered_background.into();
                if let Ok(mut text_color) = text_query.get_mut(button.text_entity) {
                    *text_color = button.hovered_text_color.into();
                }
                apply_screen_button_image(image, button, true);
            }
            PickingInteraction::None => {
                transform.scale = Vec2::ONE;
                *color = button.normal_background.into();
                if let Ok(mut text_color) = text_query.get_mut(button.text_entity) {
                    *text_color = button.normal_text_color.into();
                }
                apply_screen_button_image(image, button, false);
            }
        }
    }

    for click in clicks.read() {
        if click.button != PointerButton::Primary {
            continue;
        }
        let Ok(button) = button_query.get(click.entity) else {
            continue;
        };
        if Some(button.root) != screen_state.active_root || !button.enabled {
            continue;
        }
        let Some(value) = button.value.clone() else {
            emit_ui_effects(&mut effects, &button.click_effects);
            continue;
        };
        let Some(done) = screen_state.waiting.take() else {
            continue;
        };
        emit_ui_effects(&mut effects, &button.click_effects);
        responses.write(ScriptResponseMessage {
            request: done,
            response: ScriptResponse::Choice(value),
        });
    }
}

fn emit_ui_effects(writer: &mut MessageWriter<UiEffectMessage>, effects: &[UiEffect]) {
    for effect in effects.iter().cloned() {
        writer.write(UiEffectMessage(effect));
    }
}

pub fn process_ui_effects(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    audio: Res<AudioCatalog>,
    user_settings: Res<UserSettings>,
    mut effects: MessageReader<UiEffectMessage>,
) {
    for UiEffectMessage(effect) in effects.read() {
        match effect {
            UiEffect::PlaySfx { name, volume } => {
                let Some(definition) = audio.resolve_sfx(name) else {
                    warn!("UI sound effect `{name}` is not defined");
                    continue;
                };
                let playback_volume = apply_volume_setting(*volume, user_settings.sfx_volume);
                commands.spawn((
                    SfxChannel { volume: *volume },
                    AudioPlayer::new(asset_server.load(definition.path.clone())),
                    PlaybackSettings::DESPAWN.with_volume(Volume::Linear(playback_volume)),
                ));
            }
        }
    }
}

fn apply_screen_button_image(
    mut image: Option<Mut<ImageNode>>,
    button: &ScreenUiButton,
    hovered: bool,
) {
    let Some(image) = image.as_deref_mut() else {
        return;
    };
    if hovered && let Some(texture) = button.hovered_texture.as_ref() {
        image.image = texture.clone();
        image.texture_atlas = button.hovered_atlas.clone();
        image.rect = button.hovered_rect;
    } else if let Some(texture) = button.normal_texture.as_ref() {
        image.image = texture.clone();
        image.texture_atlas = button.normal_atlas.clone();
        image.rect = button.normal_rect;
    }
}

pub fn handle_screen_image_buttons(
    mut screen_state: ResMut<ScreenUiState>,
    mut effects: MessageWriter<UiEffectMessage>,
    mut responses: MessageWriter<ScriptResponseMessage>,
    mut clicks: MessageReader<Pointer<Click>>,
    mut interaction_query: Query<
        (
            &PickingInteraction,
            &mut ImageNode,
            &mut Node,
            &mut UiTransform,
            &ScreenUiImageButton,
        ),
        Changed<PickingInteraction>,
    >,
    button_query: Query<&ScreenUiImageButton>,
) {
    for (interaction, mut image, mut node, mut transform, button) in &mut interaction_query {
        if Some(button.root) != screen_state.active_root && button.value.is_some() {
            continue;
        }

        match *interaction {
            PickingInteraction::Pressed if button.enabled => {
                transform.scale = Vec2::splat(button.press_scale);
                image.image = button
                    .hovered_texture
                    .clone()
                    .unwrap_or_else(|| button.normal_texture.clone());
                image.texture_atlas = button
                    .hovered_atlas
                    .clone()
                    .or_else(|| button.normal_atlas.clone());
                image.rect = button.hovered_rect.or(button.normal_rect);
                *node = button
                    .hovered_node
                    .clone()
                    .unwrap_or_else(|| button.normal_node.clone());
            }
            PickingInteraction::Hovered if button.enabled || button.hovered_when_disabled => {
                transform.scale = Vec2::splat(button.hover_scale);
                image.image = button
                    .hovered_texture
                    .clone()
                    .unwrap_or_else(|| button.normal_texture.clone());
                image.texture_atlas = button
                    .hovered_atlas
                    .clone()
                    .or_else(|| button.normal_atlas.clone());
                image.rect = button.hovered_rect.or(button.normal_rect);
                *node = button
                    .hovered_node
                    .clone()
                    .unwrap_or_else(|| button.normal_node.clone());
            }
            PickingInteraction::None => {
                transform.scale = Vec2::ONE;
                image.image = button.normal_texture.clone();
                image.texture_atlas = button.normal_atlas.clone();
                image.rect = button.normal_rect;
                *node = button.normal_node.clone();
            }
            _ => {
                transform.scale = Vec2::ONE;
                image.image = button.normal_texture.clone();
                image.texture_atlas = button.normal_atlas.clone();
                image.rect = button.normal_rect;
                *node = button.normal_node.clone();
            }
        }
    }

    for click in clicks.read() {
        if click.button != PointerButton::Primary {
            continue;
        }
        let Ok(button) = button_query.get(click.entity) else {
            continue;
        };
        if (Some(button.root) != screen_state.active_root && button.value.is_some())
            || !button.enabled
        {
            continue;
        }
        let Some(value) = button.value.clone() else {
            emit_ui_effects(&mut effects, &button.click_effects);
            continue;
        };
        let Some(done) = screen_state.waiting.take() else {
            continue;
        };
        emit_ui_effects(&mut effects, &button.click_effects);
        responses.write(ScriptResponseMessage {
            request: done,
            response: ScriptResponse::Choice(value),
        });
    }
}

pub fn handle_screen_scroll(
    mut scrolls: MessageReader<Pointer<Scroll>>,
    mut scrollables: Query<(&ScreenUiScrollable, &mut ScrollPosition)>,
    parents: Query<&ChildOf>,
) {
    for scroll in scrolls.read() {
        let mut entity = scroll.entity;
        loop {
            if let Ok((scrollable, mut position)) = scrollables.get_mut(entity) {
                let unit = match scroll.unit {
                    MouseScrollUnit::Line => scrollable.speed,
                    MouseScrollUnit::Pixel => 1.0,
                };
                position.x = (position.x - scroll.x * unit).max(0.0);
                position.y = (position.y - scroll.y * unit).max(0.0);
                break;
            }
            let Ok(parent) = parents.get(entity) else {
                break;
            };
            entity = parent.parent();
        }
    }
}

pub fn handle_screen_toggles(
    mut clicks: MessageReader<Pointer<Click>>,
    mut toggles: Query<(&mut ScreenUiToggle, &mut ImageNode)>,
    parents: Query<&ChildOf>,
) {
    for click in clicks.read() {
        if click.button != PointerButton::Primary {
            continue;
        }
        let mut entity = click.entity;
        loop {
            if let Ok((mut toggle, mut image)) = toggles.get_mut(entity) {
                toggle.checked = !toggle.checked;
                if toggle.checked {
                    image.image = toggle.checked_texture.clone();
                    image.texture_atlas = toggle.checked_atlas.clone();
                    image.rect = toggle.checked_rect;
                } else {
                    image.image = toggle.unchecked_texture.clone();
                    image.texture_atlas = toggle.unchecked_atlas.clone();
                    image.rect = toggle.unchecked_rect;
                }
                break;
            }
            let Ok(parent) = parents.get(entity) else {
                break;
            };
            entity = parent.parent();
        }
    }
}

pub fn update_builtin_ui_models(
    time: Res<Time>,
    scene: Res<SceneSharedState>,
    dialogue_state: Res<DialogueState>,
    dialogue_history: Res<DialogueHistoryState>,
    mut models: ResMut<UiModels>,
    mut last_second: Local<Option<u64>>,
) {
    let elapsed = time.elapsed_secs_f64().floor().max(0.0) as u64;
    if *last_second != Some(elapsed) {
        *last_second = Some(elapsed);
        let unix = time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64;
        models.set(
            "time",
            StoredValue::Map(BTreeMap::from([
                (
                    "elapsedSeconds".to_string(),
                    StoredValue::Int(elapsed as i64),
                ),
                ("unixSeconds".to_string(), StoredValue::Int(unix as i64)),
            ])),
        );
    }

    let dialogue = scene.0.dialogue.as_ref();
    let text = dialogue
        .map(|value| value.text.as_str())
        .unwrap_or_default();
    let revealed = dialogue_state
        .reveal
        .as_ref()
        .map(|reveal| reveal.next_index)
        .unwrap_or_else(|| text.chars().count());
    models.set(
        "dialogue",
        StoredValue::Map(BTreeMap::from([
            (
                "speaker".to_string(),
                StoredValue::String(
                    dialogue
                        .map(|value| value.speaker.clone())
                        .unwrap_or_default(),
                ),
            ),
            ("text".to_string(), StoredValue::String(text.to_string())),
            ("visible".to_string(), StoredValue::Bool(dialogue.is_some())),
            (
                "revealedCharacters".to_string(),
                StoredValue::Int(revealed as i64),
            ),
            (
                "canAdvance".to_string(),
                StoredValue::Bool(dialogue_state.waiting.is_some()),
            ),
        ])),
    );

    let entries = dialogue_history
        .entries
        .iter()
        .map(|entry| {
            StoredValue::Map(BTreeMap::from([
                (
                    "speaker".to_string(),
                    StoredValue::String(entry.speaker.clone()),
                ),
                ("text".to_string(), StoredValue::String(entry.text.clone())),
            ]))
        })
        .collect::<Vec<_>>();
    let text = dialogue_history
        .entries
        .iter()
        .map(|entry| {
            if entry.speaker.is_empty() {
                entry.text.clone()
            } else {
                format!("{}\n{}", entry.speaker, entry.text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    models.set(
        "history",
        StoredValue::Map(BTreeMap::from([
            ("entries".to_string(), StoredValue::Array(entries)),
            ("text".to_string(), StoredValue::String(text)),
            (
                "visible".to_string(),
                StoredValue::Bool(dialogue_history.visible),
            ),
        ])),
    );
}

pub fn update_ui_text_bindings(
    models: Res<UiModels>,
    mut text_bindings: Query<(&mut UiTextBinding, &mut Text)>,
    mut visibility_bindings: Query<(&mut UiVisibilityBinding, &mut Visibility)>,
    mut button_bindings: Query<
        (
            &mut UiEnabledBinding,
            &mut ScreenUiButton,
            &mut BackgroundColor,
        ),
        Without<ScreenUiImageButton>,
    >,
    mut image_button_bindings: Query<
        (&mut UiEnabledBinding, &mut ScreenUiImageButton),
        Without<ScreenUiButton>,
    >,
    mut progress_bindings: Query<(&mut UiProgressBinding, &mut Node)>,
    mut button_texts: Query<&mut TextColor, With<ScreenUiButtonText>>,
) {
    let revision = models.revision();
    for (mut binding, mut text) in &mut text_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        let rendered = expand_model_template(&binding.template, &models);
        if text.0 != rendered {
            text.0 = rendered;
        }
        binding.rendered_revision = revision;
    }
    for (mut binding, mut visibility) in &mut visibility_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        if let Some(visible) = models.get(&binding.signal).and_then(StoredValue::as_bool) {
            *visibility = if visible {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
        binding.rendered_revision = revision;
    }
    for (mut binding, mut button, mut background) in &mut button_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        if let Some(enabled) = models.get(&binding.signal).and_then(StoredValue::as_bool) {
            button.enabled = enabled;
            *background = if enabled {
                button.normal_background
            } else {
                button.insensitive_background
            }
            .into();
            if let Ok(mut color) = button_texts.get_mut(button.text_entity) {
                *color = if enabled {
                    button.normal_text_color
                } else {
                    button.insensitive_text_color
                }
                .into();
            }
        }
        binding.rendered_revision = revision;
    }
    for (mut binding, mut button) in &mut image_button_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        if let Some(enabled) = models.get(&binding.signal).and_then(StoredValue::as_bool) {
            button.enabled = enabled;
        }
        binding.rendered_revision = revision;
    }
    for (mut binding, mut node) in &mut progress_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        if let Some(value) = models.get(&binding.signal).and_then(StoredValue::as_number) {
            let span = (binding.max - binding.min).max(f32::EPSILON);
            let progress = ((value as f32 - binding.min) / span).clamp(0.0, 1.0);
            node.width = percent(progress * 100.0);
        }
        binding.rendered_revision = revision;
    }
}

pub fn update_ui_reactive_bindings(
    models: Res<UiModels>,
    mut text_bindings: Query<(&mut UiReactiveTextBinding, &mut Text)>,
    mut visibility_bindings: Query<(&mut UiReactiveVisibilityBinding, &mut Visibility)>,
    mut button_bindings: Query<
        (
            &mut UiReactiveEnabledBinding,
            &mut ScreenUiButton,
            &mut BackgroundColor,
        ),
        Without<ScreenUiImageButton>,
    >,
    mut image_button_bindings: Query<
        (&mut UiReactiveEnabledBinding, &mut ScreenUiImageButton),
        Without<ScreenUiButton>,
    >,
    mut progress_bindings: Query<(&mut UiReactiveProgressBinding, &mut Node)>,
    mut button_texts: Query<&mut TextColor, With<ScreenUiButtonText>>,
) {
    let revision = models.revision();
    for (mut binding, mut text) in &mut text_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        match crate::script::evaluate_ui_reactive_binding(&binding.expression, &models) {
            Ok(hiraku_script::Value::String(value)) => text.0 = value,
            Ok(value) => warn!("reactive UI text returned {value:?}, expected String"),
            Err(error) => warn!("reactive UI text failed: {error}"),
        }
        binding.rendered_revision = revision;
    }
    for (mut binding, mut visibility) in &mut visibility_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        match crate::script::evaluate_ui_reactive_binding(&binding.expression, &models) {
            Ok(hiraku_script::Value::Bool(visible)) => {
                *visibility = if visible {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
            }
            Ok(value) => warn!("reactive UI visibility returned {value:?}, expected Bool"),
            Err(error) => warn!("reactive UI visibility failed: {error}"),
        }
        binding.rendered_revision = revision;
    }
    for (mut binding, mut button, mut background) in &mut button_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        match crate::script::evaluate_ui_reactive_binding(&binding.expression, &models) {
            Ok(hiraku_script::Value::Bool(enabled)) => {
                button.enabled = enabled;
                *background = if enabled {
                    button.normal_background
                } else {
                    button.insensitive_background
                }
                .into();
                if let Ok(mut color) = button_texts.get_mut(button.text_entity) {
                    *color = if enabled {
                        button.normal_text_color
                    } else {
                        button.insensitive_text_color
                    }
                    .into();
                }
            }
            Ok(value) => warn!("reactive UI enabled expression returned {value:?}, expected Bool"),
            Err(error) => warn!("reactive UI enabled expression failed: {error}"),
        }
        binding.rendered_revision = revision;
    }
    for (mut binding, mut button) in &mut image_button_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        match crate::script::evaluate_ui_reactive_binding(&binding.expression, &models) {
            Ok(hiraku_script::Value::Bool(enabled)) => button.enabled = enabled,
            Ok(value) => warn!("reactive UI enabled expression returned {value:?}, expected Bool"),
            Err(error) => warn!("reactive UI enabled expression failed: {error}"),
        }
        binding.rendered_revision = revision;
    }
    for (mut binding, mut node) in &mut progress_bindings {
        if binding.rendered_revision == revision {
            continue;
        }
        match crate::script::evaluate_ui_reactive_binding(&binding.expression, &models) {
            Ok(hiraku_script::Value::Number(value)) => {
                let span = (binding.max - binding.min).max(f32::EPSILON);
                let progress = ((value as f32 - binding.min) / span).clamp(0.0, 1.0);
                node.width = percent(progress * 100.0);
            }
            Ok(value) => warn!("reactive UI progress returned {value:?}, expected Float"),
            Err(error) => warn!("reactive UI progress failed: {error}"),
        }
        binding.rendered_revision = revision;
    }
}

fn expand_model_template(template: &str, models: &UiModels) -> String {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let key = &after[..end];
        if let Some(value) = models.get(key) {
            output.push_str(&value.display());
        } else {
            output.push_str("${");
            output.push_str(key);
            output.push('}');
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    output
}

pub(super) fn should_clear_stale_screen_before_command(command: &ScriptCommand) -> bool {
    matches!(
        command,
        ScriptCommand::Say { .. }
            | ScriptCommand::AwaitDialogueAdvance { .. }
            | ScriptCommand::SetDialogue { .. }
            | ScriptCommand::Choose { .. }
            | ScriptCommand::ShowSprite { .. }
            | ScriptCommand::HideSprite { .. }
            | ScriptCommand::ShowCharacter { .. }
            | ScriptCommand::HideCharacter { .. }
            | ScriptCommand::JumpCharacter { .. }
            | ScriptCommand::ShakeCharacter { .. }
            | ScriptCommand::AnimateCharacter { .. }
            | ScriptCommand::MoveSprite { .. }
            | ScriptCommand::ScaleSprite { .. }
            | ScriptCommand::FadeSprite { .. }
            | ScriptCommand::RuleTransitionBg { .. }
            | ScriptCommand::PlayCustomEffect { .. }
            | ScriptCommand::RestoreSnapshot { .. }
            | ScriptCommand::Exit
            | ScriptCommand::ReturnToTitle
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use crate::scene::{
        command_runtime::resolve_ui_component_path, runtime_menu::RuntimeMenuButtonAction,
    };

    use bevy::{
        camera::NormalizedRenderTarget,
        picking::{
            backend::HitData,
            pointer::{Location, PointerButton, PointerId},
        },
    };

    use super::*;

    #[test]
    fn ui_action_routes_are_namespace_qualified_and_structured() {
        assert!(matches!(
            parse_ui_action_route("ui.open.dialogue"),
            Some(RuntimeMenuButtonAction::OpenUi(role)) if role == "dialogue"
        ));
        assert!(matches!(
            parse_ui_action_route("storage.save.slot1"),
            Some(RuntimeMenuButtonAction::Save(slot)) if slot == "slot1"
        ));
        assert!(matches!(
            parse_ui_action_route("story.next"),
            Some(RuntimeMenuButtonAction::AdvanceDialogue)
        ));
    }

    #[test]
    fn package_ui_paths_do_not_become_relative_to_the_restored_story() {
        let vfs = VfsResource(Arc::new(crate::vfs::HdpVfs::new("assets")));
        assert_eq!(
            resolve_ui_component_path(
                &vfs,
                Some("hdp://example.hdp/scripts/chapter.hks"),
                "ui/history.ui.hks",
            ),
            "hdp://example.hdp/ui/history.ui.hks"
        );
    }

    #[test]
    fn model_templates_update_known_values_without_erasing_unknown_values() {
        let mut models = UiModels::default();
        models.set(
            "time",
            StoredValue::Map(BTreeMap::from([(
                "elapsedSeconds".to_string(),
                StoredValue::Int(12),
            )])),
        );
        assert_eq!(
            expand_model_template(
                "elapsed=${time.elapsedSeconds}, custom=${game.weather}",
                &models,
            ),
            "elapsed=12, custom=${game.weather}",
        );
    }

    #[test]
    fn screen_button_activates_on_click_not_press() {
        let mut app = App::new();
        app.init_resource::<ScreenUiState>()
            .add_message::<Pointer<Click>>()
            .add_message::<ScriptResponseMessage>()
            .add_message::<UiEffectMessage>()
            .add_systems(Update, handle_screen_buttons);
        let root = app.world_mut().spawn_empty().id();
        let text = app.world_mut().spawn(TextColor(Color::WHITE)).id();
        let button = app
            .world_mut()
            .spawn((
                PickingInteraction::Pressed,
                BackgroundColor(Color::BLACK),
                UiTransform::IDENTITY,
                ScreenUiButton {
                    root,
                    value: Some(StoredValue::String("continue".into())),
                    click_effects: Vec::new(),
                    enabled: true,
                    text_entity: text,
                    normal_background: Color::BLACK,
                    hovered_background: Color::BLACK,
                    pressed_background: Color::BLACK,
                    insensitive_background: Color::BLACK,
                    normal_text_color: Color::WHITE,
                    hovered_text_color: Color::WHITE,
                    pressed_text_color: Color::WHITE,
                    insensitive_text_color: Color::WHITE,
                    hover_scale: 1.0,
                    press_scale: 1.0,
                    normal_texture: None,
                    normal_atlas: None,
                    normal_rect: None,
                    hovered_texture: None,
                    hovered_atlas: None,
                    hovered_rect: None,
                },
            ))
            .id();
        {
            let mut state = app.world_mut().resource_mut::<ScreenUiState>();
            state.active_root = Some(root);
            state.waiting = Some(ScriptRequestId(7));
        }

        app.update();
        assert_eq!(
            app.world().resource::<ScreenUiState>().waiting,
            Some(ScriptRequestId(7)),
            "pressing must not activate a button",
        );

        app.world_mut().write_message(Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 1,
                    height: 1,
                },
                position: Vec2::ZERO,
            },
            Click {
                button: PointerButton::Primary,
                hit: HitData {
                    camera: root,
                    depth: 0.0,
                    position: None,
                    normal: None,
                    extra: None,
                },
                duration: Duration::ZERO,
                count: 1,
            },
            button,
        ));
        app.update();

        assert_eq!(app.world().resource::<ScreenUiState>().waiting, None);
    }

    #[test]
    fn closing_a_modal_consumes_the_same_frame_dialogue_action() {
        let mut app = App::new();
        app.init_resource::<DialogueState>()
            .init_resource::<AnimationState>()
            .init_resource::<ChoiceState>()
            .init_resource::<RuntimeMenuState>()
            .init_resource::<DialogueHistoryState>()
            .add_message::<crate::input::HirakuActionInput>()
            .add_message::<Pointer<Click>>()
            .add_message::<ScriptResponseMessage>()
            .add_systems(Update, advance_dialogue_on_input);
        app.world_mut().resource_mut::<DialogueState>().waiting = Some(PendingDialogueAdvance {
            animation_id: None,
            request: Some(ScriptRequestId(11)),
        });
        let modal = app.world_mut().spawn(PauseMenuRoot).id();
        let label = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(modal).add_child(label);
        app.world_mut().write_message(Pointer::new(
            PointerId::Custom(uuid::Uuid::from_u128(1)),
            Location {
                target: NormalizedRenderTarget::None {
                    width: 1,
                    height: 1,
                },
                position: Vec2::ZERO,
            },
            Click {
                button: PointerButton::Primary,
                hit: HitData {
                    camera: modal,
                    depth: 0.0,
                    position: None,
                    normal: None,
                    extra: None,
                },
                duration: Duration::ZERO,
                count: 1,
            },
            label,
        ));

        app.update();

        assert!(app.world().resource::<DialogueState>().waiting.is_some());
    }

    #[test]
    fn claimed_pointer_click_survives_modal_despawn_before_dialogue_input() {
        let mut app = App::new();
        app.init_resource::<DialogueState>()
            .init_resource::<AnimationState>()
            .init_resource::<ChoiceState>()
            .init_resource::<RuntimeMenuState>()
            .init_resource::<DialogueHistoryState>()
            .add_message::<crate::input::HirakuActionInput>()
            .add_message::<Pointer<Click>>()
            .add_message::<ScriptResponseMessage>()
            .add_systems(Update, advance_dialogue_on_input);
        app.world_mut().resource_mut::<DialogueState>().waiting = Some(PendingDialogueAdvance {
            animation_id: None,
            request: Some(ScriptRequestId(12)),
        });
        let pointer = PointerId::Custom(uuid::Uuid::from_u128(2));
        app.world_mut()
            .resource_mut::<RuntimeMenuState>()
            .consumed_pointer_clicks
            .insert(pointer, 2);
        let former_button = app.world_mut().spawn_empty().id();
        for _ in 0..2 {
            app.world_mut().write_message(Pointer::new(
                pointer,
                Location {
                    target: NormalizedRenderTarget::None {
                        width: 1,
                        height: 1,
                    },
                    position: Vec2::ZERO,
                },
                Click {
                    button: PointerButton::Primary,
                    hit: HitData {
                        camera: former_button,
                        depth: 0.0,
                        position: None,
                        normal: None,
                        extra: None,
                    },
                    duration: Duration::ZERO,
                    count: 1,
                },
                former_button,
            ));
        }

        app.update();

        assert!(app.world().resource::<DialogueState>().waiting.is_some());
        assert!(
            app.world()
                .resource::<RuntimeMenuState>()
                .consumed_pointer_clicks
                .is_empty()
        );
    }

    #[test]
    fn typed_binding_system_updates_components_without_rebuilding_entities() {
        let mut app = App::new();
        app.init_resource::<UiModels>()
            .add_systems(Update, update_ui_text_bindings);
        let text = app
            .world_mut()
            .spawn((
                Text::new("pending"),
                UiTextBinding {
                    template: "HP ${player.health}".into(),
                    rendered_revision: u64::MAX,
                },
            ))
            .id();
        let visible = app
            .world_mut()
            .spawn((
                Visibility::Inherited,
                UiVisibilityBinding {
                    signal: "hud.visible".into(),
                    rendered_revision: u64::MAX,
                },
            ))
            .id();
        let progress = app
            .world_mut()
            .spawn((
                Node::default(),
                UiProgressBinding {
                    signal: "player.health".into(),
                    min: 0.0,
                    max: 100.0,
                    rendered_revision: u64::MAX,
                },
            ))
            .id();
        {
            let mut models = app.world_mut().resource_mut::<UiModels>();
            models.set(
                "player",
                StoredValue::Map(BTreeMap::from([(
                    "health".to_string(),
                    StoredValue::Int(25),
                )])),
            );
            models.set(
                "hud",
                StoredValue::Map(BTreeMap::from([(
                    "visible".to_string(),
                    StoredValue::Bool(false),
                )])),
            );
        }

        app.update();

        assert_eq!(
            app.world().get::<Text>(text).expect("text exists").0,
            "HP 25"
        );
        assert_eq!(
            app.world().get::<Visibility>(visible),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world()
                .get::<Node>(progress)
                .expect("progress exists")
                .width,
            percent(25.0)
        );
    }

    #[test]
    fn spin_animates_ui_rotation_without_rebuilding_the_node() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_systems(Update, animate_screen_ui);
        let node = app
            .world_mut()
            .spawn((
                UiTransform::IDENTITY,
                UiAnimationPlayer {
                    spec: crate::script::AnimationSpec::Linear(1.0, true),
                    elapsed: 0.0,
                    phases: Some(vec![
                        crate::script::AnimationPhase::Transform(0.0, 1.0, 0.0, 0.0),
                        crate::script::AnimationPhase::Transform(360.0, 1.0, 0.0, 0.0),
                    ]),
                    continuous_rotation: true,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(250));

        app.update();

        let transform = app
            .world()
            .get::<UiTransform>(node)
            .expect("animated UI node remains alive");
        assert!(transform.rotation.cos.abs() < 0.0001);
        assert!((transform.rotation.sin - 1.0).abs() < 0.0001);
    }
}
