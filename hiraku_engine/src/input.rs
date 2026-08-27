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
pub enum HirakuAction {
    NextDialogue,
    Choice(usize),
    Back,
}

#[derive(Clone, Copy, Debug, Message)]
pub struct HirakuActionInput(pub HirakuAction);

pub(crate) fn bridge_virtual_pointers(
    mut commands: Commands,
    canvas: Option<Res<HirakuCanvas>>,
    target: Option<Res<HirakuInputTarget>>,
    mut input: MessageReader<HirakuPointerInput>,
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
}
