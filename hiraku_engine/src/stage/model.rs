//! Instance-local model adaptation. Neither imported assets nor shared
//! materials are rewritten; overrides belong to the owning stage.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageMaterial {
    /// sRGB RGBA factors. Texture handles remain those of the imported model.
    pub base_color: Option<[f32; 4]>,
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    pub double_sided: Option<bool>,
    pub unlit: Option<bool>,
    pub alpha: Option<StageAlpha>,
    /// UV transform in the imported model's coordinate convention.
    pub uv_scale: Option<(f32, f32)>,
    pub uv_offset: Option<(f32, f32)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StageAlpha {
    Opaque,
    Blend,
    Mask { cutoff: f32 },
}

impl StageMaterial {
    pub(super) fn validate(&self) -> bool {
        let unit = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        self.base_color.is_none_or(|v| v.into_iter().all(unit))
            && self.metallic.is_none_or(unit) && self.roughness.is_none_or(unit)
            && match self.alpha { Some(StageAlpha::Mask { cutoff }) => unit(cutoff), _ => true }
            && self.uv_scale.is_none_or(|v| Vec2::from(v).is_finite())
            && self.uv_offset.is_none_or(|v| Vec2::from(v).is_finite())
    }

    pub(super) fn apply(&self, material: &mut StandardMaterial) {
        if let Some([r, g, b, a]) = self.base_color { material.base_color = Color::srgba(r, g, b, a); }
        if let Some(v) = self.metallic { material.metallic = v; }
        if let Some(v) = self.roughness { material.perceptual_roughness = v; }
        if let Some(v) = self.unlit { material.unlit = v; }
        if let Some(v) = self.double_sided {
            material.double_sided = v;
            material.cull_mode = if v { None } else { Some(bevy::render::render_resource::Face::Back) };
        }
        if let Some(v) = &self.alpha {
            material.alpha_mode = match *v {
                StageAlpha::Opaque => AlphaMode::Opaque,
                StageAlpha::Blend => AlphaMode::Blend,
                StageAlpha::Mask { cutoff } => AlphaMode::Mask(cutoff),
            };
        }
        if self.uv_scale.is_some() || self.uv_offset.is_some() {
            material.uv_transform = bevy::math::Affine2::from_scale_angle_translation(
                self.uv_scale.map(Vec2::from).unwrap_or(Vec2::ONE), 0.0,
                self.uv_offset.map(Vec2::from).unwrap_or(Vec2::ZERO),
            );
        }
    }
}

/// A light attached to a named imported node. Pose is relative to that node;
/// this also supports models whose exporter retained only empty light markers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageLight {
    pub pose: super::StagePose,
    pub light: StageLightKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StageLightKind {
    /// Intensity is lumens, range/radius are world units, cone angles degrees.
    Spot {
        color: [f32; 3], intensity: f32, range: f32, radius: f32,
        #[serde(rename = "innerAngle")] inner_angle: f32,
        #[serde(rename = "outerAngle")] outer_angle: f32,
    },
}
impl StageLight {
    pub(super) fn validate(&self) -> bool {
        let StageLightKind::Spot { color, intensity, range, radius, inner_angle, outer_angle } = self.light;
        self.pose.validate()
            && color.into_iter().all(|c| c.is_finite() && (0.0..=1.0).contains(&c))
            && intensity.is_finite() && intensity >= 0.0
            && range.is_finite() && range > 0.0 && radius.is_finite() && radius >= 0.0
            && inner_angle.is_finite() && outer_angle.is_finite()
            && 0.0 <= inner_angle && inner_angle < outer_angle && outer_angle < 90.0
    }

    pub(super) fn spawn(&self, commands: &mut Commands, parent: Entity) {
        let StageLightKind::Spot { color: [r, g, b], intensity, range, radius, inner_angle, outer_angle } = self.light;
        commands.spawn((
            SpotLight { color: Color::srgb(r, g, b), intensity, range, radius,
                inner_angle: inner_angle.to_radians(), outer_angle: outer_angle.to_radians(), ..default() },
            self.pose.transform(), ChildOf(parent),
            super::runtime::StageSurface, super::views::spatial_layer(),
        ));
    }
}
