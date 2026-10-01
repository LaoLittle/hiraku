//! Ruby uses ordinary Bevy text spans and glyph layout, not a second camera.
use crate::ui::{PropertyComputation, UiModels};
use hiraku_text::{Document as RichText, Ruby};

fn span_color(document: &RichText, index: u32, fallback: TextColor) -> TextColor {
    document
        .styles
        .get(index as usize)
        .copied()
        .and_then(|style| style.color)
        .map_or(fallback, |[r, g, b, a]| {
            TextColor(Color::srgba_u8(r, g, b, a))
        })
}

fn span_font(document: &RichText, index: usize, font: &TextFont) -> TextFont {
    let mut font = font.clone();
    if let Some(style) = document.styles.get(index) {
        if style.bold {
            font.weight = bevy::text::FontWeight::BOLD;
        }
        if style.italic {
            font.style = bevy::text::FontStyle::Italic;
        }
    }
    font
}
use bevy::math::Affine2;
use bevy::prelude::*;

#[derive(Component)]
pub(crate) struct RichTextSource {
    pub source: String,
    count: Option<u32>,
    expression: Option<PropertyComputation>,
    revision: u64,
    rendered: Option<String>,
    document: RichText,
    spans: Vec<Entity>,
    labels: Vec<Entity>,
    previous_count: u32,
    plain: bool,
}

impl RichTextSource {
    pub fn new(source: String, layout: &crate::ui::ScreenLayout) -> Self {
        Self {
            source,
            count: layout.text_reveal,
            expression: layout.reactive_text_reveal.clone(),
            revision: u64::MAX,
            rendered: None,
            document: RichText::default(),
            spans: Vec::new(),
            labels: Vec::new(),
            previous_count: 0,
            plain: false,
        }
    }
}

#[derive(Component)]
pub(crate) struct RubyLabel {
    root: Entity,
    range: Ruby,
    positioned: bool,
}

#[derive(Component, Default)]
pub(crate) struct RichGlyphs {
    glyphs: Vec<bevy::text::PositionedGlyph>,
    decorations: Vec<bevy::text::RunGeometry>,
    scale: f32,
    shown: Option<u32>,
}

/// Preserve the complete shaped layout, exposing only revealed glyphs to the
/// renderer. Bevy's TextShadow does not respect per-span alpha, so alpha alone
/// would reveal the shadows of hidden letters. This also avoids reshaping on
/// every typewriter tick and keeps line breaks fixed throughout the reveal.
pub(crate) fn reveal_glyphs(
    mut roots: Query<(
        &RichTextSource,
        &mut bevy::text::TextLayoutInfo,
        &mut RichGlyphs,
    )>,
) {
    for (rich, mut layout, mut cache) in &mut roots {
        let shaped = layout.is_changed();
        if shaped {
            cache.glyphs.clone_from(&layout.glyphs);
            cache.decorations.clone_from(&layout.run_geometry);
            cache.scale = layout.scale_factor;
        }
        if shaped || cache.shown != Some(rich.previous_count) {
            layout.glyphs = cache
                .glyphs
                .iter()
                .filter(|glyph| glyph.section_index <= rich.previous_count)
                .cloned()
                .collect();
            layout.run_geometry = cache
                .decorations
                .iter()
                .filter(|run| run.section_index <= rich.previous_count)
                .cloned()
                .collect();
            cache.shown = Some(rich.previous_count);
        }
    }
}

pub(crate) fn update(
    evaluator: Local<crate::script::UiPropertyEvaluator>,
    mut commands: Commands,
    mut redraw: crate::redraw::Redraw,
    models: Res<UiModels>,
    parents: Query<&ChildOf>,
    locals: Query<&super::widgets::UiLocalState>,
    mut roots: Query<(
        Entity,
        &mut RichTextSource,
        Ref<TextFont>,
        Ref<TextColor>,
        Option<Ref<TextShadow>>,
        Option<&mut Text>,
    )>,
) {
    for (entity, mut rich, font, color, shadow, mut root_text) in &mut roots {
        if let Some(mut expression) = rich.expression.take() {
            let local_changed =
                super::screen_ui::refresh_local_binding(entity, &mut expression, &parents, &locals);
            if rich.revision != models.revision() || local_changed {
                let models_changed =
                    crate::script::refresh_ui_property_models(&mut expression, &models);
                let evaluate = rich.revision == u64::MAX || local_changed || models_changed;
                rich.revision = models.revision();
                if evaluate {
                    use hiraku_script::native::FromHksValue;
                    match evaluator
                        .evaluate(&expression, &models)
                        .map_err(|e| e.to_string())
                        .and_then(|value| i64::from_hks_value(&value).map_err(|e| e.to_string()))
                        .and_then(|value| {
                            u32::try_from(value).map_err(|_| {
                                "reveal count must fit a nonnegative 32-bit integer".to_string()
                            })
                        }) {
                        Ok(count) => rich.count = Some(count),
                        Err(error) => crate::script::emit_script_diagnostic(
                            "rich text reveal failed",
                            &error.to_string(),
                        ),
                    }
                }
            }
            rich.expression = Some(expression);
        }
        if rich.rendered.as_ref() != Some(&rich.source) {
            let document = match hiraku_text::parse(&rich.source) {
                Ok(value) => value,
                Err(error) => {
                    crate::script::emit_script_diagnostic("invalid rich text", &error.to_string());
                    // Keep malformed text readable, never panic in a UI system.
                    RichText::plain(rich.source.clone())
                }
            };
            let plain = root_text.is_some()
                && rich.count.is_none()
                && rich.expression.is_none()
                && document.ruby.is_empty()
                && document
                    .styles
                    .iter()
                    .all(|style| *style == hiraku_text::TextStyle::default());
            let append = plain == rich.plain
                && document.text.starts_with(&rich.document.text)
                && document.ruby.starts_with(&rich.document.ruby)
                && document.styles.starts_with(&rich.document.styles);
            if !append {
                for child in std::mem::take(&mut rich.spans)
                    .into_iter()
                    .chain(std::mem::take(&mut rich.labels))
                {
                    commands.entity(child).try_despawn();
                }
                rich.previous_count = 0;
            }
            if let Some(text) = root_text.as_deref_mut() {
                if plain {
                    text.0.clone_from(&document.text);
                } else {
                    text.0.clear();
                }
            }
            let old_len = if plain {
                document.styles.len()
            } else {
                rich.spans.len()
            };
            for (index, ch) in document.text.chars().enumerate().skip(old_len) {
                let index = index as u32;
                // Word joiners keep a ruby base on one line without entering
                // the stored text or the dialogue character counter.
                let joined = document
                    .ruby
                    .iter()
                    .any(|r| r.start <= index && index + 1 < r.end);
                let annotated = document
                    .ruby
                    .iter()
                    .any(|r| r.start <= index && index < r.end);
                let text = if joined {
                    format!("{ch}\u{2060}")
                } else {
                    ch.to_string()
                };
                let span = commands
                    .spawn((
                        TextSpan::new(text),
                        Pickable::IGNORE,
                        span_font(&document, index as usize, &font),
                        span_color(&document, index, *color),
                        bevy::text::LineHeight::RelativeToFont(if annotated { 1.8 } else { 1.2 }),
                    ))
                    .id();
                commands.entity(entity).add_child(span);
                let style = document.styles[index as usize];
                if style.strike {
                    commands.entity(span).insert(bevy::text::Strikethrough);
                }
                if style.underline {
                    commands.entity(span).insert(bevy::text::Underline);
                }
                rich.spans.push(span);
            }
            for range in document.ruby.iter().skip(rich.labels.len()) {
                let mut reading_font = (*font).clone();
                if let bevy::text::FontSize::Px(size) = font.font_size {
                    reading_font.font_size = bevy::text::FontSize::Px(size * 0.5);
                }
                let label = commands
                    .spawn((
                        RubyLabel {
                            root: entity,
                            range: range.clone(),
                            positioned: false,
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            flex_shrink: 0.0,
                            ..default()
                        },
                        Text::new(range.reading.clone()),
                        reading_font,
                        span_color(&document, range.start, *color),
                        TextLayout::new(Justify::Left, bevy::text::LineBreak::NoWrap),
                        shadow.as_deref().copied().unwrap_or_default(),
                        Visibility::Hidden,
                        Pickable::IGNORE,
                    ))
                    .id();
                let parent = parents.get(entity).map_or(entity, ChildOf::parent);
                commands.entity(parent).add_child(label);
                rich.labels.push(label);
            }
            rich.document = document;
            rich.plain = plain;
            rich.rendered = Some(rich.source.clone());
            redraw.request();
        }
        if font.is_changed()
            || color.is_changed()
            || shadow.as_ref().is_some_and(|s| s.is_changed())
        {
            for (index, &span) in rich.spans.iter().enumerate() {
                commands.entity(span).insert((
                    span_font(&rich.document, index, &font),
                    span_color(&rich.document, index as u32, *color),
                ));
            }
            let mut reading_font = (*font).clone();
            if let bevy::text::FontSize::Px(size) = font.font_size {
                reading_font.font_size = bevy::text::FontSize::Px(size * 0.5);
            }
            for (&label, range) in rich.labels.iter().zip(&rich.document.ruby) {
                commands.entity(label).insert((
                    reading_font.clone(),
                    span_color(&rich.document, range.start, *color),
                    shadow.as_deref().copied().unwrap_or_default(),
                ));
            }
            redraw.request();
        }
        let count = rich.count.map_or(rich.document.styles.len(), |n| {
            (n as usize).min(rich.document.styles.len())
        }) as u32;
        if count != rich.previous_count {
            rich.previous_count = count;
            redraw.request();
        }
    }
}

/// Positions come from the font shaper, not guessed character widths. A label
/// remains hidden until its new Node position has passed through UI layout.
pub(crate) fn position_ruby(
    mut redraw: crate::redraw::Redraw,
    roots: Query<(
        &RichTextSource,
        &RichGlyphs,
        Option<&UiGlobalTransform>,
        Option<&ChildOf>,
        Option<&ComputedNode>,
    )>,
    transforms: Query<(&UiGlobalTransform, &ComputedNode)>,
    mut labels: Query<(
        &mut RubyLabel,
        &bevy::text::TextLayoutInfo,
        &mut Node,
        &mut Visibility,
    )>,
) {
    for (mut label, reading, mut node, mut visibility) in &mut labels {
        let Ok((rich, layout, root_transform, parent, root_node)) = roots.get(label.root) else {
            continue;
        };
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        for glyph in &layout.glyphs {
            if glyph.section_index > label.range.start && glyph.section_index <= label.range.end {
                let half = glyph.atlas_info.rect.size() * 0.5;
                min = min.min((glyph.position - half) / layout.scale);
                max = max.max((glyph.position + half) / layout.scale);
            }
        }
        if !min.is_finite() || reading.size.x <= 0.0 {
            continue;
        }
        let offset = root_transform
            .zip(parent)
            .and_then(|(root, parent)| {
                transforms
                    .get(parent.parent())
                    .ok()
                    .map(|(parent, parent_node)| {
                        let relative = Affine2::from(*parent).inverse() * Affine2::from(*root);
                        (relative.translation
                            + (parent_node.size()
                                - root_node.map_or(Vec2::ZERO, ComputedNode::size))
                                * 0.5)
                            / layout.scale
                    })
            })
            .unwrap_or(Vec2::ZERO);
        let left = px(offset.x + (min.x + max.x - reading.size.x) * 0.5);
        let top = px(offset.y + min.y - reading.size.y);
        let moved = node.left != left || node.top != top;
        if moved {
            node.left = left;
            node.top = top;
            label.positioned = false;
        } else {
            label.positioned = true;
        }
        let visible = label.positioned && rich.previous_count > label.range.start;
        let next = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != next {
            *visibility = next;
            redraw.request();
        }
        if moved {
            redraw.request();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_uses_one_text_entity_and_can_switch_to_markup() {
        let mut app = App::new();
        app.init_resource::<UiModels>().add_systems(Update, update);
        let root = app
            .world_mut()
            .spawn((
                Text::new(""),
                TextFont::default(),
                TextColor::WHITE,
                RichTextSource::new("Alice#br;Bob".into(), &crate::ui::ScreenLayout::default()),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Text>(root).expect("plain text").0,
            "Alice\nBob"
        );
        assert!(
            app.world()
                .get::<RichTextSource>(root)
                .expect("source")
                .spans
                .is_empty()
        );
        app.world_mut()
            .get_mut::<RichTextSource>(root)
            .expect("source")
            .source = "*Alice*".into();
        app.update();
        assert!(
            app.world()
                .get::<Text>(root)
                .expect("rich root")
                .0
                .is_empty()
        );
        let spans = app
            .world()
            .get::<RichTextSource>(root)
            .expect("source")
            .spans
            .clone();
        assert_eq!(spans.len(), 5);
        app.world_mut()
            .get_mut::<RichTextSource>(root)
            .expect("source")
            .source = "Bob".into();
        app.update();
        assert_eq!(app.world().get::<Text>(root).expect("plain again").0, "Bob");
        assert!(
            spans
                .iter()
                .all(|&entity| app.world().get_entity(entity).is_err())
        );
    }

    #[test]
    fn ruby_keeps_base_glyphs_in_the_shaped_ui_layout() {
        use bevy::app::{HierarchyPropagatePlugin, PropagateSet};
        use bevy::ui::{ComputedUiRenderTargetInfo, ComputedUiTargetCamera, UiSystems};
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::text::TextPlugin,
        ))
        .init_resource::<Assets<Image>>()
        .init_resource::<bevy::ui::UiScale>()
        .init_resource::<bevy::ui::ui_surface::UiSurface>()
        .init_resource::<UiModels>()
        .add_plugins(HierarchyPropagatePlugin::<ComputedUiTargetCamera>::new(
            PostUpdate,
        ))
        .add_plugins(HierarchyPropagatePlugin::<ComputedUiRenderTargetInfo>::new(
            PostUpdate,
        ))
        .configure_sets(
            PostUpdate,
            (
                UiSystems::Prepare,
                UiSystems::Propagate,
                UiSystems::Content,
                UiSystems::Layout,
            )
                .chain(),
        )
        .configure_sets(
            PostUpdate,
            PropagateSet::<ComputedUiTargetCamera>::default().in_set(UiSystems::Propagate),
        )
        .configure_sets(
            PostUpdate,
            PropagateSet::<ComputedUiRenderTargetInfo>::default().in_set(UiSystems::Propagate),
        )
        .add_systems(Update, update)
        .add_systems(
            PostUpdate,
            (
                bevy::ui::update::propagate_ui_target_cameras.in_set(UiSystems::Prepare),
                bevy::ui::widget::measure_text_system
                    .in_set(UiSystems::Content)
                    .after(bevy::text::detect_text_needs_rerender)
                    .after(bevy::text::load_font_assets_into_font_collection),
                bevy::ui::ui_layout_system.in_set(UiSystems::Layout),
                bevy::ui::widget::text_system.after(UiSystems::Layout),
                (reveal_glyphs, position_ruby)
                    .chain()
                    .after(bevy::ui::widget::text_system),
            ),
        );
        app.world_mut().spawn((
            Camera2d,
            Camera {
                computed: bevy::camera::ComputedCameraValues {
                    target_info: Some(bevy::camera::RenderTargetInfo {
                        physical_size: UVec2::new(1920, 1080),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                ..default()
            },
        ));
        let wrapper = app
            .world_mut()
            .spawn(Node {
                width: px(1200),
                ..default()
            })
            .id();
        let root = app
            .world_mut()
            .spawn((
                Node {
                    width: percent(100),
                    ..default()
                },
                ChildOf(wrapper),
                Text::new(""),
                TextFont::from_font_size(40.0),
                TextColor::WHITE,
                RichTextSource::new(
                    r#"#ruby("A-lis")[Alice] and #ruby("Bob")[Robert]"#.into(),
                    &crate::ui::ScreenLayout::default(),
                ),
                RichGlyphs::default(),
            ))
            .id();
        for _ in 0..8 {
            app.update();
        }
        let world = app.world();
        let rich = world.get::<RichTextSource>(root).expect("rich source");
        let layout = world
            .get::<bevy::text::TextLayoutInfo>(root)
            .expect("base layout");
        assert_eq!(rich.document.text, "Alice and Robert");
        assert!(!layout.glyphs.is_empty(), "base glyphs must not disappear");
        assert!(layout.glyphs.iter().any(|glyph| glyph.section_index <= 5));
        assert!(layout.glyphs.iter().any(|glyph| glyph.section_index >= 11));
        assert!(
            world
                .get::<ComputedNode>(root)
                .expect("base bounds")
                .size()
                .y
                > 0.0
        );
        for &label in &rich.labels {
            assert!(
                !world
                    .get::<bevy::text::TextLayoutInfo>(label)
                    .expect("reading layout")
                    .glyphs
                    .is_empty()
            );
        }
    }

    #[test]
    fn text_sections_do_not_intercept_dialogue_or_button_picking() {
        use bevy::picking::{
            backend::{HitData, PointerHits},
            hover::{HoverMap, PointerCaptureMap, PreviousHoverMap, generate_hovermap},
            pointer::{PointerId, PointerInput},
        };
        let mut app = App::new();
        app.init_resource::<UiModels>()
            .init_resource::<HoverMap>()
            .init_resource::<bevy::picking::pointer::PointerMap>()
            .init_resource::<PreviousHoverMap>()
            .init_resource::<PointerCaptureMap>()
            .add_message::<PointerHits>()
            .add_message::<PointerInput>()
            .add_systems(Update, (update, generate_hovermap).chain());
        let root = app
            .world_mut()
            .spawn((
                Text::default(),
                Pickable::IGNORE,
                RichTextSource::new("*Alice*".into(), &crate::ui::ScreenLayout::default()),
                TextFont::default(),
                TextColor::WHITE,
            ))
            .id();
        app.update();
        let spans = app
            .world()
            .get::<RichTextSource>(root)
            .expect("rich text")
            .spans
            .clone();
        for &span in &spans {
            let pickable = app
                .world()
                .get::<Pickable>(span)
                .expect("text section picking policy");
            assert!(!pickable.is_hoverable && !pickable.should_block_lower);
        }
        let pointer = PointerId::Custom(uuid::Uuid::from_u128(1));
        app.world_mut().spawn(pointer);
        let surface = app
            .world_mut()
            .spawn((super::super::DialogueAdvanceSurface, Pickable::default()))
            .id();
        let button = app
            .world_mut()
            .spawn((
                Node::default(),
                bevy::ui_widgets::Button,
                Pickable::default(),
            ))
            .id();
        for blocking in [None, Some(button)] {
            let mut hits = vec![(spans[0], HitData::new(surface, 0.0, None, None))];
            if let Some(blocking) = blocking {
                hits.push((blocking, HitData::new(surface, 1.0, None, None)));
            }
            hits.push((surface, HitData::new(surface, 2.0, None, None)));
            app.world_mut()
                .write_message(PointerHits::new(pointer, hits, 0.0));
            app.update();
            let hover = app
                .world()
                .resource::<HoverMap>()
                .get(&pointer)
                .expect("hovered target");
            assert!(!hover.contains_key(&spans[0]));
            assert!(hover.contains_key(&blocking.unwrap_or(surface)));
            assert_eq!(hover.len(), 1);
        }
    }

    #[test]
    fn typst_styles_and_linebreaks_reach_bevy_spans() {
        let mut app = App::new();
        app.init_resource::<UiModels>().add_systems(Update, update);
        let root = app
            .world_mut()
            .spawn((
                RichTextSource::new(
                    "*Alice*#br#strike[Bob]~".into(),
                    &crate::ui::ScreenLayout::default(),
                ),
                TextFont::default(),
                TextColor::WHITE,
            ))
            .id();
        app.update();
        let spans = app
            .world()
            .get::<RichTextSource>(root)
            .expect("source")
            .spans
            .clone();
        assert_eq!(spans.len(), 10);
        assert_eq!(
            app.world().get::<TextFont>(spans[0]).expect("font").weight,
            bevy::text::FontWeight::BOLD
        );
        assert_eq!(
            app.world().get::<TextSpan>(spans[5]).expect("linebreak").0,
            "\n"
        );
        assert!(
            app.world()
                .get::<bevy::text::Strikethrough>(spans[6])
                .is_some()
        );
        assert_eq!(app.world().get::<TextSpan>(spans[9]).expect("tilde").0, "~");
        app.world_mut()
            .get_mut::<TextFont>(root)
            .expect("root font")
            .font_size = bevy::text::FontSize::Px(40.0);
        app.update();
        assert_eq!(
            app.world().get::<TextFont>(spans[0]).expect("font").weight,
            bevy::text::FontWeight::BOLD
        );
    }

    #[test]
    fn append_keeps_existing_entities_and_style_changes_reach_children() {
        let mut app = App::new();
        app.init_resource::<UiModels>().add_systems(Update, update);
        let root = app
            .world_mut()
            .spawn((
                RichTextSource::new(
                    "#ruby(\"reader\")[Alice]".into(),
                    &crate::ui::ScreenLayout::default(),
                ),
                TextFont::default(),
                TextColor::WHITE,
            ))
            .id();
        app.update();
        let source = app
            .world()
            .get::<RichTextSource>(root)
            .expect("rich source");
        let spans = source.spans.clone();
        let labels = source.labels.clone();
        assert_eq!(spans.len(), 5);
        assert_eq!(labels.len(), 1);
        app.world_mut()
            .get_mut::<RichTextSource>(root)
            .expect("rich source")
            .source
            .push_str(" and Bob");
        app.update();
        let source = app
            .world()
            .get::<RichTextSource>(root)
            .expect("appended source");
        assert!(source.spans.starts_with(&spans));
        assert_eq!(source.labels, labels);
        app.world_mut()
            .get_mut::<TextFont>(root)
            .expect("root font")
            .font_size = bevy::text::FontSize::Px(40.0);
        app.update();
        assert_eq!(
            app.world()
                .get::<TextFont>(spans[0])
                .expect("base font")
                .font_size,
            bevy::text::FontSize::Px(40.0)
        );
        assert_eq!(
            app.world()
                .get::<TextFont>(labels[0])
                .expect("reading font")
                .font_size,
            bevy::text::FontSize::Px(20.0)
        );
        app.world_mut()
            .get_mut::<RichTextSource>(root)
            .expect("rich source")
            .source = "#color(\"#ff0000\")[Bob]".into();
        app.update();
        assert!(
            spans
                .into_iter()
                .chain(labels)
                .all(|entity| app.world().get_entity(entity).is_err())
        );
        let colored = app
            .world()
            .get::<RichTextSource>(root)
            .expect("colored source")
            .spans[0];
        app.world_mut()
            .get_mut::<TextColor>(root)
            .expect("root color")
            .0 = Color::BLACK;
        app.update();
        assert_eq!(
            app.world()
                .get::<TextColor>(colored)
                .expect("explicit color")
                .0,
            Color::srgba_u8(255, 0, 0, 255)
        );
    }

    fn glyph(section_index: u32) -> bevy::text::PositionedGlyph {
        bevy::text::PositionedGlyph {
            position: Vec2::new(section_index as f32 * 20.0, 30.0),
            atlas_info: bevy::text::GlyphAtlasInfo {
                texture: Handle::<Image>::default().id(),
                rect: Rect::from_corners(Vec2::ZERO, Vec2::splat(10.0)),
                offset: Vec2::ZERO,
                is_alpha_mask: true,
            },
            section_index,
            line_index: 0,
        }
    }

    #[test]
    fn reveal_retains_full_layout_between_ticks_and_after_a_reshape() {
        let mut app = App::new();
        app.add_systems(PostUpdate, reveal_glyphs);
        let root = app
            .world_mut()
            .spawn((
                RichTextSource::new("Alice".into(), &crate::ui::ScreenLayout::default()),
                bevy::text::TextLayoutInfo {
                    glyphs: (1..=5).map(glyph).collect(),
                    run_geometry: (1..=5)
                        .map(|section_index| bevy::text::RunGeometry {
                            section_index,
                            ..default()
                        })
                        .collect(),
                    ..default()
                },
                RichGlyphs::default(),
            ))
            .id();
        for count in [0, 1, 3, 5, 2, 5] {
            app.world_mut()
                .get_mut::<RichTextSource>(root)
                .expect("source")
                .previous_count = count;
            app.update();
            assert_eq!(
                app.world()
                    .get::<bevy::text::TextLayoutInfo>(root)
                    .expect("layout")
                    .glyphs
                    .len(),
                count as usize
            );
            assert_eq!(
                app.world()
                    .get::<bevy::text::TextLayoutInfo>(root)
                    .expect("layout")
                    .run_geometry
                    .len(),
                count as usize
            );
            assert_eq!(
                app.world()
                    .get::<RichGlyphs>(root)
                    .expect("full cache")
                    .glyphs
                    .len(),
                5
            );
        }
        app.world_mut()
            .get_mut::<bevy::text::TextLayoutInfo>(root)
            .expect("layout")
            .glyphs = (1..=8).map(glyph).collect();
        app.update();
        assert_eq!(
            app.world()
                .get::<RichGlyphs>(root)
                .expect("reshaped cache")
                .glyphs
                .len(),
            8
        );
        assert_eq!(
            app.world()
                .get::<bevy::text::TextLayoutInfo>(root)
                .expect("visible layout")
                .glyphs
                .len(),
            5
        );
    }
}
