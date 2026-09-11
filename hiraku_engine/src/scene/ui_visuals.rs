//! Mount-owned fades and inherited visual modulation, independent of story VM execution.
use super::*;

#[derive(Component)]
pub(crate) struct HoverBrightness(pub f32);

#[derive(Component)]
pub(crate) struct ScreenFade {
    seconds: f32,
    alpha: f32,
}
impl ScreenFade {
    pub fn new(seconds: f32) -> Self {
        Self {
            seconds,
            alpha: 0.0,
        }
    }
    fn advance(&mut self, delta: f32, closing: bool) -> bool {
        let step = delta / self.seconds.max(f32::EPSILON);
        self.alpha = (self.alpha + if closing { -step } else { step }).clamp(0.0, 1.0);
        closing && self.alpha == 0.0
    }
}
#[derive(Component)]
pub(crate) struct FadeResult {
    pub value: hiraku_script::Value,
    pub complete: bool,
}

pub(crate) fn finish(
    commands: &mut Commands,
    screen: &mut ScreenUiState,
    responses: &mut MessageWriter<ScriptResponseMessage>,
    value: hiraku_script::Value,
    complete: bool,
) {
    loop {
        let request = screen.waiting.take();
        super::screen_ui::close_screen_ui(commands, screen);
        if let Some(request) = request {
            responses.write(ScriptResponseMessage {
                request,
                response: ScriptResponse::UiResult(value),
            });
            break;
        }
        if !complete || screen.active_root.is_none() {
            break;
        }
    }
}

pub(crate) fn tick(
    time: Res<Time>,
    mut commands: Commands,
    mut screen: ResMut<ScreenUiState>,
    mut fades: Query<(Entity, &mut ScreenFade, Option<&FadeResult>)>,
    mut responses: MessageWriter<ScriptResponseMessage>,
    mut redraw: crate::redraw::Redraw,
) {
    for (root, mut fade, result) in &mut fades {
        if screen.active_root != Some(root) || screen.pending_root.is_some() {
            continue;
        }
        if fade.alpha == 1.0 && result.is_none() {
            continue;
        }
        redraw.request();
        if fade.advance(time.delta_secs(), result.is_some()) {
            let result = result.expect("closing fade result");
            finish(
                &mut commands,
                &mut screen,
                &mut responses,
                result.value.clone(),
                result.complete,
            );
        }
    }
}

#[derive(Clone, Default)]
struct ColorState {
    base: Option<Color>,
    applied: Option<Color>,
}
impl ColorState {
    fn apply(&mut self, color: &mut Color, brightness: f32, alpha: f32) {
        // ImageNode stores an sRGB Color, whereas UiMaterial stores LinearRgba.
        // A representation change is not a new authored style: otherwise an
        // initially hidden material permanently adopts alpha zero as its base.
        if self.applied.map(|value| value.to_linear()) != Some(color.to_linear()) {
            self.base = Some(*color);
        }
        let mut value = self.base.unwrap_or(*color).to_srgba();
        value.red = (value.red * brightness).min(1.0);
        value.green = (value.green * brightness).min(1.0);
        value.blue = (value.blue * brightness).min(1.0);
        value.alpha *= alpha;
        let result = Color::Srgba(value);
        if *color != result {
            *color = result;
        }
        self.applied = Some(result);
    }
}

#[derive(Component, Clone, Default)]
pub(crate) struct AppliedVisual {
    background: ColorState,
    image: ColorState,
    text: ColorState,
    shadow: ColorState,
}

pub(crate) fn apply(
    mut commands: Commands,
    mut materials: Option<ResMut<Assets<crate::render::ui_quad::UiQuadMaterial>>>,
    parents: Query<&ChildOf>,
    modifiers: Query<(
        Option<&ScreenFade>,
        Option<&HoverBrightness>,
        Option<&PickingInteraction>,
        Option<&super::ui_keyframes::Opacity>,
    )>,
    roots: Query<(), With<ScreenUiRoot>>,
    screen: Res<ScreenUiState>,
    overlays: Res<OverlayUiState>,
    mut colors: Query<
        (
            Entity,
            Option<&mut BackgroundColor>,
            Option<&mut ImageNode>,
            Option<&mut TextColor>,
            Option<&mut TextShadow>,
            Option<&mut AppliedVisual>,
            Option<
                &bevy::ui_render::ui_material::MaterialNode<crate::render::ui_quad::UiQuadMaterial>,
            >,
        ),
        Or<(
            With<BackgroundColor>,
            With<ImageNode>,
            With<TextColor>,
            With<TextShadow>,
            With<
                bevy::ui_render::ui_material::MaterialNode<crate::render::ui_quad::UiQuadMaterial>,
            >,
        )>,
    >,
) {
    for (entity, background, image, text, shadow, cache, material) in &mut colors {
        let (mut alpha, mut brightness) = (1.0, 1.0);
        let mut current = entity;
        let mut interactive = true;
        loop {
            if roots.contains(current) {
                interactive = screen.accepts_input(current, &overlays);
            }
            if let Ok((fade, hover, interaction, opacity)) = modifiers.get(current) {
                if let Some(opacity) = opacity {
                    alpha *= opacity.0;
                }
                if let Some(fade) = fade {
                    alpha *= fade.alpha;
                }
                if let Some(hover) = hover {
                    if matches!(
                        interaction,
                        Some(PickingInteraction::Hovered | PickingInteraction::Pressed)
                    ) {
                        brightness *= hover.0;
                    }
                }
            }
            let Ok(parent) = parents.get(current) else {
                break;
            };
            current = parent.parent();
        }
        if !interactive {
            brightness = 1.0;
        }
        if alpha == 1.0 && brightness == 1.0 && cache.is_none() {
            continue;
        }
        let mut next = cache.as_deref().cloned().unwrap_or_default();
        if let Some(mut material) = material.and_then(|handle| {
            materials
                .as_mut()
                .and_then(|assets| assets.get_mut(&handle.0))
        }) {
            let mut color = Color::LinearRgba(material.color);
            next.image.apply(&mut color, brightness, alpha);
            material.color = color.to_linear();
        }
        if let Some(mut color) = background {
            next.background.apply(&mut color.0, brightness, alpha);
        }
        if let Some(mut image) = image {
            next.image.apply(&mut image.color, brightness, alpha);
        }
        if let Some(mut color) = text {
            next.text.apply(&mut color.0, brightness, alpha);
        }
        if let Some(mut shadow) = shadow {
            next.shadow.apply(&mut shadow.color, brightness, alpha);
        }
        if let Some(mut cache) = cache {
            *cache = next;
        } else {
            commands.entity(entity).try_insert(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn nested_completion_resolves_story_once_after_fade() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ScreenUiState>()
            .add_message::<ScriptResponseMessage>()
            .add_message::<bevy::window::RequestRedraw>()
            .add_systems(Update, tick);
        let parent = app.world_mut().spawn_empty().id();
        let mut fade = ScreenFade::new(0.2);
        fade.advance(0.2, false);
        let child = app
            .world_mut()
            .spawn((
                fade,
                FadeResult {
                    value: hiraku_script::Value::String("answer".into()),
                    complete: true,
                },
            ))
            .id();
        *app.world_mut().resource_mut::<ScreenUiState>() = ScreenUiState {
            active_root: Some(child),
            closing_root: Some(child),
            stack: vec![(parent, Some(ScriptRequestId(7)))],
            ..default()
        };
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(100));
        app.update();
        assert_eq!(
            app.world().resource::<ScreenUiState>().active_root,
            Some(child)
        );
        assert!(
            app.world()
                .resource::<Messages<ScriptResponseMessage>>()
                .is_empty()
        );
        app.update();
        assert!(
            app.world()
                .resource::<ScreenUiState>()
                .active_root
                .is_none()
        );
        assert!(app.world().get_entity(parent).is_err());
        assert!(app.world().get_entity(child).is_err());
        let events: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ScriptResponseMessage>>()
            .drain()
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].request, ScriptRequestId(7));
        app.update();
        assert!(
            app.world()
                .resource::<Messages<ScriptResponseMessage>>()
                .is_empty()
        );
    }
    #[test]
    fn fade_can_reverse_without_a_jump() {
        let mut fade = ScreenFade::new(0.2);
        assert!(!fade.advance(0.1, false));
        assert_eq!(fade.alpha, 0.5);
        assert!(!fade.advance(0.05, true));
        assert_eq!(fade.alpha, 0.25);
        assert!(fade.advance(0.05, true));
    }
    #[test]
    fn ecs_material_hidden_prelude_does_not_poison_later_opacity() {
        use crate::render::ui_quad::UiQuadMaterial;
        use bevy::ui_render::ui_material::MaterialNode;
        let mut app = App::new();
        app.init_resource::<Assets<UiQuadMaterial>>()
            .init_resource::<ScreenUiState>()
            .init_resource::<OverlayUiState>()
            .add_systems(Update, apply);
        let material = app
            .world_mut()
            .resource_mut::<Assets<UiQuadMaterial>>()
            .add(UiQuadMaterial {
                color: LinearRgba::WHITE,
                uv_u: Vec4::X,
                uv_v: Vec4::Y,
                image: Handle::default(),
                parameters: Vec4::ZERO,
                clock: Vec4::ZERO,
                texture1: Handle::default(),
                texture2: Handle::default(),
                texture3: Handle::default(),
                shader: None,
                blend: crate::ui::UiShaderBlend::Alpha,
            });
        let entity = app
            .world_mut()
            .spawn((
                MaterialNode(material.clone()),
                super::super::ui_keyframes::Opacity(0.0),
            ))
            .id();
        for opacity in [0.0, 0.0, 0.25, 1.0, 1.0, 0.0, 1.0] {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<super::super::ui_keyframes::Opacity>()
                .expect("opacity track")
                .0 = opacity;
            app.update();
            let assets = app.world().resource::<Assets<UiQuadMaterial>>();
            assert_eq!(
                assets.get(&material).expect("material").color.alpha,
                opacity
            );
        }
    }

    #[test]
    fn initially_hidden_material_recovers_after_color_space_roundtrip() {
        let mut state = ColorState::default();
        let mut color = Color::srgba(1.0, 0.5, 0.25, 1.0);
        state.apply(&mut color, 1.0, 0.0);
        for alpha in [0.0, 0.25, 0.5, 1.0, 0.0, 1.0] {
            color = Color::LinearRgba(color.to_linear());
            state.apply(&mut color, 1.0, alpha);
            assert_eq!(color.to_srgba().alpha, alpha);
            assert_eq!(color.to_srgba().green, 0.5);
        }
    }

    #[test]
    fn modulation_does_not_accumulate_and_respects_new_styles() {
        let mut state = ColorState::default();
        let mut color = Color::srgba(1.0, 0.5, 0.25, 0.8);
        let original = color;
        for _ in 0..100 {
            state.apply(&mut color, 0.75, 0.5);
        }
        assert_eq!(color.to_srgba().alpha, 0.4);
        state.apply(&mut color, 1.0, 1.0);
        assert_eq!(color, original);
        color = Color::BLACK;
        state.apply(&mut color, 0.75, 0.5);
        state.apply(&mut color, 1.0, 1.0);
        assert_eq!(color.to_srgba(), Color::BLACK.to_srgba());
    }
}
