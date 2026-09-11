use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stage-local coordinates use Bevy axes and units: +X right, +Y up;
/// a camera with zero rotation looks along -Z. Rotation uses XYZ degrees.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagePose {
    #[serde(default)]
    pub position: (f32, f32, f32),
    #[serde(default)]
    pub rotation: (f32, f32, f32),
    #[serde(default = "unit_scale")]
    pub scale: (f32, f32, f32),
}
fn unit_scale() -> (f32, f32, f32) {
    (1.0, 1.0, 1.0)
}
impl StagePose {
    pub fn transform(&self) -> Transform {
        Transform::from_translation(Vec3::from(self.position))
            .with_rotation(Quat::from_euler(
                EulerRot::XYZ,
                self.rotation.0.to_radians(),
                self.rotation.1.to_radians(),
                self.rotation.2.to_radians(),
            ))
            .with_scale(Vec3::from(self.scale))
    }
    fn validate(&self) -> bool {
        Vec3::from(self.position).is_finite()
            && Vec3::from(self.rotation).is_finite()
            && Vec3::from(self.scale).is_finite()
            && Vec3::from(self.scale).min_element() > 0.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StageProjection {
    Orthographic { height: f32, near: f32, far: f32 },
    Perspective { fov: f32, near: f32, far: f32 },
}
impl StageProjection {
    pub fn projection(&self) -> Projection {
        match *self {
            Self::Orthographic { height, near, far } => {
                Projection::Orthographic(OrthographicProjection {
                    scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                        viewport_height: height,
                    },
                    near,
                    far,
                    ..OrthographicProjection::default_3d()
                })
            }
            Self::Perspective { fov, near, far } => {
                Projection::Perspective(PerspectiveProjection {
                    fov: fov.to_radians(),
                    near,
                    far,
                    ..default()
                })
            }
        }
    }
    fn validate(&self) -> bool {
        match *self {
            Self::Orthographic { height, near, far } => {
                height.is_finite()
                    && height > 0.0
                    && near.is_finite()
                    && far.is_finite()
                    && far > near
            }
            Self::Perspective { fov, near, far } => {
                fov.is_finite()
                    && fov > 0.0
                    && fov < 180.0
                    && near.is_finite()
                    && far.is_finite()
                    && near > 0.0
                    && far > near
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageCamera {
    pub pose: StageCameraPose,
    pub projection: StageProjection,
}
impl StageCamera {
    pub(super) fn validate(&self) -> bool {
        self.pose.validate() && self.projection.validate()
    }
}

/// An orbit interpolates its pivot, angles and radius, rather than taking a
/// straight-line shortcut through the scene between two camera transforms.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StageCameraPose {
    Fixed(StagePose),
    Orbit(StageOrbit),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageOrbit {
    pub pivot: (f32, f32, f32),
    pub rotation: (f32, f32, f32),
    pub distance: f32,
    /// Look away from the pivot (for views from inside a circular stage).
    #[serde(default)]
    pub outward: bool,
}
impl StageCameraPose {
    pub fn transform(&self) -> Transform {
        match self {
            Self::Fixed(pose) => pose.transform(),
            Self::Orbit(orbit) => {
                let rotation = Quat::from_euler(
                    EulerRot::YXZ,
                    orbit.rotation.1.to_radians(),
                    orbit.rotation.0.to_radians(),
                    orbit.rotation.2.to_radians(),
                );
                Transform::from_translation(
                    Vec3::from(orbit.pivot) + rotation * Vec3::Z * orbit.distance,
                )
                .with_rotation(if orbit.outward {
                    rotation * Quat::from_rotation_y(std::f32::consts::PI)
                } else {
                    rotation
                })
            }
        }
    }
    fn validate(&self) -> bool {
        match self {
            Self::Fixed(pose) => pose.validate() && pose.scale == unit_scale(),
            Self::Orbit(orbit) => {
                Vec3::from(orbit.pivot).is_finite()
                    && Vec3::from(orbit.rotation).is_finite()
                    && orbit.distance.is_finite()
                    && orbit.distance > 0.0
            }
        }
    }
}

#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageDefinition {
    pub scene: Option<String>,
    /// For pre-lit scenery; defaults to the materials authored in the model.
    #[serde(default)]
    pub unlit: bool,
    /// Linear camera exposure multiplier. None uses Bevy's physical default.
    #[serde(default)]
    pub exposure: Option<f32>,
    /// Optional camera-local white ambient light, in cd/m².
    #[serde(default)]
    pub ambient_brightness: Option<f32>,
    pub default_camera: String,
    #[serde(default)]
    pub anchors: BTreeMap<String, StagePose>,
    pub cameras: BTreeMap<String, StageCamera>,
    #[serde(skip)]
    #[dependency]
    pub(super) scene_handle: Option<Handle<WorldAsset>>,
}
impl StageDefinition {
    pub fn validate(&self) -> Result<(), String> {
        if self.ambient_brightness.is_some_and(|value| !value.is_finite() || value < 0.0) {
            return Err("ambientBrightness must be finite and non-negative".into());
        }
        if self.exposure.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return Err("exposure must be finite and greater than zero".into());
        }
        if self
            .scene
            .as_ref()
            .is_some_and(|path| path.trim().is_empty())
        {
            return Err("scene path must not be empty".into());
        }
        if !self.cameras.contains_key(&self.default_camera) {
            return Err(format!(
                "defaultCamera `{}` is not defined",
                self.default_camera
            ));
        }
        for (name, pose) in &self.anchors {
            if name.trim().is_empty() || !pose.validate() {
                return Err(format!("invalid anchor `{name}`"));
            }
        }
        for (name, camera) in &self.cameras {
            if name.trim().is_empty() || !camera.validate() {
                return Err(format!("invalid camera `{name}`"));
            }
        }
        Ok(())
    }

    pub(super) fn camera_exposure(&self) -> bevy::camera::Exposure {
        self.exposure.map_or_else(bevy::camera::Exposure::default, |value| bevy::camera::Exposure {
            ev100: -((value as f64) * 1.2).log2() as f32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = r#".{ scene: null, defaultCamera: "wide",
        anchors: .{ alice: .{ position: (1, 2, 3), rotation: (0, 90, 0) } },
        cameras: .{ wide: .{ pose: .{ position: (0, 2, 10) },
            projection: .{ kind: "perspective", fov: 60, near: 0.1, far: 100 } } }
    }"#;
    #[test]
    fn hson_preserves_typed_spatial_data_and_validates_references() {
        let mut stage: StageDefinition = hiraku_script::hson::from_str(SOURCE).expect("stage HSON");
        stage.validate().expect("valid stage");
        let anchor = stage.anchor("alice").expect("named anchor");
        assert_eq!(anchor.translation, Vec3::new(1.0, 2.0, 3.0));
        assert!((anchor.rotation * Vec3::NEG_Z - Vec3::NEG_X).length() < 0.0001);
        assert!(stage.anchor("bob").is_err());
        let encoded = hiraku_script::hson::to_string(&stage).expect("serialize definition");
        let restored: StageDefinition =
            hiraku_script::hson::from_str(&encoded).expect("deserialize definition");
        restored.validate().expect("valid restored stage");
        stage.default_camera = "missing".into();
        assert!(
            stage
                .validate()
                .expect_err("bad reference")
                .contains("defaultCamera")
        );
    }

    #[test]
    fn linear_exposure_preserves_units_and_rejects_invalid_values() {
        let mut stage: StageDefinition = hiraku_script::hson::from_str(SOURCE).expect("stage");
        assert_eq!(stage.camera_exposure().ev100, bevy::camera::Exposure::default().ev100);
        for factor in [0.25, 1.0, 4.0] {
            stage.exposure = Some(factor);
            stage.validate().expect("positive exposure");
            assert!((stage.camera_exposure().exposure() - factor).abs() < 0.0001);
        }
        for factor in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            stage.exposure = Some(factor);
            assert!(stage.validate().is_err());
        }
    }
    #[test]
    fn ambient_override_is_optional_and_non_negative() {
        let mut stage: StageDefinition = hiraku_script::hson::from_str(SOURCE).expect("stage");
        assert!(stage.ambient_brightness.is_none());
        for value in [0.0, 0.08, 80.0] {
            stage.ambient_brightness = Some(value);
            stage.validate().expect("valid ambient brightness");
        }
        for value in [-1.0, f32::NAN, f32::INFINITY] {
            stage.ambient_brightness = Some(value);
            assert!(stage.validate().is_err());
        }
    }
    #[test]
    fn invalid_projection_and_camera_scale_are_rejected() {
        for replacement in ["fov: 180", "fov: 0"] {
            let stage: StageDefinition =
                hiraku_script::hson::from_str(&SOURCE.replace("fov: 60", replacement))
                    .expect("structural data");
            assert!(stage.validate().is_err());
        }
        let mut stage: StageDefinition = hiraku_script::hson::from_str(SOURCE).expect("stage");
        let StageCameraPose::Fixed(pose) = &mut stage.cameras.get_mut("wide").expect("preset").pose
        else {
            panic!("fixed camera");
        };
        pose.scale = (2.0, 2.0, 2.0);
        assert!(stage.validate().is_err());
    }

    #[test]
    fn stage_owns_anchors_without_creating_render_cameras() {
        let stage: StageDefinition = hiraku_script::hson::from_str(SOURCE).expect("stage");
        let mut world = World::new();
        let root = stage
            .spawn(&mut world.commands())
            .expect("instantiate spatial stage");
        world.flush();
        let anchor = world
            .query::<(Entity, &super::super::StageAnchor)>()
            .iter(&world)
            .find(|(_, name)| name.0 == "alice")
            .map(|(entity, _)| entity)
            .expect("named child anchor");
        assert_eq!(
            world.get::<ChildOf>(anchor).expect("owned anchor").parent(),
            root
        );
        assert_eq!(world.query::<&Camera>().iter(&world).count(), 0);
        world.entity_mut(root).despawn();
        assert!(world.get_entity(anchor).is_err());
    }
}
