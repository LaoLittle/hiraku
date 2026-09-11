//! Window-independent scheduling requests for reactive application runners.
use bevy::{ecs::system::SystemParam, prelude::*, window::RequestRedraw};

/// At most one request per system invocation. Windowless embeddings and unit
/// tests may omit the window message resource entirely.
#[derive(SystemParam)]
pub(crate) struct Redraw<'w, 's> {
    commands: Commands<'w, 's>,
    messages: Option<Res<'w, Messages<RequestRedraw>>>,
}

impl Redraw<'_, '_> {
    pub(crate) fn request(&mut self) {
        if self.messages.take().is_some() {
            // Defer the write so independent animation systems can still run
            // in parallel rather than all borrowing the message queue mutably.
            self.commands.write_message(RequestRedraw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Active(bool);

    fn tick(active: Res<Active>, mut redraw: Redraw) {
        if active.0 {
            redraw.request();
            redraw.request();
        }
    }

    #[test]
    fn only_active_work_requests_one_redraw_each_frame() {
        let mut app = App::new();
        app.add_message::<RequestRedraw>()
            .init_resource::<Active>()
            .add_systems(Update, tick);
        let mut cursor = bevy::ecs::message::MessageCursor::<RequestRedraw>::default();
        for (active, expected) in [(false, 0), (true, 1), (true, 1), (false, 0)] {
            app.world_mut().resource_mut::<Active>().0 = active;
            app.update();
            assert_eq!(
                cursor
                    .read(app.world().resource::<Messages<RequestRedraw>>())
                    .count(),
                expected
            );
        }
    }

    #[test]
    fn headless_updates_do_not_require_window_resources() {
        let mut app = App::new();
        app.insert_resource(Active(true)).add_systems(Update, tick);
        app.update();
    }
}
