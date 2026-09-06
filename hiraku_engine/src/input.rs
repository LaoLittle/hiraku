//! Platform-independent input boundary for an embedded Hiraku canvas.

use std::collections::HashMap;

use bevy::{
    camera::RenderTarget,
    picking::pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput},
    prelude::*,
};
use uuid::Uuid;

use crate::{HirakuCanvas, HirakuInputTarget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirakuPointerPhase {
    Move,
    Press,
    Release,
    Cancel,
}

/// A host-provided pointer sample. UV uses a top-left origin and is independent
/// of the engine canvas resolution.
#[derive(Clone, Copy, Debug, Message)]
pub struct HirakuPointerInput {
    pub pointer: u64,
    pub uv: Vec2,
    pub phase: HirakuPointerPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirakuScrollUnit {
    Line,
    Pixel,
}

/// Scroll at a canvas-relative position, using the same pointer identity as
/// move/press/release. Hosts decide how physical input maps to this message.
#[derive(Clone, Copy, Debug, Message)]
pub struct HirakuScrollInput {
    pub pointer: u64,
    pub uv: Vec2,
    pub delta: Vec2,
    pub unit: HirakuScrollUnit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirakuAction {
    NextDialogue,
    Choice(usize),
    Back,
}

#[derive(Clone, Copy, Debug, Message)]
pub struct HirakuActionInput(pub HirakuAction);

/// Host-independent text editing commands. Hosts forward committed text (including
/// IME commits) here; the engine never reads physical keyboard state.
#[derive(Clone, Debug, Message)]
pub enum HirakuTextInput {
    Insert(String),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    SelectAll,
    Submit,
    Cancel,
    /// Composition is presentation only; it does not emit onChange.
    Preedit(String),
}

#[derive(Default, Resource)]
pub struct HirakuTextFocus(pub Option<Entity>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_sets_virtual_pointer_location_and_preserves_delta_units() {
        let mut app = App::new();
        app.insert_resource(HirakuCanvas {
            image: Handle::default(),
            size: UVec2::new(800, 600),
        })
        .insert_resource(HirakuInputTarget(Handle::default()))
        .add_message::<HirakuPointerInput>()
        .add_message::<HirakuScrollInput>()
        .add_message::<PointerInput>()
        .add_systems(Update, bridge_virtual_pointers);
        for unit in [HirakuScrollUnit::Line, HirakuScrollUnit::Pixel] {
            app.world_mut().write_message(HirakuScrollInput {
                pointer: 7,
                uv: Vec2::new(0.25, 0.75),
                delta: Vec2::new(1.0, -2.0),
                unit,
            });
            app.update();
            let events = app
                .world_mut()
                .resource_mut::<Messages<PointerInput>>()
                .drain()
                .collect::<Vec<_>>();
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].pointer_id, PointerId::Custom(Uuid::from_u128(8)));
            assert_eq!(events[0].location.position, Vec2::new(200.0, 450.0));
            assert!(matches!(events[0].action, PointerAction::Move { .. }));
            assert_eq!(events[1].pointer_id, events[0].pointer_id);
            let PointerAction::Scroll {
                x, y, unit: actual, ..
            } = events[1].action
            else {
                panic!("expected scroll")
            };
            assert_eq!((x, y), (1.0, -2.0));
            assert_eq!(
                actual,
                match unit {
                    HirakuScrollUnit::Line => bevy::input::mouse::MouseScrollUnit::Line,
                    HirakuScrollUnit::Pixel => bevy::input::mouse::MouseScrollUnit::Pixel,
                }
            );
        }
    }
}

pub(crate) fn bridge_virtual_pointers(
    mut commands: Commands,
    canvas: Option<Res<HirakuCanvas>>,
    target: Option<Res<HirakuInputTarget>>,
    mut input: MessageReader<HirakuPointerInput>,
    mut scrolls: MessageReader<HirakuScrollInput>,
    mut output: MessageWriter<PointerInput>,
    mut pointers: Local<HashMap<u64, Vec2>>,
) {
    let (Some(canvas), Some(target)) = (canvas, target) else {
        return;
    };
    let Some(target) = RenderTarget::Image(target.0.clone().into()).normalize(None) else {
        return;
    };
    for sample in input.read() {
        let position = sample.uv.clamp(Vec2::ZERO, Vec2::ONE) * canvas.size.as_vec2();
        let location = Location {
            target: target.clone(),
            position,
        };
        let id = PointerId::Custom(Uuid::from_u128(sample.pointer as u128 + 1));
        let is_new = !pointers.contains_key(&sample.pointer);
        let last = *pointers.entry(sample.pointer).or_insert_with(|| {
            commands.spawn(id);
            position
        });
        if sample.phase != HirakuPointerPhase::Move && (is_new || last != position) {
            output.write(PointerInput::new(
                id,
                location.clone(),
                PointerAction::Move {
                    delta: position - last,
                },
            ));
        }
        *pointers
            .get_mut(&sample.pointer)
            .expect("virtual pointer was inserted") = position;
        let action = match sample.phase {
            HirakuPointerPhase::Move => PointerAction::Move {
                delta: position - last,
            },
            HirakuPointerPhase::Press => PointerAction::Press(PointerButton::Primary),
            HirakuPointerPhase::Release => PointerAction::Release(PointerButton::Primary),
            HirakuPointerPhase::Cancel => PointerAction::Cancel,
        };
        output.write(PointerInput::new(id, location, action));
    }
    for scroll in scrolls.read() {
        if !scroll.uv.is_finite() || !scroll.delta.is_finite() {
            continue;
        }
        let position = scroll.uv.clamp(Vec2::ZERO, Vec2::ONE) * canvas.size.as_vec2();
        let location = Location {
            target: target.clone(),
            position,
        };
        let id = PointerId::Custom(Uuid::from_u128(scroll.pointer as u128 + 1));
        let previous = *pointers.entry(scroll.pointer).or_insert_with(|| {
            commands.spawn(id);
            position
        });
        pointers.insert(scroll.pointer, position);
        // A scroll can be the first sample, or follow layout movement with no
        // physical pointer motion. Set its location before picking the target.
        output.write(PointerInput::new(
            id,
            location.clone(),
            PointerAction::Move {
                delta: position - previous,
            },
        ));
        output.write(PointerInput::new(
            id,
            location,
            PointerAction::Scroll {
                unit: match scroll.unit {
                    HirakuScrollUnit::Line => bevy::input::mouse::MouseScrollUnit::Line,
                    HirakuScrollUnit::Pixel => bevy::input::mouse::MouseScrollUnit::Pixel,
                },
                x: scroll.delta.x,
                y: scroll.delta.y,
                phase: bevy::input::touch::TouchPhase::Moved,
            },
        ));
    }
}
