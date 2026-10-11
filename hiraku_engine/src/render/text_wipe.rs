use bevy::{
    prelude::*,
    render::{Extract, ExtractSchedule, RenderApp},
    ui_render::{ExtractedGlyph, ExtractedUiItem, ExtractedUiNodes},
};

#[derive(Clone, PartialEq)]
pub(crate) struct GlyphWipe {
    pub rect: Rect,
    pub position: Vec2,
    pub progress: f32,
}

#[derive(Component, Clone, Default, PartialEq)]
pub(crate) struct TextWipe(pub Vec<GlyphWipe>);

pub(crate) fn register(app: &mut App) {
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            ExtractSchedule,
            extract_wipes
                .after(bevy::ui_render::extract_text_sections)
                .after(bevy::ui_render::extract_text_shadows),
        );
    }
}

fn coverage(x: f32, width: f32, progress: f32) -> f32 {
    let feather = (width * 0.2).clamp(1.0, 12.0);
    let edge = (width + feather) * progress.clamp(0.0, 1.0) - feather * 0.5;
    let t = ((x - edge + feather * 0.5) / feather).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

fn slice(glyph: &ExtractedGlyph, left: f32, right: f32, alpha: f32) -> ExtractedGlyph {
    let mut color = glyph.color;
    color.alpha *= alpha;
    ExtractedGlyph {
        color,
        translation: glyph.translation + Vec2::new((left + right - glyph.rect.width()) * 0.5, 0.0),
        rect: Rect::new(
            glyph.rect.min.x + left,
            glyph.rect.min.y,
            glyph.rect.min.x + right,
            glyph.rect.max.y,
        ),
    }
}

fn wipe_glyph(glyph: &ExtractedGlyph, progress: f32, output: &mut Vec<ExtractedGlyph>) {
    let width = glyph.rect.width();
    if width <= 0.0 || progress <= 0.0 {
        return;
    }
    if progress >= 1.0 {
        output.push(slice(glyph, 0.0, width, 1.0));
        return;
    }
    let feather = (width * 0.2).clamp(1.0, 12.0);
    let edge = (width + feather) * progress - feather * 0.5;
    let start = (edge - feather * 0.5).clamp(0.0, width);
    let end = (edge + feather * 0.5).clamp(0.0, width);
    if start > 0.0 {
        output.push(slice(glyph, 0.0, start, 1.0));
    }
    let bands = ((end - start) * 2.0).ceil().clamp(1.0, 32.0) as u32;
    for index in 0..bands {
        let left = start + (end - start) * index as f32 / bands as f32;
        let right = start + (end - start) * (index + 1) as f32 / bands as f32;
        if right > left {
            output.push(slice(
                glyph,
                left,
                right,
                coverage((left + right) * 0.5, width, progress),
            ));
        }
    }
}

fn extract_wipes(wipes: Extract<Query<(Entity, &TextWipe)>>, mut nodes: ResMut<ExtractedUiNodes>) {
    for (entity, wipe) in &wipes {
        let entity: bevy::render::sync_world::MainEntity = entity.into();
        if !nodes.changed.contains(&entity) {
            continue;
        }
        let Some((_, batches)) = nodes.uinodes.get_mut(&entity) else {
            continue;
        };
        for (_, node) in batches.iter_mut() {
            let ExtractedUiItem::Glyphs { glyphs } = &mut node.item else {
                continue;
            };
            if !glyphs.iter().any(|glyph| {
                wipe.0
                    .iter()
                    .any(|w| w.rect == glyph.rect && w.position == glyph.translation)
            }) {
                continue;
            }
            let mut output = Vec::with_capacity(glyphs.len() + 32);
            for glyph in std::mem::take(glyphs) {
                if let Some(w) = wipe
                    .0
                    .iter()
                    .find(|w| w.rect == glyph.rect && w.position == glyph.translation)
                {
                    wipe_glyph(&glyph, w.progress, &mut output);
                } else {
                    output.push(glyph);
                }
            }
            *glyphs = output;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_splits_one_glyph_and_preserves_atlas_geometry_and_tint() {
        let glyph = ExtractedGlyph {
            color: LinearRgba::new(0.2, 0.4, 0.8, 0.6),
            translation: Vec2::new(50.0, 60.0),
            rect: Rect::new(100.0, 200.0, 120.0, 230.0),
        };
        let mut output = Vec::new();
        wipe_glyph(&glyph, 0.5, &mut output);
        assert!(
            output
                .iter()
                .any(|g| g.color.alpha > 0.0 && g.color.alpha < 0.6)
        );
        assert!(output.iter().any(|g| g.color.alpha == 0.6));
        for g in &output {
            assert_eq!(g.color.red, glyph.color.red);
            assert_eq!(g.color.green, glyph.color.green);
            assert_eq!(g.color.blue, glyph.color.blue);
            assert_eq!(g.rect.height(), glyph.rect.height());
            assert!(
                (g.translation.x
                    - g.rect.center().x
                    - (glyph.translation.x - glyph.rect.center().x))
                    .abs()
                    < 1e-5
            );
        }
        assert_eq!(coverage(10.0, 20.0, 0.5), 0.5);
        assert_eq!(coverage(0.0, 20.0, 0.0), 0.0);
        assert_eq!(coverage(20.0, 20.0, 1.0), 1.0);
        output.clear();
        wipe_glyph(&glyph, 0.0, &mut output);
        assert!(output.is_empty());
        wipe_glyph(&glyph, 1.0, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].rect, glyph.rect);
        assert_eq!(output[0].color, glyph.color);
    }
}
