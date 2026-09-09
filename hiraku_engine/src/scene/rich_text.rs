//! Ruby uses ordinary Bevy text spans and glyph layout, not a second camera.
use bevy::prelude::*;
use crate::{rich_text::{RichText, Ruby}, ui::{PropertyComputation, UiModels}};

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
    previous_count: usize,
}

impl RichTextSource {
    pub fn new(source: String, layout: &crate::ui::ScreenLayout) -> Self {
        Self { source, count: layout.text_reveal, expression: layout.reactive_text_reveal.clone(),
            revision: u64::MAX, rendered: None, document: RichText::default(), spans: Vec::new(),
            labels: Vec::new(), previous_count: 0 }
    }
}

#[derive(Component)]
struct RubyLabel { root: Entity, range: Ruby, positioned: bool }

pub(crate) fn update(
    mut commands: Commands,
    mut redraw: crate::redraw::Redraw,
    models: Res<UiModels>,
    parents: Query<&ChildOf>,
    locals: Query<&super::widgets::UiLocalState>,
    mut roots: Query<(Entity, &mut RichTextSource, &TextFont, &TextColor, Option<&TextShadow>)>,
) {
    for (entity, mut rich, font, color, shadow) in &mut roots {
        if let Some(mut expression) = rich.expression.take() {
            let local_changed = super::screen_ui::refresh_local_binding(entity, &mut expression, &parents, &locals);
            if rich.revision != models.revision() || local_changed {
                use hiraku_script::FromHksValue;
                match crate::script::evaluate_ui_reactive_binding(&expression, &models).map_err(|e| e.to_string())
                    .and_then(|value| u32::from_hks_value(&value).map_err(|e| e.to_string())) {
                    Ok(count) => rich.count = Some(count),
                    Err(error) => crate::script::emit_script_diagnostic("rich text reveal failed", &error.to_string()),
                }
                rich.revision = models.revision();
            }
            rich.expression = Some(expression);
        }
        if rich.rendered.as_ref() != Some(&rich.source) {
            let document = match crate::rich_text::parse(&rich.source) {
                Ok(value) => value,
                Err(error) => {
                    crate::script::emit_script_diagnostic("invalid rich text", &error);
                    // Keep malformed text readable, never panic in a UI system.
                    RichText { text: rich.source.clone(), ruby: Vec::new() }
                }
            };
            let append = document.text.starts_with(&rich.document.text)
                && document.ruby.starts_with(&rich.document.ruby);
            if !append {
                for child in std::mem::take(&mut rich.spans).into_iter().chain(std::mem::take(&mut rich.labels)) {
                    commands.entity(child).try_despawn();
                }
                rich.previous_count = 0;
            }
            let old_len = rich.spans.len();
            let shown = rich.count.map_or(usize::MAX, |n| n as usize);
            for (index, ch) in document.text.chars().enumerate().skip(old_len) {
                // Word joiners keep a ruby base on one line without entering
                // the stored text or the dialogue character counter.
                let joined = document.ruby.iter().any(|r| r.start <= index && index + 1 < r.end);
                let text = if joined { format!("{ch}\u{2060}") } else { ch.to_string() };
                let tint = if index < shown { color.0 } else { color.0.with_alpha(0.0) };
                let span = commands.spawn((TextSpan::new(text), font.clone(), TextColor(tint),
                    bevy::text::LineHeight::RelativeToFont(1.8))).id();
                commands.entity(entity).add_child(span);
                rich.spans.push(span);
            }
            for range in document.ruby.iter().skip(rich.labels.len()) {
                let mut reading_font = font.clone();
                if let bevy::text::FontSize::Px(size) = font.font_size {
                    reading_font.font_size = bevy::text::FontSize::Px(size * 0.5);
                }
                let label = commands.spawn((
                    RubyLabel { root: entity, range: range.clone(), positioned: false },
                    Node { position_type: PositionType::Absolute, flex_shrink: 0.0, ..default() },
                    Text::new(range.reading.clone()), reading_font, *color,
                    TextLayout::new(Justify::Left, bevy::text::LineBreak::NoWrap),
                    shadow.cloned().unwrap_or_default(), Visibility::Hidden, Pickable::IGNORE,
                )).id();
                commands.entity(entity).add_child(label);
                rich.labels.push(label);
            }
            rich.document = document;
            rich.rendered = Some(rich.source.clone());
            redraw.request();
        }
        let count = rich.count.map_or(rich.spans.len(), |n| (n as usize).min(rich.spans.len()));
        if count != rich.previous_count {
            for index in count.min(rich.previous_count)..count.max(rich.previous_count) {
                if let Some(&span) = rich.spans.get(index) {
                    commands.entity(span).insert(TextColor(if index < count { color.0 } else { color.0.with_alpha(0.0) }));
                }
            }
            rich.previous_count = count;
            redraw.request();
        }
    }
}

/// Positions come from the font shaper, not guessed character widths. A label
/// remains hidden until its new Node position has passed through UI layout.
pub(crate) fn position_ruby(
    mut redraw: crate::redraw::Redraw,
    roots: Query<(&RichTextSource, &bevy::text::TextLayoutInfo)>,
    mut labels: Query<(&mut RubyLabel, &bevy::text::TextLayoutInfo, &mut Node, &mut Visibility)>,
) {
    for (mut label, reading, mut node, mut visibility) in &mut labels {
        let Ok((rich, layout)) = roots.get(label.root) else { continue };
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        for glyph in &layout.glyphs {
            if glyph.section_index > label.range.start && glyph.section_index <= label.range.end {
                let half = glyph.atlas_info.rect.size() * 0.5;
                min = min.min((glyph.position - half) / layout.scale_factor);
                max = max.max((glyph.position + half) / layout.scale_factor);
            }
        }
        if !min.is_finite() || reading.size.x <= 0.0 { continue }
        let left = px((min.x + max.x - reading.size.x) * 0.5);
        let top = px(min.y - reading.size.y);
        let moved = node.left != left || node.top != top;
        if moved {
            node.left = left;
            node.top = top;
            label.positioned = false;
        } else { label.positioned = true; }
        let visible = label.positioned && rich.previous_count > label.range.start;
        let next = if visible { Visibility::Inherited } else { Visibility::Hidden };
        if *visibility != next { *visibility = next; redraw.request(); }
        if moved { redraw.request(); }
    }
}
