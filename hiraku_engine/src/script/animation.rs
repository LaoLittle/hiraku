use hiraku_script::native::{NativeError, NativeRegistry, RegistrationError};
use serde::{Deserialize, Serialize};

/// Common `.time(seconds)` boundary for effects whose host representation is
/// milliseconds. Keep validation and rounding independent of builder family.
pub(crate) fn duration_millis(seconds: f64) -> Result<u64, NativeError> {
    AnimationSpec::Linear(0.0, false).with_time(seconds)?;
    Ok((seconds * 1000.0).round() as u64)
}

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Easing {
 Linear, Ease, EaseIn, EaseOut, EaseInOut, SmoothStep, EaseOutSine, EaseInOutSine, Bounce, EaseOutBack,
 Spring,
 EaseInQuad,
 EaseOutQuad,
 EaseInOutQuad,
 EaseInCubic,
 EaseOutCubic,
 EaseInOutCubic,
 EaseInQuart,
 EaseOutQuart,
 EaseInOutQuart,
 EaseInQuint,
 EaseOutQuint,
 EaseInOutQuint,
 EaseInSine,
 EaseInExpo,
 EaseOutExpo,
 EaseInOutExpo,
 EaseInCirc,
 EaseOutCirc,
 EaseInOutCirc,
 EaseInBounce,
 EaseOutBounce,
 EaseInOutBounce,
 EaseInBack,
 EaseInOutBack,
 EaseInElastic,
 EaseOutElastic,
 EaseInOutElastic,
 CubicBezier(f64, f64, f64, f64),
}
#[allow(non_snake_case)]
impl Easing {
 #[getter]
 fn linear() -> Easing { Self::Linear }
 #[getter]
 fn ease() -> Easing { Self::Ease }
 #[getter]
 fn easeIn() -> Easing { Self::EaseIn }
 #[getter]
 fn easeOut() -> Easing { Self::EaseOut }
 #[getter]
 fn easeInOut() -> Easing { Self::EaseInOut }
 #[getter]
 fn smoothStep() -> Easing { Self::SmoothStep }
 #[getter]
 fn easeOutSine() -> Easing { Self::EaseOutSine }
 #[getter]
 fn easeInOutSine() -> Easing { Self::EaseInOutSine }
 #[getter]
 fn bounce() -> Easing { Self::Bounce }
 #[getter]
 fn easeOutBack() -> Easing { Self::EaseOutBack }
 #[getter]
 fn spring() -> Easing { Self::Spring }
 #[getter]
 fn easeInQuad() -> Easing { Self::EaseInQuad }
 #[getter]
 fn easeOutQuad() -> Easing { Self::EaseOutQuad }
 #[getter]
 fn easeInOutQuad() -> Easing { Self::EaseInOutQuad }
 #[getter]
 fn easeInCubic() -> Easing { Self::EaseInCubic }
 #[getter]
 fn easeOutCubic() -> Easing { Self::EaseOutCubic }
 #[getter]
 fn easeInOutCubic() -> Easing { Self::EaseInOutCubic }
 #[getter]
 fn easeInQuart() -> Easing { Self::EaseInQuart }
 #[getter]
 fn easeOutQuart() -> Easing { Self::EaseOutQuart }
 #[getter]
 fn easeInOutQuart() -> Easing { Self::EaseInOutQuart }
 #[getter]
 fn easeInQuint() -> Easing { Self::EaseInQuint }
 #[getter]
 fn easeOutQuint() -> Easing { Self::EaseOutQuint }
 #[getter]
 fn easeInOutQuint() -> Easing { Self::EaseInOutQuint }
 #[getter]
 fn easeInSine() -> Easing { Self::EaseInSine }
 #[getter]
 fn easeInExpo() -> Easing { Self::EaseInExpo }
 #[getter]
 fn easeOutExpo() -> Easing { Self::EaseOutExpo }
 #[getter]
 fn easeInOutExpo() -> Easing { Self::EaseInOutExpo }
 #[getter]
 fn easeInCirc() -> Easing { Self::EaseInCirc }
 #[getter]
 fn easeOutCirc() -> Easing { Self::EaseOutCirc }
 #[getter]
 fn easeInOutCirc() -> Easing { Self::EaseInOutCirc }
 #[getter]
 fn easeInBounce() -> Easing { Self::EaseInBounce }
 #[getter]
 fn easeOutBounce() -> Easing { Self::EaseOutBounce }
 #[getter]
 fn easeInOutBounce() -> Easing { Self::EaseInOutBounce }
 #[getter]
 fn easeInBack() -> Easing { Self::EaseInBack }
 #[getter]
 fn easeInOutBack() -> Easing { Self::EaseInOutBack }
 #[getter]
 fn easeInElastic() -> Easing { Self::EaseInElastic }
 #[getter]
 fn easeOutElastic() -> Easing { Self::EaseOutElastic }
 #[getter]
 fn easeInOutElastic() -> Easing { Self::EaseInOutElastic }
 fn cubicBezier(x1:f64,y1:f64,x2:f64,y2:f64) -> Result<Easing, NativeError> {
   let value=Self::CubicBezier(x1,y1,x2,y2);
   value.validate()?;
   Ok(value)
 }
}
}
impl Easing {
    pub fn validate(self) -> Result<(), NativeError> {
        if let Self::CubicBezier(x1, y1, x2, y2) = self {
            if ![x1, y1, x2, y2].iter().all(|v| v.is_finite())
                || !(0.0..=1.0).contains(&x1)
                || !(0.0..=1.0).contains(&x2)
                || y1.abs() > 100000.0
                || y2.abs() > 100000.0
            {
                return Err(NativeError::message(
                    "cubicBezier requires x controls in 0..=1 and finite y controls in -100000..=100000",
                ));
            }
        }
        Ok(())
    }
    pub fn sample(self, progress: f32) -> f32 {
        let t = progress.clamp(0.0, 1.0);
        // Exact endpoints also cover expo/elastic tails and normalized spring.
        if t == 0.0 || t == 1.0 {
            return t;
        }
        match self {
            Self::Spring => {
                // Unit step response: damping=6, damped angular frequency=12.
                let response =
                    |v: f32| 1.0 - (-6.0 * v).exp() * ((12.0 * v).cos() + 0.5 * (12.0 * v).sin());
                response(t) / response(1.0)
            }
            Self::Linear => t,
            Self::Ease | Self::SmoothStep => t * t * (3.0 - 2.0 * t),
            Self::EaseIn | Self::EaseInQuad => t * t,
            Self::EaseOut | Self::EaseOutQuad => 1.0 - (1.0 - t).powi(2),
            Self::EaseInOut | Self::EaseInOutQuad => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
            Self::EaseOutSine => (t * std::f32::consts::FRAC_PI_2).sin(),
            Self::EaseInOutSine => (1.0 - (t * std::f32::consts::PI).cos()) * 0.5,
            Self::EaseOutBack => 1.0 + 2.70158 * (t - 1.0).powi(3) + 1.70158 * (t - 1.0).powi(2),
            Self::Bounce | Self::EaseOutBounce => {
                let n = 7.5625;
                let d = 2.75;
                if t < 1.0 / d {
                    n * t * t
                } else if t < 2.0 / d {
                    n * (t - 1.5 / d).powi(2) + 0.75
                } else if t < 2.5 / d {
                    n * (t - 2.25 / d).powi(2) + 0.9375
                } else {
                    n * (t - 2.625 / d).powi(2) + 0.984375
                }
            }

            Self::EaseInCubic => t.powi(3),
            Self::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Self::EaseInOutCubic => {
                if t < 0.5 {
                    (2.0 * t).powi(3) * 0.5
                } else {
                    1.0 - (2.0 - 2.0 * t).powi(3) * 0.5
                }
            }

            Self::EaseInQuart => t.powi(4),
            Self::EaseOutQuart => 1.0 - (1.0 - t).powi(4),
            Self::EaseInOutQuart => {
                if t < 0.5 {
                    (2.0 * t).powi(4) * 0.5
                } else {
                    1.0 - (2.0 - 2.0 * t).powi(4) * 0.5
                }
            }

            Self::EaseInQuint => t.powi(5),
            Self::EaseOutQuint => 1.0 - (1.0 - t).powi(5),
            Self::EaseInOutQuint => {
                if t < 0.5 {
                    (2.0 * t).powi(5) * 0.5
                } else {
                    1.0 - (2.0 - 2.0 * t).powi(5) * 0.5
                }
            }

            Self::EaseInSine => 1.0 - (t * std::f32::consts::FRAC_PI_2).cos(),
            Self::EaseInExpo => 2.0_f32.powf(10.0 * t - 10.0),
            Self::EaseOutExpo => 1.0 - 2.0_f32.powf(-10.0 * t),
            Self::EaseInOutExpo => {
                if t < 0.5 {
                    2.0_f32.powf(20.0 * t - 10.0) * 0.5
                } else {
                    (2.0 - 2.0_f32.powf(-20.0 * t + 10.0)) * 0.5
                }
            }
            Self::EaseInCirc => 1.0 - (1.0 - t * t).max(0.0).sqrt(),
            Self::EaseOutCirc => (1.0 - (t - 1.0).powi(2)).max(0.0).sqrt(),
            Self::EaseInOutCirc => {
                if t < 0.5 {
                    (1.0 - (1.0 - (2.0 * t).powi(2)).max(0.0).sqrt()) * 0.5
                } else {
                    ((1.0 - (-2.0 * t + 2.0).powi(2)).max(0.0).sqrt() + 1.0) * 0.5
                }
            }
            Self::EaseInBounce => 1.0 - Self::EaseOutBounce.sample(1.0 - t),
            Self::EaseInOutBounce => {
                if t < 0.5 {
                    (1.0 - Self::EaseOutBounce.sample(1.0 - 2.0 * t)) * 0.5
                } else {
                    (1.0 + Self::EaseOutBounce.sample(2.0 * t - 1.0)) * 0.5
                }
            }
            Self::EaseInBack => 2.70158 * t.powi(3) - 1.70158 * t * t,
            Self::EaseInOutBack => {
                let c = 1.70158 * 1.525;
                if t < 0.5 {
                    (2.0 * t).powi(2) * ((c + 1.0) * 2.0 * t - c) * 0.5
                } else {
                    ((2.0 * t - 2.0).powi(2) * ((c + 1.0) * (2.0 * t - 2.0) + c) + 2.0) * 0.5
                }
            }
            Self::EaseInElastic => {
                -2.0_f32.powf(10.0 * t - 10.0)
                    * ((10.0 * t - 10.75) * std::f32::consts::TAU / 3.0).sin()
            }
            Self::EaseOutElastic => {
                2.0_f32.powf(-10.0 * t) * ((10.0 * t - 0.75) * std::f32::consts::TAU / 3.0).sin()
                    + 1.0
            }
            Self::EaseInOutElastic => {
                let phase = (20.0 * t - 11.125) * std::f32::consts::TAU / 4.5;
                if t < 0.5 {
                    -2.0_f32.powf(20.0 * t - 10.0) * phase.sin() * 0.5
                } else {
                    2.0_f32.powf(-20.0 * t + 10.0) * phase.sin() * 0.5 + 1.0
                }
            }

            Self::CubicBezier(x1, y1, x2, y2) => {
                if t == 0.0 || t == 1.0 {
                    return t;
                }
                // Solve x(u)=progress, then sample y(u); progress is not the curve parameter.
                let curve = |u: f64, a: f64, b: f64| {
                    3.0 * (1.0 - u).powi(2) * u * a + 3.0 * (1.0 - u) * u * u * b + u * u * u
                };
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..32 {
                    let u = (lo + hi) * 0.5;
                    if curve(u, x1, x2) < t as f64 {
                        lo = u
                    } else {
                        hi = u
                    }
                }
                curve((lo + hi) * 0.5, y1, y2) as f32
            }
        }
    }
    pub fn named(name: &str) -> Result<Self, NativeError> {
        Ok(match name {
            "" | "linear" => Self::Linear,
            "ease" => Self::Ease,
            "smoothStep" => Self::SmoothStep,
            "easeIn" | "ease_in" => Self::EaseIn,
            "easeOut" | "ease_out" => Self::EaseOut,
            "easeInOut" | "ease_in_out" => Self::EaseInOut,
            "easeOutSine" => Self::EaseOutSine,
            "easeInOutSine" => Self::EaseInOutSine,
            "bounce" => Self::Bounce,
            "easeOutBack" => Self::EaseOutBack,
            "spring" => Self::Spring,
            "easeInQuad" => Self::EaseInQuad,
            "easeOutQuad" => Self::EaseOutQuad,
            "easeInOutQuad" => Self::EaseInOutQuad,
            "easeInCubic" => Self::EaseInCubic,
            "easeOutCubic" => Self::EaseOutCubic,
            "easeInOutCubic" => Self::EaseInOutCubic,
            "easeInQuart" => Self::EaseInQuart,
            "easeOutQuart" => Self::EaseOutQuart,
            "easeInOutQuart" => Self::EaseInOutQuart,
            "easeInQuint" => Self::EaseInQuint,
            "easeOutQuint" => Self::EaseOutQuint,
            "easeInOutQuint" => Self::EaseInOutQuint,
            "easeInSine" => Self::EaseInSine,
            "easeInExpo" => Self::EaseInExpo,
            "easeOutExpo" => Self::EaseOutExpo,
            "easeInOutExpo" => Self::EaseInOutExpo,
            "easeInCirc" => Self::EaseInCirc,
            "easeOutCirc" => Self::EaseOutCirc,
            "easeInOutCirc" => Self::EaseInOutCirc,
            "easeInBounce" => Self::EaseInBounce,
            "easeOutBounce" => Self::EaseOutBounce,
            "easeInOutBounce" => Self::EaseInOutBounce,
            "easeInBack" => Self::EaseInBack,
            "easeInOutBack" => Self::EaseInOutBack,
            "easeInElastic" => Self::EaseInElastic,
            "easeOutElastic" => Self::EaseOutElastic,
            "easeInOutElastic" => Self::EaseInOutElastic,
            _ => return Err(NativeError::message(format!("unknown easing `{name}`"))),
        })
    }
}
// Rust-authored named defaults; script input uses the checked native constructor.
impl From<&str> for Easing {
    fn from(name: &str) -> Self {
        Self::named(name).expect("engine easing default must be registered")
    }
}

/// Save-safe animation parameters. Duration and curve are independent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationSpec {
    pub seconds: f64,
    pub easing: Easing,
    pub repeat: bool,
}
impl AnimationSpec {
    pub fn new(seconds: f64, easing: Easing, repeat: bool) -> Self {
        Self {
            seconds,
            easing,
            repeat,
        }
    }
    #[allow(non_snake_case)]
    pub fn Linear(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::Linear, repeat)
    }
    #[allow(non_snake_case)]
    pub fn Ease(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::Ease, repeat)
    }
    #[allow(non_snake_case)]
    pub fn EaseIn(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::EaseIn, repeat)
    }
    #[allow(non_snake_case)]
    pub fn EaseOut(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::EaseOut, repeat)
    }
    #[allow(non_snake_case)]
    pub fn EaseInOut(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::EaseInOut, repeat)
    }
    #[allow(non_snake_case)]
    pub fn SmoothStep(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::SmoothStep, repeat)
    }
    #[allow(non_snake_case)]
    pub fn EaseOutSine(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::EaseOutSine, repeat)
    }
    #[allow(non_snake_case)]
    pub fn EaseInOutSine(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::EaseInOutSine, repeat)
    }
    #[allow(non_snake_case)]
    pub fn Bounce(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::Bounce, repeat)
    }
    #[allow(non_snake_case)]
    pub fn EaseOutBack(seconds: f64, repeat: bool) -> Self {
        Self::new(seconds, Easing::EaseOutBack, repeat)
    }
    pub fn seconds(self) -> f64 {
        self.seconds
    }
    pub fn easing(self) -> Easing {
        self.easing
    }
    pub fn duration(self) -> f32 {
        self.seconds.max(0.0) as f32
    }
    pub fn repeats(self) -> bool {
        self.repeat
    }
    pub fn sample(self, progress: f32) -> f32 {
        self.easing.sample(progress)
    }
    pub(crate) fn repeat_forever(self) -> Self {
        Self {
            repeat: true,
            ..self
        }
    }
    pub fn with_time(self, seconds: f64) -> Result<Self, NativeError> {
        if !seconds.is_finite() || !(0.0..=3600.0).contains(&seconds) {
            return Err(NativeError::message(
                "animation time must be finite and in 0..=3600 seconds",
            ));
        }
        Ok(Self { seconds, ..self })
    }
    pub fn with_easing(self, easing: Easing) -> Result<Self, NativeError> {
        easing.validate()?;
        Ok(Self { easing, ..self })
    }
}
hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AnimationPhase {
    Transform(f64, f64, f64, f64),
}

impl AnimationPhase {
    fn rotation(degrees: f64) -> AnimationPhase { Self::Transform(degrees, 1.0, 0.0, 0.0) }
    fn scale(value: f64) -> AnimationPhase { Self::Transform(0.0, value, 0.0, 0.0) }
    fn offset(x: f64, y: f64) -> AnimationPhase { Self::Transform(0.0, 1.0, x, y) }
    fn transform(rotation: f64, scale: f64, x: f64, y: f64) -> AnimationPhase {
        Self::Transform(rotation, scale, x, y)
    }
}
}

impl AnimationPhase {
    pub fn values(self) -> (f32, f32, f32, f32) {
        match self {
            Self::Transform(rotation, scale, x, y) => {
                (rotation as f32, scale as f32, x as f32, y as f32)
            }
        }
    }
}

pub fn register_animation_api<C: 'static>(
    registry: &mut NativeRegistry<C>,
) -> Result<(), RegistrationError> {
    Easing::register_hks(registry)?;
    AnimationPhase::register_hks(registry)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn millisecond_effects_share_seconds_validation_and_rounding() {
        for (seconds, expected) in [
            (0.0, 0),
            (0.0004, 0),
            (0.0006, 1),
            (0.4, 400),
            (3600.0, 3_600_000),
        ] {
            assert_eq!(
                super::duration_millis(seconds).expect("valid duration"),
                expected
            );
        }
        for seconds in [-0.1, 3600.001, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(super::duration_millis(seconds).is_err(), "{seconds}");
        }
    }
    use super::*;

    const PRESETS: &[(&str, Easing)] = &[
        ("linear", Easing::Linear),
        ("smoothStep", Easing::SmoothStep),
        ("spring", Easing::Spring),
        ("easeInQuad", Easing::EaseInQuad),
        ("easeOutQuad", Easing::EaseOutQuad),
        ("easeInOutQuad", Easing::EaseInOutQuad),
        ("easeInCubic", Easing::EaseInCubic),
        ("easeOutCubic", Easing::EaseOutCubic),
        ("easeInOutCubic", Easing::EaseInOutCubic),
        ("easeInQuart", Easing::EaseInQuart),
        ("easeOutQuart", Easing::EaseOutQuart),
        ("easeInOutQuart", Easing::EaseInOutQuart),
        ("easeInQuint", Easing::EaseInQuint),
        ("easeOutQuint", Easing::EaseOutQuint),
        ("easeInOutQuint", Easing::EaseInOutQuint),
        ("easeInSine", Easing::EaseInSine),
        ("easeOutSine", Easing::EaseOutSine),
        ("easeInOutSine", Easing::EaseInOutSine),
        ("easeInExpo", Easing::EaseInExpo),
        ("easeOutExpo", Easing::EaseOutExpo),
        ("easeInOutExpo", Easing::EaseInOutExpo),
        ("easeInCirc", Easing::EaseInCirc),
        ("easeOutCirc", Easing::EaseOutCirc),
        ("easeInOutCirc", Easing::EaseInOutCirc),
        ("easeInBounce", Easing::EaseInBounce),
        ("easeOutBounce", Easing::EaseOutBounce),
        ("easeInOutBounce", Easing::EaseInOutBounce),
        ("easeInBack", Easing::EaseInBack),
        ("easeOutBack", Easing::EaseOutBack),
        ("easeInOutBack", Easing::EaseInOutBack),
        ("easeInElastic", Easing::EaseInElastic),
        ("easeOutElastic", Easing::EaseOutElastic),
        ("easeInOutElastic", Easing::EaseInOutElastic),
    ];

    #[test]
    fn all_presets_register_serialize_and_have_exact_endpoints() {
        for &(name, curve) in PRESETS {
            assert_eq!(Easing::named(name).expect("named curve"), curve);
            curve.validate().expect("valid preset");
            assert_eq!(curve.sample(-1.0), 0.0, "{name}");
            assert_eq!(curve.sample(1.0), 1.0, "{name}");
            assert_eq!(curve.sample(2.0), 1.0, "{name}");
            for i in 0..=1000 {
                assert!(curve.sample(i as f32 / 1000.0).is_finite(), "{name}");
            }
            let data = hiraku_script::hson::to_string(&curve).expect("serialize");
            assert_eq!(
                hiraku_script::hson::from_str::<Easing>(&data).expect("restore"),
                curve
            );
            crate::script::compile_story_bytecode(
                "easing.hks",
                &format!("camera().zoom(1.2).time(0.8).easing(.{name})"),
            )
            .expect("registered typed selector");
        }
    }

    #[test]
    fn directional_curves_are_symmetric_and_polynomial_values_are_exact() {
        for (input, output, in_out) in [
            (
                Easing::EaseInQuad,
                Easing::EaseOutQuad,
                Easing::EaseInOutQuad,
            ),
            (
                Easing::EaseInCubic,
                Easing::EaseOutCubic,
                Easing::EaseInOutCubic,
            ),
            (
                Easing::EaseInQuart,
                Easing::EaseOutQuart,
                Easing::EaseInOutQuart,
            ),
            (
                Easing::EaseInQuint,
                Easing::EaseOutQuint,
                Easing::EaseInOutQuint,
            ),
            (
                Easing::EaseInSine,
                Easing::EaseOutSine,
                Easing::EaseInOutSine,
            ),
            (
                Easing::EaseInExpo,
                Easing::EaseOutExpo,
                Easing::EaseInOutExpo,
            ),
            (
                Easing::EaseInCirc,
                Easing::EaseOutCirc,
                Easing::EaseInOutCirc,
            ),
            (
                Easing::EaseInBounce,
                Easing::EaseOutBounce,
                Easing::EaseInOutBounce,
            ),
            (
                Easing::EaseInBack,
                Easing::EaseOutBack,
                Easing::EaseInOutBack,
            ),
            (
                Easing::EaseInElastic,
                Easing::EaseOutElastic,
                Easing::EaseInOutElastic,
            ),
        ] {
            for i in 0..=100 {
                let t = i as f32 / 100.0;
                assert!(
                    (input.sample(t) - (1.0 - output.sample(1.0 - t))).abs() < 0.00001,
                    "{input:?}"
                );
                assert!(
                    (in_out.sample(t) + in_out.sample(1.0 - t) - 1.0).abs() < 0.00001,
                    "{in_out:?}"
                );
            }
            assert!((in_out.sample(0.5) - 0.5).abs() < 0.00001);
        }
        assert_eq!(Easing::EaseInCubic.sample(0.5), 0.125);
        assert_eq!(Easing::EaseOutQuart.sample(0.5), 0.9375);
        assert_eq!(Easing::EaseInQuint.sample(0.5), 0.03125);
        assert!((Easing::EaseInExpo.sample(0.5) - 0.03125).abs() < 0.000001);
        assert!((Easing::EaseOutCirc.sample(0.5) - 0.8660254).abs() < 0.000001);
        assert!(Easing::EaseInBack.sample(0.25) < 0.0);
        assert!(Easing::EaseOutBack.sample(0.75) > 1.0);
        assert!((1..100).any(|i| Easing::EaseOutElastic.sample(i as f32 / 100.0) > 1.0));
        assert!((1..100).any(|i| Easing::Spring.sample(i as f32 / 100.0) > 1.0));
    }

    #[test]
    fn cubic_bezier_solves_time_and_preserves_overshoot() {
        let curve = Easing::CubicBezier(0.25, 0.1, 0.25, 1.0);
        assert!((curve.sample(0.5) - 0.8024034).abs() < 0.00001);
        assert_eq!(curve.sample(0.0), 0.0);
        assert_eq!(curve.sample(1.0), 1.0);
        for curve in [
            Easing::CubicBezier(0.0, 0.0, 0.0, 0.0),
            Easing::CubicBezier(1.0, 1.0, 1.0, 1.0),
        ] {
            assert!((curve.sample(0.25) - 0.25).abs() < 0.00001);
        }
        assert!(Easing::CubicBezier(0.2, 1.5, 0.8, 1.5).sample(0.5) > 1.0);
        for invalid in [
            Easing::CubicBezier(-0.1, 0.0, 1.0, 1.0),
            Easing::CubicBezier(0.0, 0.0, 1.1, 1.0),
            Easing::CubicBezier(0.0, f64::NAN, 1.0, 1.0),
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn independent_parameters_commute_and_serialize() {
        let curve = Easing::CubicBezier(0.25, 0.1, 0.25, 1.0);
        let base = AnimationSpec::Linear(0.3, false);
        let first = base
            .with_time(2.0)
            .expect("time")
            .with_easing(curve)
            .expect("curve");
        let second = base
            .with_easing(curve)
            .expect("curve")
            .with_time(2.0)
            .expect("time");
        assert_eq!(first, second);
        let data = hiraku_script::hson::to_string(&first).expect("serialize");
        let restored: AnimationSpec = hiraku_script::hson::from_str(&data).expect("restore");
        assert_eq!(restored, first);
        assert_eq!(restored.sample(0.5), first.sample(0.5));
        for seconds in [-1.0, f64::NAN, f64::INFINITY, 3601.0] {
            assert!(base.with_time(seconds).is_err());
        }
    }

    #[test]
    fn smooth_step_uses_cubic_progress_and_preserves_repeat_duration() {
        let animation = AnimationSpec::SmoothStep(0.4, false);
        assert_eq!(animation.sample(0.25), 0.15625);
        assert_eq!(animation.sample(-1.0), 0.0);
        assert_eq!(animation.sample(2.0), 1.0);
        assert_eq!(animation.repeat_forever().duration(), 0.4);
        assert!(animation.repeat_forever().repeats());
    }
}
