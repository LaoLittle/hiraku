//! Typed authoring surfaces over the shared pending scene-command storage.
//!
//! Adapters have distinct runtime handle tags as well as compile-time types.
//! The modifier implementation stays in `api`, so validation and commit timing
//! do not diverge between show, transform, exit, and stage operations.
use super::*;
use hiraku_script::native::RegistrationError;

macro_rules! builder {
    (
        $module:ident, $handle:ident, $name:tt, $tag:tt;
        $( $method:ident [ $public:tt ] ( $( $arg:ident : $ty:ty ),* )
            -> $result:ty => $implementation:ident; )*
    ) => {
        #[derive(Clone, Copy, hiraku_script::HksHandle)]
        #[hks(name = $name, handle_type = $tag)]
        pub(crate) struct $handle(pub(crate) u64);

        impl From<SceneTransitionHandle> for $handle {
            fn from(handle: SceneTransitionHandle) -> Self { Self(handle.0) }
        }

        #[hiraku_script::hks_module]
        mod $module {
            use super::*;

            $(
                #[hks(name = $public, selector = $name, receiver)]
                fn $method(
                    context: &mut CharacterContext,
                    handle: $handle,
                    $( $arg: $ty ),*
                ) -> Result<$result, NativeError> {
                    api::$implementation(context, SceneTransitionHandle(handle.0), $( $arg ),*)
                        .map(Into::into)
                }
            )*
        }
    };
}

builder! {
    picture_show, PictureShowHandle, "PictureShow", 10;
    at["at"](position: Position) -> PictureShowHandle => picture_at;
    view["view"](view: CameraScope) -> PictureShowHandle => picture_view;
    screen_space["screenSpace"]() -> PictureShowHandle => screen_space;
    frame["frame"](x: f64, y: f64, scale: f64, rotation: f64, layer: f64)
        -> PictureShowHandle => frame;
    layer["layer"](value: f64) -> PictureShowHandle => picture_layer;
    replace["replace"]() -> PictureShowHandle => picture_replace;
    dissolve["dissolve"](texture: String, softness: Option<f64>)
        -> PictureShowHandle => picture_dissolve;
    blur["blur"](radius: f64) -> PictureShowHandle => picture_blur;
    grayscale_gamma["grayscaleGamma"](red: f64, green: f64, blue: f64)
        -> PictureShowHandle => picture_grayscale_gamma;
    size["size"](width: f64, height: f64) -> PictureShowHandle => picture_size;
    slice["slice"](left: f64, top: f64, right: f64, bottom: f64)
        -> PictureShowHandle => picture_slice;
    tint["tint"](red: i32, green: i32, blue: i32, alpha: i32)
        -> PictureShowHandle => picture_tint;
    x["x"](value: f64) -> PictureShowHandle => transform_x;
    y["y"](value: f64) -> PictureShowHandle => transform_y;
    scale["scale"](value: f64) -> PictureShowHandle => transform_scale;
    rotation["rotation"](value: f64) -> PictureShowHandle => transform_rotation;
    time["time"](seconds: f64) -> PictureShowHandle => transition_time;
    fade["fade"](milliseconds: f64) -> PictureShowHandle => fade_in;
    wait["await"]() -> () => await_transition;
}

builder! {
    picture_transform, PictureTransformHandle, "PictureTransform", 11;
    at["at"](position: Position) -> PictureTransformHandle => picture_at;
    x["x"](value: f64) -> PictureTransformHandle => transform_x;
    y["y"](value: f64) -> PictureTransformHandle => transform_y;
    scale["scale"](value: f64) -> PictureTransformHandle => transform_scale;
    rotation["rotation"](value: f64) -> PictureTransformHandle => transform_rotation;
    time["time"](seconds: f64) -> PictureTransformHandle => transition_time;
    easing["easing"](curve: crate::script::animation::Easing)
        -> PictureTransformHandle => transition_easing;
    wait["await"]() -> () => await_transition;
}

builder! {
    picture_hide, PictureHideHandle, "PictureHide", 12;
    to["to"](x: f64, y: f64) -> PictureExitHandle => exit_to;
    time["time"](seconds: f64) -> PictureHideHandle => transition_time;
    fade["fade"](milliseconds: f64) -> PictureHideHandle => fade_in;
    wait["await"]() -> () => await_transition;
}

builder! {
    picture_exit, PictureExitHandle, "PictureExit", 13;
    time["time"](seconds: f64) -> PictureExitHandle => transition_time;
    fade["fade"](milliseconds: f64) -> PictureExitHandle => fade_in;
    easing["easing"](curve: crate::script::animation::Easing)
        -> PictureExitHandle => transition_easing;
    wait["await"]() -> () => await_transition;
}

builder! {
    curtain, CurtainTransitionHandle, "CurtainTransition", 14;
    color["color"](red: i32, green: i32, blue: i32) -> CurtainTransitionHandle => color;
    dissolve["dissolve"](texture: String, softness: Option<f64>)
        -> CurtainTransitionHandle => dissolve;
    time["time"](seconds: f64) -> CurtainTransitionHandle => transition_time;
    fade["fade"](milliseconds: f64) -> CurtainTransitionHandle => fade_in;
    wait["await"]() -> () => await_transition;
}

builder! {
    fade, FadeTransitionHandle, "FadeTransition", 15;
    time["time"](seconds: f64) -> FadeTransitionHandle => transition_time;
    fade["fade"](milliseconds: f64) -> FadeTransitionHandle => fade_in;
    wait["await"]() -> () => await_transition;
}

builder! {
    timed, TimedSceneTransitionHandle, "TimedSceneTransition", 16;
    time["time"](seconds: f64) -> TimedSceneTransitionHandle => transition_time;
    wait["await"]() -> () => await_transition;
}

builder! {
    effect, SceneEffectHandle, "SceneEffect", 17;
    wait["await"]() -> () => await_transition;
}

builder! {
    stage_camera, StageCameraTransitionHandle, "StageCameraTransition", 18;
    time["time"](seconds: f64) -> StageCameraTransitionHandle => transition_time;
    easing["easing"](curve: crate::script::animation::Easing)
        -> StageCameraTransitionHandle => transition_easing;
    wait["await"]() -> () => await_transition;
}

builder! {
    stage_view, StageViewTransitionHandle, "StageViewTransition", 19;
    time["time"](seconds: f64) -> StageViewTransitionHandle => transition_time;
    easing["easing"](curve: crate::script::animation::Easing)
        -> StageViewTransitionHandle => transition_easing;
    wait["await"]() -> () => await_transition;
}

pub(super) fn register(
    registry: &mut NativeRegistry<CharacterContext>,
) -> Result<(), RegistrationError> {
    picture_show::register_hks(registry)?;
    picture_transform::register_hks(registry)?;
    picture_hide::register_hks(registry)?;
    picture_exit::register_hks(registry)?;
    curtain::register_hks(registry)?;
    fade::register_hks(registry)?;
    timed::register_hks(registry)?;
    effect::register_hks(registry)?;
    stage_camera::register_hks(registry)?;
    stage_view::register_hks(registry)?;
    Ok(())
}
