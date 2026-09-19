//! Baked presentation tracks. Script runs once; ECS samples the track using the mount clock.
use crate::render::ui_quad::{UiQuadMaterial, UiShaderSource, projection, shader_parameters};
use crate::ui::UiKeyframe;
use bevy::prelude::*;
use bevy::ui_render::ui_material::MaterialNode;
impl UiKeyframe {
    fn values(self) -> [f64; 8] {
        match self {
            Self::Rect(t, x, y, w, h, a) => [t, x, y, w, 0., 0., h, a],
            Self::Quad(t, x, y, ux, uy, vx, vy, a) => [t, x, y, ux, uy, vx, vy, a],
        }
    }
}
pub(crate) fn validate(frames: &[UiKeyframe]) -> bool {
    !frames.is_empty()
        && frames.iter().all(|frame| {
            let [t, x, y, ux, uy, vx, vy, a] = frame.values();
            [t, x, y, ux, uy, vx, vy, a].iter().all(|v| v.is_finite())
                && t >= 0.0
                && !matches!(frame, UiKeyframe::Rect(_,_,_,w,h,_) if *w < 0. || *h < 0.)
                && (0.0..=1.0).contains(&a)
        })
        && frames
            .windows(2)
            .all(|pair| pair[0].values()[0] < pair[1].values()[0])
}
fn sample(frames: &[UiKeyframe], elapsed: f64) -> [f64; 8] {
    let index = frames.partition_point(|frame| frame.values()[0] <= elapsed);
    if index == 0 {
        return frames[0].values();
    }
    let a = frames[index - 1].values();
    if index == frames.len() {
        return a;
    }
    let b = frames[index].values();
    let fraction = ((elapsed - a[0]) / (b[0] - a[0])).clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * fraction)
}
#[derive(Component)]
pub(crate) struct UiKeyframes {
    frames: Vec<UiKeyframe>,
    elapsed: f64,
    projected: bool,
}
#[derive(Component)]
pub(crate) struct Opacity(pub f32);
impl UiKeyframes {
    pub fn new(frames: &[UiKeyframe]) -> Self {
        Self {
            frames: frames.to_vec(),
            elapsed: 0.0,
            projected: frames
                .iter()
                .any(|frame| matches!(frame, UiKeyframe::Quad(..))),
        }
    }
}
pub(crate) fn tick(
    mut commands: Commands,
    clock: super::ui_timers::UiClock,
    mut tracks: Query<(
        Entity,
        &mut UiKeyframes,
        &mut Node,
        &mut Opacity,
        Option<&ImageNode>,
        Option<&MaterialNode<UiQuadMaterial>>,
        Option<&UiShaderSource>,
    )>,
    mut materials: Option<ResMut<Assets<UiQuadMaterial>>>,
    mut redraw: crate::redraw::Redraw,
) {
    for (entity, mut track, mut node, mut opacity, image, material, source) in &mut tracks {
        let delta = clock.delta(entity).as_secs_f64();
        if delta == 0.0 && track.elapsed > 0.0 {
            continue;
        }
        track.elapsed += delta;
        let [_, x, y, ux, uy, vx, vy, a] = sample(&track.frames, track.elapsed);
        let (bounds, u, v) = projection(
            Vec2::new(x as f32, y as f32),
            Vec2::new(ux as f32, uy as f32),
            Vec2::new(vx as f32, vy as f32),
        );
        node.left = px(bounds.min.x);
        node.top = px(bounds.min.y);
        node.width = px(bounds.width());
        node.height = px(bounds.height());
        if source.is_some() || track.projected {
            if let Some(materials) = materials.as_mut() {
                if let Some(mut material) = material.and_then(|m| materials.get_mut(&m.0)) {
                    material.uv_u = u;
                    material.uv_v = v;
                    material.clock = Vec4::new(track.elapsed as f32, 0., 0., 0.);
                    if let Some(source) = source {
                        material.parameters = shader_parameters(&source.keys, track.elapsed);
                    }
                } else if let Some(image) = image {
                    let aux = |i| {
                        source
                            .and_then(|s| s.textures.get(i))
                            .cloned()
                            .unwrap_or_else(|| image.image.clone())
                    };
                    let handle = materials.add(UiQuadMaterial {
                        sampling: UVec4::ZERO,
                        color: image.color.to_linear(),
                        uv_u: u,
                        uv_v: v,
                        image: image.image.clone(),
                        blend: source.map_or(crate::ui::UiShaderBlend::Alpha, |s| s.blend),
                        parameters: source
                            .map_or(Vec4::ZERO, |s| shader_parameters(&s.keys, track.elapsed)),
                        clock: Vec4::new(track.elapsed as f32, 0., 0., 0.),
                        texture1: aux(0),
                        texture2: aux(1),
                        texture3: aux(2),
                        shader: source.map(|s| s.shader.clone()),
                    });
                    commands
                        .entity(entity)
                        .remove::<ImageNode>()
                        .insert(MaterialNode(handle));
                }
            }
        }
        opacity.0 = a as f32;
        if delta > 0.0 {
            redraw.request();
        }
        if track.elapsed
            >= track
                .frames
                .last()
                .expect("validated nonempty keyframes")
                .values()[0]
        {
            commands.entity(entity).try_remove::<UiKeyframes>();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_and_clamps_track_samples() {
        let frames = [
            UiKeyframe::Rect(0.0, 0.0, 0.0, 10.0, 20.0, 0.0),
            UiKeyframe::Rect(1.0, 100.0, 50.0, 20.0, 40.0, 1.0),
        ];
        assert!(validate(&frames));
        assert_eq!(
            sample(&frames, 0.5),
            [0.5, 50.0, 25.0, 15.0, 0.0, 0.0, 30.0, 0.5]
        );
        assert_eq!(sample(&frames, 20.0), frames[1].values());
        assert!(!validate(&[]));
        assert!(!validate(&[frames[1], frames[0]]));
    }
}
