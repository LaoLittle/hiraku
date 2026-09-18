//! Ruby uses ordinary Bevy text spans and glyph layout, not a second camera.
use crate::{
    rich_text::{RichText, Ruby},
    ui::{PropertyComputation, UiModels},
};

fn span_color(document: &RichText, index: u32, fallback: TextColor) -> TextColor {
    document
        .colors
        .get(index as usize)
        .copied()
        .flatten()
        .map_or(fallback, |[r, g, b, a]| {
            TextColor(Color::srgba_u8(r, g, b, a))
        })
}
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
            cache.scale = layout.scale_factor;
        }
        if shaped || cache.shown != Some(rich.previous_count) {
            layout.glyphs = cache
                .glyphs
                .iter()
                .filter(|glyph| glyph.section_index <= rich.previous_count)
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
    )>,
) {
    for (entity, mut rich, font, color, shadow) in &mut roots {
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
                        .and_then(|value| u32::from_hks_value(&value).map_err(|e| e.to_string()))
                    {
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
            let document = match crate::rich_text::parse(&rich.source) {
                Ok(value) => value,
                Err(error) => {
                    crate::script::emit_script_diagnostic("invalid rich text", &error);
                    // Keep malformed text readable, never panic in a UI system.
                    RichText {
                        text: rich.source.clone(),
                        ruby: Vec::new(),
                        colors: vec![None; rich.source.chars().count()],
                    }
                }
            };
            let append = document.text.starts_with(&rich.document.text)
                && document.ruby.starts_with(&rich.document.ruby)
                && document.colors.starts_with(&rich.document.colors);
            if !append {
                for child in std::mem::take(&mut rich.spans)
                    .into_iter()
                    .chain(std::mem::take(&mut rich.labels))
                {
                    commands.entity(child).try_despawn();
                }
                rich.previous_count = 0;
            }
            let old_len = rich.spans.len();
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
                        (*font).clone(),
                        span_color(&document, index, *color),
                        bevy::text::LineHeight::RelativeToFont(if annotated { 1.8 } else { 1.2 }),
                    ))
                    .id();
                commands.entity(entity).add_child(span);
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
                commands.entity(entity).add_child(label);
                rich.labels.push(label);
            }
            rich.document = document;
            rich.rendered = Some(rich.source.clone());
            redraw.request();
        }
        if font.is_changed()
            || color.is_changed()
            || shadow.as_ref().is_some_and(|s| s.is_changed())
        {
            for (index, &span) in rich.spans.iter().enumerate() {
                commands.entity(span).insert((
                    (*font).clone(),
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
        let count = rich
            .count
            .map_or(rich.spans.len(), |n| (n as usize).min(rich.spans.len()))
            as u32;
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
    roots: Query<(&RichTextSource, &RichGlyphs)>,
    mut labels: Query<(
        &mut RubyLabel,
        &bevy::text::TextLayoutInfo,
        &mut Node,
        &mut Visibility,
    )>,
) {
    for (mut label, reading, mut node, mut visibility) in &mut labels {
        let Ok((rich, layout)) = roots.get(label.root) else {
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
        let left = px((min.x + max.x - reading.size.x) * 0.5);
        let top = px(min.y - reading.size.y);
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
    fn append_keeps_existing_entities_and_style_changes_reach_children() {
        let mut app = App::new();
        app.init_resource::<UiModels>().add_systems(Update, update);
        let root = app
            .world_mut()
            .spawn((
                RichTextSource::new(
                    "{ruby:reader}Alice{/ruby}".into(),
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
            .source = "{color:#ff0000}Bob{/color}".into();
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
