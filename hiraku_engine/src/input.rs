//! Platform-independent input boundary for an embedded Hiraku canvas.

use std::collections::HashMap;

use bevy::{
    camera::RenderTarget,
    picking::pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput},
    prelude::*,
};
use uuid::Uuid;

use crate::{HirakuCanvas, HirakuInputTarget};

/// Physical picking forwards canvas input after this frame's `First` bridge.
/// Wake the next update to consume it; reactive runners otherwise wait for
/// another OS event or their idle timeout. Independent readers never steal input.
pub(crate) fn request_input_redraw(
    mut redraw: crate::redraw::Redraw,
    mut pointers: MessageReader<HirakuPointerInput>,
    mut scrolls: MessageReader<HirakuScrollInput>,
    mut actions: MessageReader<HirakuActionInput>,
    mut text: MessageReader<HirakuTextInput>,
) {
    let pending = pointers.read().count() + scrolls.read().count()
        + actions.read().count() + text.read().count();
    if pending != 0 { redraw.request(); }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirakuPointerPhase {
    Move,
    Press,
    Release,
    Cancel,
}

/// Host identities are namespaced so a finger never aliases the mouse or a
/// virtual cursor. The engine only interprets touch's direct-manipulation mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HirakuPointerId {
    Pointer(u64),
    Touch(u64),
}

impl HirakuPointerId {
    fn picking_id(self) -> PointerId {
        let value = match self {
            Self::Pointer(id) => id as u128 + 1,
            Self::Touch(id) => (1u128 << 65) | id as u128,
        };
        PointerId::Custom(Uuid::from_u128(value))
    }
}

/// A host-provided pointer sample. UV uses a top-left origin and is independent
/// of the engine canvas resolution.
#[derive(Clone, Copy, Debug, Message)]
pub struct HirakuPointerInput {
    pub pointer: HirakuPointerId,
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
    pub pointer: HirakuPointerId,
    pub uv: Vec2,
    pub delta: Vec2,
    pub unit: HirakuScrollUnit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirakuAction {
    /// Hosts send both press and release, including release on focus loss.
    FastForwardHeld(bool),
    ToggleFastForward,
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

#[derive(Clone)]
pub(crate) struct TouchScrollGesture {
    origin: Vec2,
    last: Vec2,
    target: Option<(Entity, bevy::picking::backend::HitData)>,
    dragging: bool,
}

#[derive(Component)]
pub(crate) struct RetiredTouchPointer;

pub(crate) fn cleanup_touch_pointers(
    mut commands: Commands,
    retired: Query<Entity, With<RetiredTouchPointer>>,
) {
    for entity in &retired {
        commands.entity(entity).despawn();
    }
}

fn retire_touch_pointer(
    pointer: HirakuPointerId,
    pointers: &mut HashMap<HirakuPointerId, (Vec2, Entity)>,
    commands: &mut Commands,
) {
    if let Some((_, entity)) = pointers.remove(&pointer) {
        commands.entity(entity).insert(RetiredTouchPointer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_host_input_wakes_the_bridge_then_returns_to_idle() {
        use bevy::{ecs::message::MessageCursor, picking::events::Scroll, window::RequestRedraw};
        for pointer in [HirakuPointerId::Pointer(0), HirakuPointerId::Touch(7)] {
            let mut app = App::new();
            app.insert_resource(HirakuCanvas { image: Handle::default(), size: UVec2::new(800, 600) })
                .insert_resource(HirakuInputTarget(Handle::default()))
                .add_message::<HirakuPointerInput>()
                .add_message::<HirakuScrollInput>()
                .add_message::<HirakuActionInput>()
                .add_message::<HirakuTextInput>()
                .add_message::<PointerInput>()
                .add_message::<Pointer<Scroll>>()
                .add_message::<RequestRedraw>()
                .add_systems(First, bridge_virtual_pointers)
                // Host picking happens after First; no animation or subsequent
                // physical input is available to drive another frame here.
                .add_systems(Update, move |mut sent: Local<bool>, mut output: MessageWriter<HirakuPointerInput>| {
                    if !*sent {
                        *sent = true;
                        output.write(HirakuPointerInput { pointer, uv: Vec2::splat(0.5), phase: HirakuPointerPhase::Press });
                    }
                })
                .add_systems(Last, request_input_redraw);
            let mut redraws = MessageCursor::<RequestRedraw>::default();
            let mut inputs = MessageCursor::<PointerInput>::default();
            app.update();
            assert_eq!(inputs.read(app.world().resource::<Messages<PointerInput>>()).count(), 0);
            assert_eq!(redraws.read(app.world().resource::<Messages<RequestRedraw>>()).count(), 1);
            app.update();
            assert!(inputs.read(app.world().resource::<Messages<PointerInput>>()).any(|event| matches!(event.action, PointerAction::Press(PointerButton::Primary))));
            assert_eq!(redraws.read(app.world().resource::<Messages<RequestRedraw>>()).count(), 1);
            app.update();
            assert_eq!(redraws.read(app.world().resource::<Messages<RequestRedraw>>()).count(), 0);
        }
    }

    #[test]
    fn non_pointer_input_also_requests_a_follow_up_frame() {
        use bevy::{ecs::message::MessageCursor, window::RequestRedraw};
        let mut app = App::new();
        app.add_message::<HirakuPointerInput>()
            .add_message::<HirakuScrollInput>()
            .add_message::<HirakuActionInput>()
            .add_message::<HirakuTextInput>()
            .add_message::<RequestRedraw>()
            .add_systems(Last, request_input_redraw);
        let mut redraws = MessageCursor::<RequestRedraw>::default();
        app.world_mut().write_message(HirakuActionInput(HirakuAction::NextDialogue));
        app.world_mut().write_message(HirakuTextInput::Insert("Alice".into()));
        app.world_mut().write_message(HirakuScrollInput {
            pointer: HirakuPointerId::Pointer(0), uv: Vec2::splat(0.5), delta: Vec2::Y, unit: HirakuScrollUnit::Line,
        });
        app.update();
        assert_eq!(redraws.read(app.world().resource::<Messages<RequestRedraw>>()).count(), 1);
        app.update();
        assert_eq!(redraws.read(app.world().resource::<Messages<RequestRedraw>>()).count(), 0);
    }

    #[test]
    fn touch_scroll_cancels_click_and_keeps_fingers_independent() {
        use bevy::picking::{backend::HitData, events::Scroll, hover::HoverMap};
        let mut app = App::new();
        app.insert_resource(HirakuCanvas {
            image: Handle::default(),
            size: UVec2::new(800, 600),
        })
        .insert_resource(HirakuInputTarget(Handle::default()))
        .init_resource::<HoverMap>()
        .add_message::<HirakuPointerInput>()
        .add_message::<HirakuScrollInput>()
        .add_message::<PointerInput>()
        .add_message::<Pointer<Scroll>>()
        .add_systems(Update, bridge_virtual_pointers)
        .add_systems(Last, cleanup_touch_pointers);
        let scrollable = app
            .world_mut()
            .spawn((
                Node {
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ScrollPosition::default(),
            ))
            .id();
        let child = app.world_mut().spawn(ChildOf(scrollable)).id();
        let finger = HirakuPointerId::Touch(0);
        assert_ne!(
            finger.picking_id(),
            HirakuPointerId::Pointer(0).picking_id()
        );
        assert_ne!(
            finger.picking_id(),
            HirakuPointerId::Pointer(u64::MAX).picking_id()
        );
        assert_ne!(
            finger.picking_id(),
            HirakuPointerId::Touch(u64::MAX).picking_id()
        );
        app.world_mut()
            .resource_mut::<HoverMap>()
            .0
            .entry(finger.picking_id())
            .or_default()
            .insert(
                child,
                HitData {
                    camera: scrollable,
                    depth: 0.0,
                    position: None,
                    normal: None,
                    extra: None,
                },
            );
        for (pointer, phase, uv) in [
            (finger, HirakuPointerPhase::Press, Vec2::splat(0.5)),
            (
                HirakuPointerId::Touch(1),
                HirakuPointerPhase::Press,
                Vec2::splat(0.5),
            ),
            (finger, HirakuPointerPhase::Move, Vec2::new(0.5, 0.4)),
            (finger, HirakuPointerPhase::Move, Vec2::new(0.5, 0.3)),
            (finger, HirakuPointerPhase::Release, Vec2::new(0.5, 0.3)),
            (
                HirakuPointerId::Touch(1),
                HirakuPointerPhase::Release,
                Vec2::splat(0.5),
            ),
        ] {
            app.world_mut()
                .write_message(HirakuPointerInput { pointer, phase, uv });
        }
        app.update();
        let scrolls = app
            .world_mut()
            .resource_mut::<Messages<Pointer<Scroll>>>()
            .drain()
            .collect::<Vec<_>>();
        assert_eq!(scrolls.len(), 2);
        for event in scrolls {
            assert_eq!(event.entity, scrollable);
            assert_eq!(event.pointer_id, finger.picking_id());
            assert!((event.y + 60.0).abs() < 0.001);
            assert_eq!(event.unit, bevy::input::mouse::MouseScrollUnit::Pixel);
        }
        let events = app
            .world_mut()
            .resource_mut::<Messages<PointerInput>>()
            .drain()
            .collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.pointer_id == finger.picking_id()
                    && matches!(event.action, PointerAction::Cancel))
                .count(),
            1
        );
        assert!(
            !events
                .iter()
                .any(|event| event.pointer_id == finger.picking_id()
                    && matches!(event.action, PointerAction::Release(_)))
        );
        assert!(events.iter().any(|event| event.pointer_id
            == HirakuPointerId::Touch(1).picking_id()
            && matches!(event.action, PointerAction::Release(_))));
        assert_eq!(
            app.world_mut()
                .query::<&PointerId>()
                .iter(app.world())
                .count(),
            0,
            "released virtual fingers are removed after event dispatch"
        );
    }

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
        .add_message::<Pointer<bevy::picking::events::Scroll>>()
        .add_systems(Update, bridge_virtual_pointers);
        for unit in [HirakuScrollUnit::Line, HirakuScrollUnit::Pixel] {
            app.world_mut().write_message(HirakuScrollInput {
                pointer: HirakuPointerId::Pointer(7),
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
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    canvas: Option<Res<HirakuCanvas>>,
    target: Option<Res<HirakuInputTarget>>,
    mut input: MessageReader<HirakuPointerInput>,
    mut scrolls: MessageReader<HirakuScrollInput>,
    mut output: MessageWriter<PointerInput>,
    mut pointers: Local<HashMap<HirakuPointerId, (Vec2, Entity)>>,
    hover: Option<Res<bevy::picking::hover::HoverMap>>,
    parents: Query<&ChildOf>,
    scroll_nodes: Query<&Node, With<ScrollPosition>>,
    mut gestures: Local<HashMap<HirakuPointerId, TouchScrollGesture>>,
    mut drag_scrolls: MessageWriter<Pointer<bevy::picking::events::Scroll>>,
) {
    let (Some(canvas), Some(target)) = (canvas, target) else {
        return;
    };
    let Some(target) = RenderTarget::Image(target.0.clone().into()).normalize(None) else {
        return;
    };
    for sample in input.read() {
        if !sample.uv.is_finite() {
            continue;
        }
        redraw.request();
        if matches!(sample.pointer, HirakuPointerId::Touch(_))
            && matches!(
                sample.phase,
                HirakuPointerPhase::Release | HirakuPointerPhase::Cancel
            )
            && !pointers.contains_key(&sample.pointer)
        {
            gestures.remove(&sample.pointer);
            continue;
        }
        let position = sample.uv.clamp(Vec2::ZERO, Vec2::ONE) * canvas.size.as_vec2();
        let location = Location {
            target: target.clone(),
            position,
        };
        let id = sample.pointer.picking_id();
        if matches!(sample.pointer, HirakuPointerId::Touch(_)) {
            if sample.phase == HirakuPointerPhase::Press {
                gestures.insert(
                    sample.pointer,
                    TouchScrollGesture {
                        origin: position,
                        last: position,
                        target: None,
                        dragging: false,
                    },
                );
            } else if let Some(gesture) = gestures.get_mut(&sample.pointer) {
                if gesture.target.is_none() && !gesture.dragging {
                    if let Some(hits) = hover.as_ref().and_then(|hover| hover.0.get(&id)) {
                        let mut hits = hits.iter().collect::<Vec<_>>();
                        hits.sort_by(|a, b| a.1.depth.total_cmp(&b.1.depth));
                        for (entity, hit) in hits {
                            let mut entity = *entity;
                            loop {
                                if scroll_nodes.get(entity).is_ok_and(|node| {
                                    node.overflow.x == OverflowAxis::Scroll
                                        || node.overflow.y == OverflowAxis::Scroll
                                }) {
                                    gesture.target = Some((entity, hit.clone()));
                                    break;
                                }
                                let Ok(parent) = parents.get(entity) else {
                                    break;
                                };
                                entity = parent.parent();
                            }
                            if gesture.target.is_some() {
                                break;
                            }
                        }
                    }
                }
                if sample.phase == HirakuPointerPhase::Move {
                    if let Some((entity, hit)) = &gesture.target {
                        let offset = position - gesture.origin;
                        let distance = scroll_nodes.get(*entity).map_or(0.0, |node| {
                            Vec2::new(
                                if node.overflow.x == OverflowAxis::Scroll {
                                    offset.x
                                } else {
                                    0.0
                                },
                                if node.overflow.y == OverflowAxis::Scroll {
                                    offset.y
                                } else {
                                    0.0
                                },
                            )
                            .length()
                        });
                        if !gesture.dragging && distance >= 8.0 {
                            gesture.dragging = true;
                            // Cancel the pending press before it can become a click.
                            output.write(PointerInput::new(
                                id,
                                location.clone(),
                                PointerAction::Cancel,
                            ));
                        }
                        if gesture.dragging {
                            let delta = position - gesture.last;
                            drag_scrolls.write(Pointer::new(
                                id,
                                location.clone(),
                                bevy::picking::events::Scroll {
                                    unit: bevy::input::mouse::MouseScrollUnit::Pixel,
                                    x: delta.x,
                                    y: delta.y,
                                    hit: hit.clone(),
                                    phase: bevy::input::touch::TouchPhase::Moved,
                                },
                                *entity,
                            ));
                            gesture.last = position;
                            continue;
                        }
                    }
                    gesture.last = position;
                }
                if matches!(
                    sample.phase,
                    HirakuPointerPhase::Release | HirakuPointerPhase::Cancel
                ) {
                    let dragging = gesture.dragging;
                    gestures.remove(&sample.pointer);
                    if dragging {
                        retire_touch_pointer(sample.pointer, &mut pointers, &mut commands);
                        continue;
                    }
                }
            }
        }
        let is_new = !pointers.contains_key(&sample.pointer);
        let last = pointers
            .entry(sample.pointer)
            .or_insert_with(|| (position, commands.spawn(id).id()))
            .0;
        if sample.phase != HirakuPointerPhase::Move && (is_new || last != position) {
            output.write(PointerInput::new(
                id,
                location.clone(),
                PointerAction::Move {
                    delta: position - last,
                },
            ));
        }
        pointers
            .get_mut(&sample.pointer)
            .expect("virtual pointer was inserted")
            .0 = position;
        let action = match sample.phase {
            HirakuPointerPhase::Move => PointerAction::Move {
                delta: position - last,
            },
            HirakuPointerPhase::Press => PointerAction::Press(PointerButton::Primary),
            HirakuPointerPhase::Release => PointerAction::Release(PointerButton::Primary),
            HirakuPointerPhase::Cancel => PointerAction::Cancel,
        };
        output.write(PointerInput::new(id, location.clone(), action));
        if matches!(sample.pointer, HirakuPointerId::Touch(_))
            && matches!(
                sample.phase,
                HirakuPointerPhase::Release | HirakuPointerPhase::Cancel
            )
        {
            if sample.phase == HirakuPointerPhase::Release {
                output.write(PointerInput::new(id, location, PointerAction::Cancel));
            }
            retire_touch_pointer(sample.pointer, &mut pointers, &mut commands);
        }
    }
    for scroll in scrolls.read() {
        if !scroll.uv.is_finite() || !scroll.delta.is_finite() {
            continue;
        }
        redraw.request();
        let position = scroll.uv.clamp(Vec2::ZERO, Vec2::ONE) * canvas.size.as_vec2();
        let location = Location {
            target: target.clone(),
            position,
        };
        let id = scroll.pointer.picking_id();
        let state = pointers
            .entry(scroll.pointer)
            .or_insert_with(|| (position, commands.spawn(id).id()));
        let previous = state.0;
        state.0 = position;
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
