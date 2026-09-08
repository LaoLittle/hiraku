//! Model-owned controls. Picking edits transient interaction state, never the
//! script model. Only a callback can accept a proposed value.
use super::*;
use crate::input::{HirakuTextFocus, HirakuTextInput};
use crate::ui::{InputKind, InputNode, UiCallback};
use bevy::picking::{
    events::{Drag, DragEnd, Press, Release},
    pointer::PointerId,
};
use hiraku_script::Value;

/// Transient state belonging to one mounted UI, never to the story snapshot.
#[derive(Component, Default)]
pub(crate) struct UiLocalState(pub BTreeMap<String, Value>);

#[derive(Component)]
pub(crate) struct ToggleCallback {
    pub root: Entity,
    pub callback: Option<UiCallback>,
    pub binding: Option<crate::ui::PropertyComputation>,
    pub revision: u64,
    pub globals: BTreeMap<String, Value>,
}

pub(crate) fn sync_toggles(
    local_states: Query<&UiLocalState>,
    runtime: Res<ScriptRuntimeState>,
    models: Res<UiModels>,
    mut toggles: Query<(
        &mut ToggleCallback,
        &mut ScreenUiToggle,
        &mut ImageNode,
        &mut Node,
    )>,
) {
    let globals = runtime
        .story
        .as_ref()
        .map(|story| story.globals().clone())
        .unwrap_or_default();
    for (mut model, mut toggle, mut image, mut node) in &mut toggles {
        let mut globals = globals.clone();
        if let Ok(local) = local_states.get(model.root) {
            globals.extend(local.0.clone());
        }
        if model.revision == models.revision() && model.globals == globals {
            continue;
        }
        model.revision = models.revision();
        model.globals = globals.clone();
        let Some(mut binding) = model.binding.clone() else {
            continue;
        };
        binding.globals.extend(globals.clone());
        match crate::script::evaluate_ui_reactive_binding(&binding, &models) {
            Ok(Value::Bool(checked)) if checked != toggle.checked => {
                toggle.checked = checked;
                if checked {
                    image.image = toggle.checked_texture.clone();
                    image.texture_atlas = toggle.checked_atlas.clone();
                    image.rect = toggle.checked_rect;
                    *node = toggle.checked_node.clone();
                } else {
                    image.image = toggle.unchecked_texture.clone();
                    image.texture_atlas = toggle.unchecked_atlas.clone();
                    image.rect = toggle.unchecked_rect;
                    *node = toggle.unchecked_node.clone();
                }
            }
            Ok(_) => {}
            Err(error) => {
                crate::script::emit_script_diagnostic("toggle value failed", &error.to_string());
                model.binding = None;
            }
        }
    }
}

#[derive(Clone, Message)]
pub(crate) struct UiCallbackRequest {
    pub entity: Entity,
    pub root: Entity,
    pub callback: UiCallback,
    pub arguments: Vec<Value>,
}

#[derive(Component)]
pub(crate) struct InputControl {
    entity: Entity,
    pub root: Entity,
    pub spec: InputNode,
    label: Entity,
    fill: Option<Entity>,
    thumb: Option<Entity>,
    edit: TextEdit,
    drag: Option<PointerId>,
    proposal: Option<Value>,
    rendered_revision: u64,
    rendered_globals: BTreeMap<String, Value>,
}

#[derive(Default, Debug)]
struct TextEdit {
    text: String,
    cursor: usize,
    select_all: bool,
    preedit: String,
}

impl TextEdit {
    fn set(&mut self, text: &str) {
        if self.text != text {
            self.text = text.to_owned();
            self.cursor = self.cursor.min(text.len());
            while !text.is_char_boundary(self.cursor) {
                self.cursor -= 1;
            }
            self.select_all = false;
        }
    }

    fn apply(&mut self, input: &HirakuTextInput) -> bool {
        let before = self.text.clone();
        let previous = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        let next = self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8());
        match input {
            HirakuTextInput::Insert(value) => {
                let value: String = value.chars().filter(|c| !c.is_control()).collect();
                if value.is_empty() {
                    return false;
                }
                if self.select_all {
                    self.text.clear();
                    self.cursor = 0;
                }
                self.text.insert_str(self.cursor, &value);
                self.cursor += value.len();
                self.select_all = false;
                self.preedit.clear();
            }
            HirakuTextInput::Backspace | HirakuTextInput::Delete if self.select_all => {
                self.text.clear();
                self.cursor = 0;
                self.select_all = false;
            }
            HirakuTextInput::Backspace => {
                self.text.replace_range(previous..self.cursor, "");
                self.cursor = previous;
            }
            HirakuTextInput::Delete => {
                self.text.replace_range(self.cursor..next, "");
            }
            HirakuTextInput::Left => {
                self.cursor = previous;
                self.select_all = false;
            }
            HirakuTextInput::Right => {
                self.cursor = next;
                self.select_all = false;
            }
            HirakuTextInput::Home => {
                self.cursor = 0;
                self.select_all = false;
            }
            HirakuTextInput::End => {
                self.cursor = self.text.len();
                self.select_all = false;
            }
            HirakuTextInput::SelectAll => self.select_all = true,
            HirakuTextInput::Preedit(text) => self.preedit = text.clone(),
            HirakuTextInput::Cancel => {
                self.preedit.clear();
                self.select_all = false;
            }
            HirakuTextInput::Submit => {}
        }
        self.text != before
    }
}

pub(super) fn spawn_input(
    commands: &mut Commands,
    root: Entity,
    spec: &InputNode,
    fonts: &UiFonts,
    assets: &AssetServer,
    image_handles: &mut Vec<Handle<Image>>,
) -> Entity {
    let mut node = Node {
        width: px(320),
        height: px(48),
        padding: UiRect::axes(px(12), px(6)),
        align_items: AlignItems::Center,
        overflow: Overflow::clip(),
        ..default()
    };
    super::screen_ui::apply_screen_layout(&mut node, &spec.layout);
    if spec.slider_skin.is_some() {
        node.overflow = Overflow::visible();
        node.padding = UiRect::ZERO;
    }
    let entity = commands
        .spawn((
            ScreenUiNode,
            Button,
            node,
            BackgroundColor(
                spec.background
                    .map(color_from_rgba)
                    .unwrap_or(Color::srgb(0.13, 0.15, 0.20)),
            ),
        ))
        .id();
    let fill = if matches!(spec.kind, InputKind::Slider { .. }) {
        Some(
            commands
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        top: px(0),
                        bottom: px(0),
                        width: percent(0),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.22, 0.40, 0.65)),
                    Pickable::IGNORE,
                ))
                .id(),
        )
    } else {
        None
    };
    let label = commands
        .spawn((
            Text::new(""),
            ui_text_font(fonts, spec.text_size.unwrap_or(24.0)),
            TextColor(spec.text_color.map(color_from_rgba).unwrap_or(Color::WHITE)),
            Pickable::IGNORE,
        ))
        .id();
    if let Some(fill) = fill {
        commands.entity(entity).add_child(fill);
    }
    let thumb = spec.slider_skin.as_ref().map(|skin| {
        commands.entity(entity).insert(BackgroundColor(Color::NONE));
        commands.entity(label).insert(Visibility::Hidden);
        let mut image = |texture: &crate::ui::ScreenTexture| {
            let handle = super::save_preview::load_image(assets, &texture.path);
            image_handles.push(handle.clone());
            let mut image = ImageNode::new(handle).with_mode(NodeImageMode::Stretch);
            image.rect = texture.rect.map(|r| {
                Rect::from_corners(Vec2::new(r[0], r[1]), Vec2::new(r[0] + r[2], r[1] + r[3]))
            });
            image
        };
        // Decoration is never a picking target: the input retains one hit area.
        let track = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: percent(22),
                    width: percent(100),
                    height: percent(56),
                    ..default()
                },
                image(&skin[0]),
                Pickable::IGNORE,
            ))
            .id();
        commands.entity(entity).insert_children(0, &[track]);
        if let Some(fill) = fill {
            commands.entity(fill).insert((
                image(&skin[1]),
                BackgroundColor(Color::NONE),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: percent(22),
                    width: percent(0),
                    height: percent(56),
                    ..default()
                },
            ));
        }
        let thumb = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(0),
                    top: px(0),
                    height: percent(100),
                    aspect_ratio: Some(1.0),
                    ..default()
                },
                image(&skin[2]),
                Pickable::IGNORE,
            ))
            .id();
        commands.entity(entity).add_child(thumb);
        thumb
    });
    commands.entity(entity).add_child(label);
    let text = match &spec.value {
        StoredValue::String(text) => text.clone(),
        _ => String::new(),
    };
    commands.entity(entity).insert(InputControl {
        entity,
        root,
        spec: spec.clone(),
        label,
        fill,
        thumb,
        edit: TextEdit {
            cursor: text.len(),
            text,
            ..default()
        },
        drag: None,
        proposal: None,
        rendered_revision: u64::MAX,
        rendered_globals: BTreeMap::new(),
    });
    super::screen_ui::apply_live_layout_bindings(commands, entity, &spec.layout);
    entity
}

fn is_active(root: Entity, screen: &ScreenUiState, overlays: &OverlayUiState) -> bool {
    // Modal screens block interaction with underlying overlays.
    screen.accepts_input(root, overlays)
}

fn request(
    output: &mut MessageWriter<UiCallbackRequest>,
    entity: Entity,
    control: &InputControl,
    value: Value,
    commit: bool,
) {
    let callback = if commit {
        &control.spec.on_commit
    } else {
        &control.spec.on_change
    };
    if let Some(callback) = callback {
        output.write(UiCallbackRequest {
            entity,
            root: control.root,
            callback: callback.clone(),
            arguments: vec![value],
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn input_events(
    mut presses: MessageReader<Pointer<Press>>,
    mut clicks: MessageReader<Pointer<Click>>,
    mut drags: MessageReader<Pointer<Drag>>,
    mut ends: MessageReader<Pointer<DragEnd>>,
    mut releases: MessageReader<Pointer<Release>>,
    mut edits: MessageReader<HirakuTextInput>,
    mut focus: ResMut<HirakuTextFocus>,
    screen: Res<ScreenUiState>,
    overlays: Res<OverlayUiState>,
    parents: Query<&ChildOf>,
    mut controls: Query<(&mut InputControl, &ComputedNode, &UiGlobalTransform)>,
    mut output: MessageWriter<UiCallbackRequest>,
) {
    if focus.0.is_some_and(|entity| {
        controls.get(entity).ok().is_none_or(|(control, _, _)| {
            !control.spec.enabled || !is_active(control.root, &screen, &overlays)
        })
    }) {
        focus.0 = None;
    }
    for event in presses
        .read()
        .filter(|event| event.button == PointerButton::Primary)
    {
        let target = find_component_ancestor(event.entity, &controls, &parents);
        focus.0 = None;
        let Some(entity) = target else { continue };
        let Ok((mut control, computed, transform)) = controls.get_mut(entity) else {
            continue;
        };
        if !control.spec.enabled || !is_active(control.root, &screen, &overlays) {
            continue;
        }
        match control.spec.kind {
            InputKind::TextInput { .. } => {
                focus.0 = Some(entity);
                control.edit.cursor = control.edit.text.len();
            }
            InputKind::Slider { .. } => {
                control.drag = Some(event.pointer_id);
                propose_slider(
                    &mut control,
                    computed,
                    transform,
                    event.pointer_location.position,
                    entity,
                    &mut output,
                );
            }
            InputKind::Checkbox => {}
        }
    }
    for event in clicks
        .read()
        .filter(|event| event.button == PointerButton::Primary)
    {
        let Some(entity) = find_component_ancestor(event.entity, &controls, &parents) else {
            continue;
        };
        let Ok((control, _, _)) = controls.get(entity) else {
            continue;
        };
        if control.spec.enabled
            && is_active(control.root, &screen, &overlays)
            && matches!(control.spec.kind, InputKind::Checkbox)
            && let StoredValue::Bool(value) = control.spec.value
        {
            request(&mut output, entity, &control, Value::Bool(!value), false);
        }
    }
    for event in drags
        .read()
        .filter(|event| event.button == PointerButton::Primary)
    {
        let Some(entity) = find_component_ancestor(event.entity, &controls, &parents) else {
            continue;
        };
        let Ok((mut control, computed, transform)) = controls.get_mut(entity) else {
            continue;
        };
        if control.drag == Some(event.pointer_id)
            && control.spec.enabled
            && is_active(control.root, &screen, &overlays)
        {
            propose_slider(
                &mut control,
                computed,
                transform,
                event.pointer_location.position,
                entity,
                &mut output,
            );
        }
    }
    for event in ends
        .read()
        .filter(|event| event.button == PointerButton::Primary)
    {
        let Some(entity) = find_component_ancestor(event.entity, &controls, &parents) else {
            continue;
        };
        let Ok((mut control, _, _)) = controls.get_mut(entity) else {
            continue;
        };
        if control.drag == Some(event.pointer_id) {
            control.drag = None;
            if control.spec.enabled
                && is_active(control.root, &screen, &overlays)
                && let Some(value) = control.proposal.take()
            {
                request(&mut output, entity, &control, value, true);
            }
        }
    }
    for event in releases
        .read()
        .filter(|event| event.button == PointerButton::Primary)
    {
        // Release can land outside the original track, or without a DragEnd
        // when a user simply clicks. Complete the captured pointer exactly once.
        for (mut control, _, _) in &mut controls {
            if control.drag == Some(event.pointer_id) {
                control.drag = None;
                if control.spec.enabled
                    && is_active(control.root, &screen, &overlays)
                    && let Some(value) = control.proposal.take()
                {
                    request(&mut output, control.entity, &control, value, true);
                }
            }
        }
    }
    for edit in edits.read() {
        let Some(entity) = focus.0 else { continue };
        let Ok((mut control, _, _)) = controls.get_mut(entity) else {
            continue;
        };
        if control.edit.apply(edit) {
            request(
                &mut output,
                entity,
                &control,
                Value::String(control.edit.text.clone()),
                false,
            );
        }
        if matches!(edit, HirakuTextInput::Submit) {
            request(
                &mut output,
                entity,
                &control,
                Value::String(control.edit.text.clone()),
                true,
            );
        }
        if matches!(edit, HirakuTextInput::Cancel) {
            focus.0 = None;
        }
    }
}

fn propose_slider(
    control: &mut InputControl,
    computed: &ComputedNode,
    transform: &UiGlobalTransform,
    position: Vec2,
    entity: Entity,
    output: &mut MessageWriter<UiCallbackRequest>,
) {
    let InputKind::Slider { min, max } = control.spec.kind else {
        return;
    };
    let Some(inverse) = transform.try_inverse() else {
        return;
    };
    if computed.size().x <= 0.0 {
        return;
    }
    let local = inverse.transform_point2(position / computed.inverse_scale_factor());
    let fraction = (local.x / computed.size().x + 0.5).clamp(0.0, 1.0);
    let value = Value::Number(min + (max - min) * fraction as f64);
    if control.proposal.as_ref() != Some(&value) {
        control.proposal = Some(value.clone());
        request(output, entity, control, value, false);
    }
}

pub(crate) fn sync_inputs(
    local_states: Query<&UiLocalState>,
    mut controls: Query<(Entity, &mut InputControl, &ComputedNode)>,
    mut texts: Query<&mut Text>,
    mut nodes: Query<&mut Node>,
    runtime: Res<ScriptRuntimeState>,
    models: Res<UiModels>,
    focus: Res<HirakuTextFocus>,
) {
    let globals = runtime
        .story
        .as_ref()
        .map(|story| story.globals().clone())
        .unwrap_or_default();
    for (entity, mut control, computed) in &mut controls {
        let mut globals = globals.clone();
        if let Ok(local) = local_states.get(control.root) {
            globals.extend(local.0.clone());
        }
        if (control.rendered_revision != models.revision() || control.rendered_globals != globals)
            && let Some(mut binding) = control.spec.reactive_enabled.clone()
        {
            binding.globals.extend(globals.clone());
            match crate::script::evaluate_ui_reactive_binding(&binding, &models) {
                Ok(Value::Bool(enabled)) => control.spec.enabled = enabled,
                Ok(_) => control.spec.enabled = false,
                Err(error) => {
                    crate::script::emit_script_diagnostic(
                        "input enabled state failed",
                        &error.to_string(),
                    );
                    control.spec.enabled = false;
                    control.spec.reactive_enabled = None;
                }
            }
        }
        if (control.rendered_revision != models.revision() || control.rendered_globals != globals)
            && let Some(mut binding) = control.spec.reactive_value.clone()
        {
            binding.globals.extend(globals.clone());
            match crate::script::evaluate_ui_reactive_binding(&binding, &models) {
                Ok(Value::Bool(value)) if matches!(control.spec.kind, InputKind::Checkbox) => {
                    control.spec.value = StoredValue::Bool(value)
                }
                Ok(Value::Number(value))
                    if value.is_finite()
                        && matches!(control.spec.kind, InputKind::Slider { .. }) =>
                {
                    control.spec.value = StoredValue::Float(value)
                }
                Ok(Value::String(value))
                    if matches!(control.spec.kind, InputKind::TextInput { .. }) =>
                {
                    control.spec.value = StoredValue::String(value)
                }
                Ok(_) => {}
                Err(error) => {
                    crate::script::emit_script_diagnostic("input value failed", &error.to_string());
                    control.spec.reactive_value = None;
                }
            }
        }
        control.rendered_revision = models.revision();
        if control.rendered_globals != globals {
            control.rendered_globals = globals.clone();
        }
        let display = match (&control.spec.kind, &control.spec.value) {
            (InputKind::Checkbox, StoredValue::Bool(value)) => {
                if *value { "[x]" } else { "[ ]" }.into()
            }
            (InputKind::Slider { min, max }, StoredValue::Float(value)) => {
                if let Some(thumb) = control.thumb
                    && let Ok(mut node) = nodes.get_mut(thumb)
                {
                    node.left =
                        percent(((value - min) / (max - min)).clamp(0.0, 1.0) as f32 * 100.0);
                    node.margin.left =
                        px(-0.5 * computed.size().y * computed.inverse_scale_factor());
                }
                if let Some(fill) = control.fill
                    && let Ok(mut node) = nodes.get_mut(fill)
                {
                    node.width =
                        percent(((value - min) / (max - min)).clamp(0.0, 1.0) as f32 * 100.0);
                }
                format!("{value:.2}")
            }
            (InputKind::TextInput { placeholder }, StoredValue::String(value)) => {
                let value = value.clone();
                let placeholder = placeholder.clone();
                control.edit.set(&value);
                if value.is_empty() && control.edit.preedit.is_empty() {
                    placeholder
                } else if focus.0 == Some(entity) {
                    let cursor = control.edit.cursor;
                    format!(
                        "{}{}│{}",
                        &value[..cursor],
                        control.edit.preedit,
                        &value[cursor..]
                    )
                } else {
                    value
                }
            }
            _ => String::new(),
        };
        if let Ok(mut text) = texts.get_mut(control.label)
            && text.0 != display
        {
            text.0 = display;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_input_app() -> (App, Entity, Entity) {
        let screen = crate::script::evaluate_ui_component_named_with_args(
            "memory://text_input.ui.hks",
            "import ui.widgets.*\nscreen { textInput(\"alice\").onChange { value: String -> () }.onCommit { value: String -> () } }",
            crate::script::UiContext::default(), &TextureCatalog::default(), &TermCatalog::default(), &[],
        ).expect("synthetic text input mounts");
        let ScreenNode::Input(spec) = screen.children.into_iter().next().expect("one input") else {
            panic!("expected input")
        };
        let mut app = App::new();
        app.init_resource::<ScreenUiState>()
            .init_resource::<OverlayUiState>()
            .init_resource::<HirakuTextFocus>()
            .init_resource::<ScriptRuntimeState>()
            .init_resource::<UiModels>()
            .add_message::<Pointer<Press>>()
            .add_message::<Pointer<Click>>()
            .add_message::<Pointer<Drag>>()
            .add_message::<Pointer<DragEnd>>()
            .add_message::<Pointer<Release>>()
            .add_message::<HirakuTextInput>()
            .add_message::<UiCallbackRequest>()
            .add_systems(Update, (input_events, sync_inputs).chain());
        let root = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(root);
        let label = app.world_mut().spawn(Text::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                Node::default(),
                ComputedNode::default(),
                UiGlobalTransform::default(),
            ))
            .id();
        app.world_mut().entity_mut(entity).insert(InputControl {
            entity,
            root,
            spec,
            label,
            fill: None,
            thumb: None,
            edit: TextEdit {
                text: "alice".into(),
                cursor: 5,
                ..default()
            },
            drag: None,
            proposal: None,
            rendered_revision: u64::MAX,
            rendered_globals: BTreeMap::new(),
        });
        app.world_mut().resource_mut::<HirakuTextFocus>().0 = Some(entity);
        (app, entity, label)
    }

    #[test]
    fn text_edits_are_proposals_and_external_updates_do_not_emit_changes() {
        let (mut app, entity, label) = text_input_app();
        app.update();
        assert!(
            app.world()
                .resource::<Messages<UiCallbackRequest>>()
                .is_empty()
        );
        app.world_mut().write_message(HirakuTextInput::SelectAll);
        app.world_mut()
            .write_message(HirakuTextInput::Insert("bob".into()));
        app.update();
        let changes = app
            .world_mut()
            .resource_mut::<Messages<UiCallbackRequest>>()
            .drain()
            .collect::<Vec<_>>();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].arguments, [Value::String("bob".into())]);
        assert_eq!(
            app.world()
                .get::<InputControl>(entity)
                .expect("input exists")
                .spec
                .value,
            StoredValue::String("alice".into()),
            "proposal must not overwrite model"
        );
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<InputControl>()
            .expect("input exists")
            .spec
            .value = StoredValue::String("bob".into());
        app.update();
        assert!(
            app.world()
                .resource::<Messages<UiCallbackRequest>>()
                .is_empty()
        );
        assert!(
            app.world()
                .get::<Text>(label)
                .expect("label exists")
                .0
                .contains("bob")
        );
        app.world_mut().despawn(entity);
        app.update();
        assert_eq!(app.world().resource::<HirakuTextFocus>().0, None);
    }

    #[test]
    fn ime_preedit_is_transient_and_submit_is_separate_from_change() {
        let (mut app, _, _) = text_input_app();
        app.world_mut()
            .write_message(HirakuTextInput::Preedit("é".into()));
        app.update();
        assert!(
            app.world()
                .resource::<Messages<UiCallbackRequest>>()
                .is_empty()
        );
        app.world_mut().write_message(HirakuTextInput::Submit);
        app.update();
        let requests = app
            .world_mut()
            .resource_mut::<Messages<UiCallbackRequest>>()
            .drain()
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].arguments, [Value::String("alice".into())]);
    }

    #[test]
    fn slider_click_without_drag_commits_once_on_release() {
        use bevy::{
            camera::NormalizedRenderTarget,
            picking::{backend::HitData, pointer::Location},
        };
        let (mut app, entity, _) = text_input_app();
        let screen = crate::script::evaluate_ui_component_named_with_args(
            "memory://slider.ui.hks",
            "import ui.widgets.*\nscreen { slider(0.5, 0.0, 1.0).onChange { value: Float -> () }.onCommit { value: Float -> () } }",
            crate::script::UiContext::default(), &TextureCatalog::default(), &TermCatalog::default(), &[],
        ).expect("slider mounts");
        let ScreenNode::Input(spec) = screen.children.into_iter().next().expect("one slider")
        else {
            panic!("expected slider")
        };
        app.world_mut()
            .get_mut::<InputControl>(entity)
            .expect("input exists")
            .spec = spec;
        app.world_mut()
            .get_mut::<ComputedNode>(entity)
            .expect("layout exists")
            .size = Vec2::new(100.0, 48.0);
        let location = Location {
            target: NormalizedRenderTarget::None {
                width: 100,
                height: 48,
            },
            position: Vec2::new(25.0, 0.0),
        };
        let hit = HitData {
            camera: entity,
            depth: 0.0,
            position: None,
            normal: None,
            extra: None,
        };
        let pointer = PointerId::Custom(uuid::Uuid::from_u128(5));
        app.world_mut().write_message(Pointer::new(
            pointer,
            location.clone(),
            Press {
                button: PointerButton::Primary,
                hit: hit.clone(),
                count: 1,
            },
            entity,
        ));
        app.world_mut().write_message(Pointer::new(
            pointer,
            location,
            Release {
                button: PointerButton::Primary,
                hit,
            },
            entity,
        ));
        app.update();
        let requests = app
            .world_mut()
            .resource_mut::<Messages<UiCallbackRequest>>()
            .drain()
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 2, "one proposal and one commit");
        assert_eq!(requests[0].arguments, [Value::Number(0.75)]);
        assert_eq!(requests[1].arguments, [Value::Number(0.75)]);
        assert_eq!(
            app.world()
                .get::<InputControl>(entity)
                .expect("slider exists")
                .spec
                .value,
            StoredValue::Float(0.5)
        );
        app.update();
        assert!(
            app.world()
                .resource::<Messages<UiCallbackRequest>>()
                .is_empty()
        );
    }
    #[test]
    fn unicode_edits_and_preedit_do_not_corrupt_text() {
        let mut edit = TextEdit::default();
        assert!(edit.apply(&HirakuTextInput::Insert("aliceé🙂".into())));
        assert!(!edit.apply(&HirakuTextInput::Preedit("bob".into())));
        assert_eq!(edit.text, "aliceé🙂");
        assert!(edit.apply(&HirakuTextInput::Backspace));
        assert_eq!(edit.text, "aliceé");
        edit.apply(&HirakuTextInput::Left);
        edit.apply(&HirakuTextInput::Delete);
        assert_eq!(edit.text, "alice");
        edit.apply(&HirakuTextInput::SelectAll);
        edit.apply(&HirakuTextInput::Insert("bob".into()));
        assert_eq!(edit.text, "bob");
        assert_eq!(edit.cursor, 3);
    }
}
