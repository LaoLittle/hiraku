use super::*;

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
                .insert((Visibility::Inherited, GlobalZIndex(SCREEN_ACTIVE_Z)));
            if let Some(previous) = pending.previous {
                commands
                    .entity(previous)
                    .insert(GlobalZIndex(SCREEN_STALE_Z));
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
                    .insert((Visibility::Inherited, GlobalZIndex(SCREEN_PENDING_Z)));
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
            size,
            color,
            align,
            layout,
        }) => {
            let mut node = Node::default();
            apply_screen_layout(&mut node, layout);
            commands
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
                ))
                .id()
        }
        ScreenNode::Button(ButtonNode {
            text,
            value,
            action,
            enabled,
            size,
            color,
            hovered_color,
            pressed_color,
            insensitive_color,
            background,
            border,
            hovered_background,
            pressed_background,
            align,
            padding_x,
            padding_y,
            border_width,
            radius,
            layout,
        }) => {
            let normal_background = background
                .map(color_from_rgba)
                .unwrap_or(ui_style.choice_button_bg);
            let hovered_background = hovered_background
                .map(color_from_rgba)
                .unwrap_or(ui_style.choice_button_hovered);
            let pressed_background = pressed_background
                .map(color_from_rgba)
                .unwrap_or(ui_style.choice_button_pressed);
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
                ))
                .id();
            let button = commands
                .spawn((
                    ScreenUiNode,
                    Button,
                    button_node,
                    BackgroundColor(initial_background),
                    BorderColor::all(
                        border
                            .map(color_from_rgba)
                            .unwrap_or(ui_style.choice_button_border),
                    ),
                ))
                .id();
            if let Some(value) = value.as_ref() {
                commands.entity(button).insert(ScreenUiButton {
                    root,
                    value: value.clone(),
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
                });
            }
            if *enabled
                && let Some(action) = action
                    .as_ref()
                    .and_then(|action| runtime_menu_action_from_str(action))
            {
                commands.entity(button).insert(RuntimeMenuButton { action });
            }
            commands.entity(button).add_child(text);
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
            commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    image_node(image, resolved),
                    node,
                ))
                .id()
        }
        ScreenNode::ImageButton(ScreenImageButtonNode {
            texture,
            hovered_texture,
            hovered_layout,
            value,
            enabled,
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
            commands
                .spawn((
                    ScreenUiNode,
                    Button,
                    image_node(image.clone(), resolved),
                    node,
                    ScreenUiImageButton {
                        root,
                        value: value.clone(),
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
                    },
                ))
                .id()
        }
        ScreenNode::Bar(BarNode {
            value,
            min,
            max,
            width,
            height,
            background,
            fill,
            border,
        }) => {
            let span = (*max - *min).max(f32::EPSILON);
            let progress = ((*value - *min) / span).clamp(0.0, 1.0);

            let bar = commands
                .spawn((
                    ScreenUiNode,
                    Pickable::IGNORE,
                    Node {
                        width: px(*width),
                        height: px(*height),
                        border: UiRect::all(px(1.0)),
                        align_items: AlignItems::Stretch,
                        ..default()
                    },
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
            container
        }
        ScreenNode::Spacer(SpacerNode { width, height }) => commands
            .spawn((
                ScreenUiNode,
                Pickable::IGNORE,
                Node {
                    width: px(*width),
                    height: px(*height),
                    ..default()
                },
            ))
            .id(),
    }
}

fn image_node(image: Handle<Image>, atlas: Option<&crate::texture::AtlasTexture>) -> ImageNode {
    if let Some(atlas) = atlas {
        ImageNode::from_atlas_image(image, atlas.atlas.clone())
    } else {
        ImageNode::new(image)
    }
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
pub fn update_offscreen_ui_interactions(
    canvas: Res<crate::HirakuCanvas>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    screen_state: Res<ScreenUiState>,
    mut buttons: Query<
        (
            Entity,
            &ComputedNode,
            &UiGlobalTransform,
            &ComputedStackIndex,
            &InheritedVisibility,
            &mut Interaction,
            Option<&ScreenUiButton>,
            Option<&ScreenUiImageButton>,
        ),
        Or<(
            With<ScreenUiButton>,
            With<ScreenUiImageButton>,
            With<ChoiceButton>,
            With<RuntimeMenuButton>,
        )>,
    >,
) {
    let cursor = windows
        .single()
        .ok()
        .and_then(|window| window.cursor_position().map(|cursor| (window, cursor)))
        .and_then(|(window, cursor)| {
            window_cursor_to_canvas(
                Vec2::new(window.width(), window.height()),
                cursor,
                canvas.size.as_vec2(),
            )
        });

    let mut topmost = None::<(u32, Entity)>;
    if let Some(cursor) = cursor {
        for (entity, computed, transform, stack, visibility, _, screen_button, image_button) in
            &mut buttons
        {
            let belongs_to_active_screen = screen_button
                .map(|button| Some(button.root) == screen_state.active_root)
                .or_else(|| {
                    image_button.map(|button| Some(button.root) == screen_state.active_root)
                })
                .unwrap_or(true);
            if !belongs_to_active_screen
                || !visibility.get()
                || !computed.contains_point(*transform, cursor)
            {
                continue;
            }
            if topmost.is_none_or(|(index, _)| stack.0 >= index) {
                topmost = Some((stack.0, entity));
            }
        }
    }

    let topmost = topmost.map(|(_, entity)| entity);
    for (entity, _, _, _, _, mut interaction, _, _) in &mut buttons {
        let next = if Some(entity) == topmost {
            if mouse.just_pressed(MouseButton::Left) {
                Interaction::Pressed
            } else {
                Interaction::Hovered
            }
        } else {
            Interaction::None
        };
        interaction.set_if_neq(next);
    }
}

pub(super) fn window_cursor_to_canvas(
    window_size: Vec2,
    cursor: Vec2,
    canvas_size: Vec2,
) -> Option<Vec2> {
    let scale = (window_size.x / canvas_size.x).min(window_size.y / canvas_size.y);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let displayed_size = canvas_size * scale;
    let origin = (window_size - displayed_size) * 0.5;
    let cursor = (cursor - origin) / scale;
    (cursor.cmpge(Vec2::ZERO).all() && cursor.cmplt(canvas_size).all()).then_some(cursor)
}

pub fn handle_screen_buttons(
    mut screen_state: ResMut<ScreenUiState>,
    mut responses: MessageWriter<ScriptResponseMessage>,
    mut interaction_query: Query<
        (&Interaction, &mut BackgroundColor, &ScreenUiButton),
        Changed<Interaction>,
    >,
    mut text_query: Query<&mut TextColor, With<ScreenUiButtonText>>,
) {
    for (interaction, mut color, button) in &mut interaction_query {
        if Some(button.root) != screen_state.active_root {
            continue;
        }

        if !button.enabled {
            *color = button.insensitive_background.into();
            if let Ok(mut text_color) = text_query.get_mut(button.text_entity) {
                *text_color = button.insensitive_text_color.into();
            }
            continue;
        }

        match *interaction {
            Interaction::Pressed => {
                *color = button.pressed_background.into();
                if let Ok(mut text_color) = text_query.get_mut(button.text_entity) {
                    *text_color = button.pressed_text_color.into();
                }
                let Some(done) = screen_state.waiting.take() else {
                    continue;
                };
                responses.write(ScriptResponseMessage {
                    request: done,
                    response: ScriptResponse::Choice(button.value.clone()),
                });
            }
            Interaction::Hovered => {
                *color = button.hovered_background.into();
                if let Ok(mut text_color) = text_query.get_mut(button.text_entity) {
                    *text_color = button.hovered_text_color.into();
                }
            }
            Interaction::None => {
                *color = button.normal_background.into();
                if let Ok(mut text_color) = text_query.get_mut(button.text_entity) {
                    *text_color = button.normal_text_color.into();
                }
            }
        }
    }
}

pub fn handle_screen_image_buttons(
    mut screen_state: ResMut<ScreenUiState>,
    mut responses: MessageWriter<ScriptResponseMessage>,
    mut interaction_query: Query<
        (
            &Interaction,
            &mut ImageNode,
            &mut Node,
            &ScreenUiImageButton,
        ),
        Changed<Interaction>,
    >,
) {
    for (interaction, mut image, mut node, button) in &mut interaction_query {
        if Some(button.root) != screen_state.active_root {
            continue;
        }

        match *interaction {
            Interaction::Pressed if button.enabled => {
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
                let Some(done) = screen_state.waiting.take() else {
                    continue;
                };
                responses.write(ScriptResponseMessage {
                    request: done,
                    response: ScriptResponse::Choice(button.value.clone()),
                });
            }
            Interaction::Hovered if button.enabled || button.hovered_when_disabled => {
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
            Interaction::None => {
                image.image = button.normal_texture.clone();
                image.texture_atlas = button.normal_atlas.clone();
                image.rect = button.normal_rect;
                *node = button.normal_node.clone();
            }
            _ => {
                image.image = button.normal_texture.clone();
                image.texture_atlas = button.normal_atlas.clone();
                image.rect = button.normal_rect;
                *node = button.normal_node.clone();
            }
        }
    }
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
