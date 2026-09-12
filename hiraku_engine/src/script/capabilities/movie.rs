//! Statement-scoped movie configuration. Playback remains an ECS responsibility.
use super::*;

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "MoviePlayback", handle_type = 8)]
struct MovieHandle(u64);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct MovieState {
    next: u64,
    pending: BTreeMap<u64, (String, bool, u64)>,
}
impl MovieState {
    pub(super) fn commit(&mut self, commands: &mut Vec<StoryEffect>, wait: &mut Option<StoryWait>) {
        for (_, (path, blocking, fade_out_ms)) in std::mem::take(&mut self.pending) {
            if blocking {
                *wait = Some(StoryWait::Movie { path, fade_out_ms });
            } else {
                commands.push(StoryEffect::MovieBackground { path, fade_out_ms });
            }
        }
    }
}
pub(super) fn register(registry: &mut NativeRegistry<CharacterContext>) {
    api::register_hks(registry).expect("movie API must be consistent");
}
#[hiraku_script::hks_module]
mod api {
    use super::*;
    #[hks(name = "movie")]
    fn movie(context: &mut CharacterContext, path: String) -> Result<MovieHandle, NativeError> {
        if path.trim().is_empty() {
            return Err(NativeError::message("movie path must not be empty"));
        }
        if !context.movie.pending.is_empty() {
            return Err(NativeError::message(
                "only one movie may be started per statement",
            ));
        }
        context.movie.next = context
            .movie
            .next
            .checked_add(1)
            .ok_or_else(|| NativeError::message("movie handles exhausted"))?;
        let id = context.movie.next;
        context.movie.pending.insert(id, (path, true, 0));
        Ok(MovieHandle(id))
    }
    #[hks(name = "fadeOut", receiver)]
    fn fade_out(
        context: &mut CharacterContext,
        MovieHandle(id): MovieHandle,
        milliseconds: f64,
    ) -> Result<MovieHandle, NativeError> {
        let duration =
            std::time::Duration::try_from_secs_f64(milliseconds / 1000.0).map_err(|_| {
                NativeError::message("movie fade duration must be finite and non-negative")
            })?;
        let millis = u64::try_from(duration.as_millis())
            .map_err(|_| NativeError::message("movie fade duration is too large"))?;
        context
            .movie
            .pending
            .get_mut(&id)
            .ok_or_else(|| NativeError::message("movie builder already committed"))?
            .2 = millis;
        Ok(MovieHandle(id))
    }

    /// Nonblocking video behind script UI; a terminal event cannot advance dialogue.
    #[hks(name = "nonBlocking", receiver)]
    fn non_blocking(
        context: &mut CharacterContext,
        MovieHandle(id): MovieHandle,
    ) -> Result<MovieHandle, NativeError> {
        context
            .movie
            .pending
            .get_mut(&id)
            .ok_or_else(|| NativeError::message("movie builder already committed"))?
            .1 = false;
        Ok(MovieHandle(id))
    }
    #[hks(name = "await", selector = "MoviePlayback", receiver)]
    fn await_movie(
        context: &mut CharacterContext,
        MovieHandle(id): MovieHandle,
    ) -> Result<(), NativeError> {
        context
            .movie
            .pending
            .get_mut(&id)
            .ok_or_else(|| NativeError::message("movie builder already committed"))?
            .1 = true;
        Ok(())
    }
    #[hks(name = "stopMovie")]
    fn stop_movie(context: &mut CharacterContext) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::StopMovie);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{StoryRuntime, StoryRuntimeEvent};
    #[test]
    fn nonblocking_movie_reaches_dialogue_and_can_be_stopped() {
        let code = compile_story_bytecode(
            "movie.hks",
            r#"
            movie("movies/scene.webm").nonBlocking()
            stopMovie()
            "Alice is listening."
        "#,
        )
        .expect("movie builder compiles");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        assert!(matches!(
            runtime.step().expect("movie"),
            Some(StoryRuntimeEvent::Effect(
                StoryEffect::MovieBackground { .. }
            ))
        ));
        assert!(matches!(
            runtime.step().expect("stop"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::StopMovie))
        ));
        assert!(matches!(
            runtime.step().expect("dialogue"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { .. }))
        ));
    }
    #[test]
    fn explicit_await_restores_blocking_mode() {
        let code = compile_story_bytecode(
            "movie.hks",
            "movie(\"movies/scene.webm\").nonBlocking().await()",
        )
        .expect("movie builder");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        assert!(matches!(
            runtime.step().expect("wait"),
            Some(StoryRuntimeEvent::Wait(StoryWait::Movie { .. }))
        ));
    }

    #[test]
    fn movie_exit_duration_reaches_the_host_and_survives_restore() {
        let code = compile_story_bytecode("movie.hks", "movie(\"clip\").fadeOut(1000).await()")
            .expect("movie with exit fade compiles");
        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
        let expected = StoryRuntimeEvent::Wait(StoryWait::Movie {
            path: "clip".into(),
            fade_out_ms: 1000,
        });
        assert_eq!(runtime.step().expect("host wait"), Some(expected.clone()));
        let snapshot = runtime.snapshot().expect("movie wait snapshot");
        let restored = StoryRuntime::restore(code, snapshot).expect("restore");
        assert_eq!(restored.restored_boundary_event(), Some(expected));
    }
}
