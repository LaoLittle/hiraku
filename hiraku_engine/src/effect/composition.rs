//! Composition crossfades, separate from per-picture opacity.
//!
//! A branch is a presentation description or render-target handle, never a
//! decoder or a copied pixel buffer. The owner controls readiness, scene time,
//! easing and animation completion. Only advance progress after both inputs
//! are renderable; building a target does not advance a transition.
use bevy::{pbr::Material, prelude::*, render::render_resource::AsBindGroup, shader::ShaderRef};
use serde::{Deserialize, Serialize};

/// Must run before MaterialPlugin: Bevy requests material shaders during build.
pub(crate) fn load_internal_shaders(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/composition_crossfade.wesl");
}

/// A blend of immutable composition references. Interruptions retain the exact
/// visible mixture rather than selecting either unfinished endpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompositionBlend<T> {
    from: Vec<(T, f32)>,
    target: T,
    progress: f32,
}

impl<T: Clone + PartialEq> CompositionBlend<T> {
    pub fn new(value: T) -> Self {
        Self {
            from: Vec::new(),
            target: value,
            progress: 1.0,
        }
    }

    /// Begin from the currently presented mixture. Readiness and elapsed time
    /// belong to the caller; no clock or callback is hidden in this operation.
    pub fn begin(&mut self, target: T) {
        let mut from: Vec<(T, f32)> = Vec::new();
        for (source, weight) in self.layers() {
            if let Some((_, existing)) = from.iter_mut().find(|(item, _)| item == source) {
                *existing += weight;
            } else {
                from.push((source.clone(), weight));
            }
        }
        self.from = from;
        self.target = target;
        self.progress = 0.0;
    }

    /// Supply an eased blend weight, not a duration. Overshooting curves must
    /// be clamped by their owner: opacity weights cannot be negative.
    pub fn set_progress(&mut self, progress: f32) -> Result<(), &'static str> {
        if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
            return Err("composition progress must be finite and within 0..=1");
        }
        if self.from.is_empty() && progress != 1.0 {
            return Err("cannot rewind a completed composition; begin a new transition");
        }
        self.progress = progress;
        if progress == 1.0 {
            self.from.clear();
        }
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.from.is_empty()
    }

    /// Nonzero, normalized weights for linear premultiplied RGBA accumulation.
    /// Branches must not be drawn with ordinary source-over alpha blending.
    pub fn layers(&self) -> impl Iterator<Item = (&T, f32)> {
        self.from
            .iter()
            .map(|(item, weight)| (item, weight * (1.0 - self.progress)))
            .chain(std::iter::once((&self.target, self.progress)))
            .filter(|(_, weight)| *weight > 0.0)
    }

    /// Validate a deserialized snapshot before presenting any branch.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.progress.is_finite() || !(0.0..=1.0).contains(&self.progress) {
            return Err("invalid saved composition progress");
        }
        if self.from.is_empty() {
            return if self.progress == 1.0 {
                Ok(())
            } else {
                Err("missing source composition")
            };
        }
        if self.progress == 1.0
            || self.from.iter().any(|(_, w)| !w.is_finite() || *w <= 0.0)
            || (self.from.iter().map(|(_, w)| *w).sum::<f32>() - 1.0).abs() > 0.00001
        {
            return Err("invalid saved composition weights");
        }
        Ok(())
    }
}

/// Mix two *already composed*, linear, premultiplied render targets.
/// This is not a raw image crossfade: inputs must have identical viewport and
/// transparent clear color. Their own transforms/effects happen before mixing.
/// The output uses premultiplied source-over when mounted above another layer.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct CompositionCrossfadeMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub from: Handle<Image>,
    #[texture(2)]
    pub to: Handle<Image>,
    /// X is the blend weight; remaining components are reserved and zero.
    #[uniform(3)]
    progress: Vec4,
}

impl CompositionCrossfadeMaterial {
    pub fn new(from: Handle<Image>, to: Handle<Image>) -> Self {
        Self {
            from,
            to,
            progress: Vec4::ZERO,
        }
    }

    pub fn set_progress(&mut self, progress: f32) -> Result<(), &'static str> {
        if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
            return Err("composition progress must be finite and within 0..=1");
        }
        self.progress.x = progress;
        Ok(())
    }

    pub fn progress(&self) -> f32 {
        self.progress.x
    }
}

impl Material for CompositionCrossfadeMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_engine/effect/shaders/composition_crossfade.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
    fn enable_shadows() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_shader_is_readable_before_post_process_plugin_is_built() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        load_internal_shaders(&mut app);
        let ShaderRef::Path(path) = CompositionCrossfadeMaterial::fragment_shader() else {
            panic!("composition material uses an embedded path");
        };
        let server = app.world().resource::<AssetServer>();
        let source = server
            .get_source(path.source())
            .expect("embedded asset source");
        let bytes = bevy::tasks::block_on(async {
            let mut reader = source
                .reader()
                .read(path.path())
                .await
                .expect("shader registered before material initialization");
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .await
                .expect("embedded shader bytes");
            bytes
        });
        assert_eq!(bytes, include_bytes!("shaders/composition_crossfade.wesl"));
    }

    fn weights(blend: &CompositionBlend<String>) -> Vec<(&str, f32)> {
        blend.layers().map(|(s, w)| (s.as_str(), w)).collect()
    }

    #[test]
    fn interrupted_fade_preserves_the_visible_mixture() {
        let mut blend = CompositionBlend::new("alice".to_owned());
        blend.begin("bob".into());
        blend.set_progress(0.25).expect("progress");
        let visible = weights(&blend)
            .into_iter()
            .map(|(s, w)| (s.to_owned(), w))
            .collect::<Vec<_>>();
        blend.begin("room".into());
        assert_eq!(
            blend
                .layers()
                .map(|(s, w)| (s.clone(), w))
                .collect::<Vec<_>>(),
            visible
        );
        blend.set_progress(0.5).expect("progress");
        assert_eq!(
            weights(&blend),
            [("alice", 0.375), ("bob", 0.125), ("room", 0.5)]
        );
        blend.validate().expect("normalized composition");
    }

    #[test]
    fn snapshots_retain_interrupted_weights_and_completion_releases_sources() {
        let mut blend = CompositionBlend::new("alice".to_owned());
        blend.begin("bob".into());
        blend.set_progress(0.25).expect("progress");
        blend.begin("alice".into());
        blend.set_progress(0.5).expect("progress");
        let encoded = hiraku_script::hson::to_string(&blend).expect("encode");
        let mut restored: CompositionBlend<String> =
            hiraku_script::hson::from_str(&encoded).expect("decode");
        restored.validate().expect("saved mixture");
        assert_eq!(restored, blend);
        restored.begin("room".into());
        assert_eq!(weights(&restored), [("alice", 0.875), ("bob", 0.125)]);
        restored.set_progress(1.0).expect("complete");
        assert!(restored.is_complete());
        assert!(restored.from.is_empty());
        assert_eq!(weights(&restored), [("room", 1.0)]);
    }

    #[test]
    fn invalid_progress_does_not_modify_presentation() {
        let mut blend = CompositionBlend::new("alice".to_owned());
        assert!(blend.set_progress(0.0).is_err());
        blend.begin("bob".into());
        let previous = blend.clone();
        for progress in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(blend.set_progress(progress).is_err());
            assert_eq!(blend, previous);
        }
        blend.from[0].1 = 0.5;
        assert!(blend.validate().is_err());
    }

    #[test]
    fn crossfade_uses_premultiplied_output_without_a_second_alpha_multiply() {
        let material = CompositionCrossfadeMaterial {
            from: Handle::default(),
            to: Handle::default(),
            progress: Vec4::ZERO,
        };
        assert_eq!(material.alpha_mode(), AlphaMode::Premultiplied);
        assert!(!CompositionCrossfadeMaterial::enable_shadows());
    }
}
