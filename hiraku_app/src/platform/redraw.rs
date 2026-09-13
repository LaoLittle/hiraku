//! Render work can be queued after the main world's last redraw request.
//! Wake the host loop from the render thread instead of waiting for user input.
use bevy::{
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems, render_resource::PipelineCache,
    },
    winit::{EventLoopProxyWrapper, WinitUserEvent},
};

#[derive(Resource)]
struct RenderWake(Box<dyn Fn() + Send + Sync>);

fn extract_wake(
    mut commands: Commands,
    proxy: Extract<Option<Res<EventLoopProxyWrapper>>>,
    existing: Option<Res<RenderWake>>,
) {
    if existing.is_some() {
        return;
    }
    let Some(proxy) = proxy.as_ref() else { return };
    let proxy = (***proxy).clone();
    commands.insert_resource(RenderWake(Box::new(move || {
        // Closing the application invalidates the proxy; it is not a render error.
        let _ = proxy.send_event(WinitUserEvent::WakeUp);
    })));
}

#[derive(Default)]
struct PipelineWakeState {
    was_pending: bool,
}

impl PipelineWakeState {
    fn update(&mut self, pending: bool) -> bool {
        // Also wake on completion: the presentation camera can sample the
        // offscreen target one frame later under pipelined rendering.
        let wake = pending || self.was_pending;
        self.was_pending = pending;
        wake
    }
}

fn wake_pipeline_work(
    cache: Res<PipelineCache>,
    wake: Option<Res<RenderWake>>,
    mut state: Local<PipelineWakeState>,
) {
    let pending = cache.waiting_pipelines().next().is_some();
    if state.update(pending)
        && let Some(wake) = wake
    {
        (wake.0)();
    }
}

pub(crate) fn register(app: &mut App) {
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .add_systems(ExtractSchedule, extract_wake)
            .add_systems(Render, wake_pipeline_work.in_set(RenderSystems::Cleanup));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_pipeline_work_wakes_idle_runner_and_presents_completion() {
        let mut state = PipelineWakeState::default();
        for (pending, wake) in [
            (false, false),
            (true, true),
            (true, true),
            (false, true),
            (false, false),
        ] {
            assert_eq!(state.update(pending), wake);
        }
    }
    #[test]
    fn headless_host_does_not_require_a_proxy_or_render_world() {
        let mut app = App::new();
        register(&mut app);
        app.update();
    }
}
