use super::super::navigation::NavigationRequest;
use super::{CharacterContext, StoryNativeHost};
use hiraku_script::native::{NativeError, NativeRegistry};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "Countdown", handle_type = 22)]
struct Countdown(u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TimerState {
    remaining: Duration,
    running: bool,
    #[serde(default)]
    navigation: Option<NavigationRequest>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct ClockState {
    next: u64,
    timers: BTreeMap<u64, TimerState>,
}

impl ClockState {
    pub(super) fn advance(&mut self, delta: Duration) -> bool {
        let active = !delta.is_zero()
            && self
                .timers
                .values()
                .any(|timer| timer.running && !timer.remaining.is_zero());
        for timer in self.timers.values_mut().filter(|timer| timer.running) {
            timer.remaining = timer.remaining.saturating_sub(delta);
        }
        active
    }

    fn get_mut(&mut self, timer: Countdown) -> Result<&mut TimerState, NativeError> {
        self.timers.get_mut(&timer.0).ok_or_else(|| {
            NativeError::message(
                "countdown has been disposed or does not belong to this story session",
            )
        })
    }
}

fn duration(seconds: f64) -> Result<Duration, NativeError> {
    Duration::try_from_secs_f64(seconds).map_err(|_| {
        NativeError::message("countdown duration must be finite, nonnegative and representable")
    })
}

pub(super) fn register(registry: &mut NativeRegistry<CharacterContext>) {
    factory::register_hks(registry).expect("clock API registration must be consistent");
    methods::register_hks(registry).expect("countdown API registration must be consistent");
}

#[hiraku_script::hks_module("clock")]
mod factory {
    use super::*;

    #[hks]
    pub(super) fn countdown(
        context: &mut CharacterContext,
        seconds: f64,
    ) -> Result<Countdown, NativeError> {
        let remaining = duration(seconds)?;
        let next = context
            .clock
            .next
            .checked_add(1)
            .ok_or_else(|| NativeError::message("countdown handles exhausted"))?;
        context.clock.next = next;
        context.clock.timers.insert(
            next,
            TimerState {
                remaining,
                running: true,
                navigation: None,
                name: None,
            },
        );
        Ok(Countdown(next))
    }
}

#[hiraku_script::hks_module]
mod methods {
    use super::*;

    #[hks(selector = "Countdown", receiver)]
    pub(super) fn named(
        context: &mut CharacterContext,
        timer: Countdown,
        name: String,
    ) -> Result<Countdown, NativeError> {
        context.clock.get_mut(timer)?;
        if name.trim().is_empty() {
            return Err(NativeError::message("countdown name must not be empty"));
        }
        if context
            .clock
            .timers
            .iter()
            .any(|(&id, state)| id != timer.0 && state.name.as_deref() == Some(name.as_str()))
        {
            return Err(NativeError::message(format!(
                "countdown `{name}` already exists"
            )));
        }
        context.clock.get_mut(timer)?.name = Some(name);
        Ok(timer)
    }

    #[hks(name = "gotoOnExpire", selector = "Countdown", receiver)]
    pub(super) fn goto_on_expire(
        context: &mut CharacterContext,
        timer: Countdown,
        path: String,
    ) -> Result<Countdown, NativeError> {
        let mut request = NavigationRequest::goto(path)?.with_origin(context.origin.clone());
        request.preload = Some(false);
        context.clock.get_mut(timer)?.navigation = Some(request);
        Ok(timer)
    }

    #[hks(selector = "Countdown", receiver)]
    pub(super) fn remaining(
        context: &mut CharacterContext,
        timer: Countdown,
    ) -> Result<f64, NativeError> {
        Ok(context.clock.get_mut(timer)?.remaining.as_secs_f64())
    }

    #[hks(selector = "Countdown", receiver)]
    pub(super) fn pause(
        context: &mut CharacterContext,
        timer: Countdown,
    ) -> Result<Countdown, NativeError> {
        context.clock.get_mut(timer)?.running = false;
        Ok(timer)
    }

    #[hks(selector = "Countdown", receiver)]
    pub(super) fn resume(
        context: &mut CharacterContext,
        timer: Countdown,
    ) -> Result<Countdown, NativeError> {
        context.clock.get_mut(timer)?.running = true;
        Ok(timer)
    }

    #[hks(selector = "Countdown", receiver)]
    pub(super) fn reset(
        context: &mut CharacterContext,
        timer: Countdown,
        seconds: f64,
    ) -> Result<Countdown, NativeError> {
        let remaining = duration(seconds)?;
        context.clock.get_mut(timer)?.remaining = remaining;
        Ok(timer)
    }

    #[hks(selector = "Countdown", receiver)]
    pub(super) fn dispose(
        context: &mut CharacterContext,
        timer: Countdown,
    ) -> Result<(), NativeError> {
        context.clock.get_mut(timer)?;
        context.clock.timers.remove(&timer.0);
        Ok(())
    }
}

impl StoryNativeHost {
    pub(crate) fn named_countdowns(&self) -> BTreeMap<String, Duration> {
        self.context
            .clock
            .timers
            .values()
            .filter_map(|timer| {
                timer
                    .name
                    .as_ref()
                    .map(|name| (name.clone(), timer.remaining))
            })
            .collect()
    }
    pub(crate) fn due_navigation(&self) -> Option<u64> {
        self.context.clock.timers.iter().find_map(|(&id, timer)| {
            (timer.running && timer.remaining.is_zero() && timer.navigation.is_some()).then_some(id)
        })
    }

    pub(crate) fn deadline_navigation(&self, id: u64) -> Option<&NavigationRequest> {
        self.context
            .clock
            .timers
            .get(&id)
            .filter(|timer| timer.running)?
            .navigation
            .as_ref()
    }

    pub(crate) fn expire_navigation(&mut self, id: u64) -> Option<NavigationRequest> {
        let timer = self.context.clock.timers.get_mut(&id)?;
        if !timer.running {
            return None;
        }
        timer.remaining = Duration::ZERO;
        timer.running = false;
        timer.navigation.take()
    }

    pub(crate) fn advance_time(&mut self, delta: Duration) -> bool {
        self.context.clock.advance(delta)
    }

    pub(crate) fn restore_clock_observation(
        &mut self,
        call: &hiraku_script::BuiltinCall,
        value: &hiraku_script::Value,
    ) -> Result<(), NativeError> {
        let Some(receiver) = &call.receiver else {
            return Err(NativeError::message(
                "recorded countdown observation has no receiver",
            ));
        };
        let hiraku_script::Value::Handle { type_id: 22, id } = receiver else {
            return Err(NativeError::message(
                "recorded countdown observation has an invalid receiver",
            ));
        };
        let hiraku_script::Value::Number(seconds) = value else {
            return Err(NativeError::message(
                "recorded countdown observation must be Float",
            ));
        };
        let remaining = duration(*seconds)?;
        self.context.clock.get_mut(Countdown(*id))?.remaining = remaining;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_countdowns_reject_collisions_and_preserve_identity_on_restore() {
        let mut context = CharacterContext::default();
        let alice = factory::countdown(&mut context, 5.0).expect("alice timer");
        let bob = factory::countdown(&mut context, 2.0).expect("bob timer");
        methods::named(&mut context, alice, "alice".into()).expect("publish alice");
        methods::named(&mut context, alice, "alice".into()).expect("same identity");
        assert!(methods::named(&mut context, bob, "alice".into()).is_err());
        assert!(methods::named(&mut context, bob, "  ".into()).is_err());
        let bytes = hiraku_script::bhson::to_vec(&context.clock).expect("clock encoding");
        context.clock = hiraku_script::bhson::from_slice(&bytes).expect("clock decoding");
        context.clock.advance(Duration::from_secs(1));
        assert_eq!(
            methods::remaining(&mut context, alice).expect("remaining"),
            4.0
        );
        methods::dispose(&mut context, alice).expect("dispose alice");
        methods::named(&mut context, bob, "alice".into()).expect("name is reusable");
    }

    #[test]
    fn expiration_at_a_host_wait_boundary_survives_restore_and_replay() {
        use crate::script::{StoryRuntime, StoryRuntimeEvent};
        use hiraku_script::Value;

        fn waiting(runtime: &mut StoryRuntime) {
            loop {
                match runtime.step().expect("step to host wait") {
                    Some(StoryRuntimeEvent::Wait(_)) => break,
                    Some(_) => (),
                    None => panic!("expected host wait"),
                }
            }
        }
        fn expired(runtime: &mut StoryRuntime) {
            runtime.resume(Value::Unit).expect("finish host wait");
            loop {
                match runtime.step().expect("check deadline") {
                    Some(StoryRuntimeEvent::Effect(super::super::StoryEffect::Log(value))) => {
                        assert_eq!(value, "expired");
                        break;
                    }
                    Some(_) => (),
                    None => panic!("expected expiration branch"),
                }
            }
            assert!(!runtime.advance_time(Duration::from_secs(1)));
        }

        let code = super::super::compile_story_bytecode(
            "alice.hks",
            r#"
            let timer = clock.countdown(1)
            sleep(2)
            if timer.remaining() <= 0 {
                timer.dispose()
                log("expired")
            } else {
                log("active")
            }
            "#,
        )
        .expect("deadline story");
        let mut live = StoryRuntime::new(code.clone()).expect("live runtime");
        waiting(&mut live);
        live.advance_time(Duration::from_secs(2));
        let bytes = hiraku_script::bhson::to_vec(&live.snapshot().expect("snapshot"))
            .expect("encode suspended deadline");
        let mut restored = StoryRuntime::restore(
            code.clone(),
            hiraku_script::bhson::from_slice(&bytes).expect("decode suspended deadline"),
        )
        .expect("restore deadline");
        expired(&mut live);
        expired(&mut restored);
        let (trace, unrecordable) = live.take_native_trace();
        assert!(!unrecordable);
        assert_eq!(trace.len(), 1);
        let mut replay = StoryRuntime::new(code).expect("replay runtime");
        replay.replay_inputs(trace);
        waiting(&mut replay);
        // Replay has no real-time advancement; the recorded observation decides the branch.
        expired(&mut replay);
    }

    #[test]
    fn countdown_observations_replay_and_restore_the_live_remaining_budget() {
        use crate::script::{StoryRuntime, StoryRuntimeEvent};
        use hiraku_script::Value;

        fn wait(runtime: &mut StoryRuntime) {
            loop {
                match runtime.step().expect("step") {
                    Some(StoryRuntimeEvent::Wait(_)) => return,
                    Some(_) => (),
                    None => panic!("expected dialogue wait"),
                }
            }
        }
        fn log(runtime: &mut StoryRuntime) -> f64 {
            loop {
                match runtime.step().expect("step") {
                    Some(StoryRuntimeEvent::Effect(super::super::StoryEffect::Log(value))) => {
                        return value.parse().expect("numeric timer value");
                    }
                    Some(_) => (),
                    None => panic!("expected timer observation"),
                }
            }
        }
        let code = super::super::compile_story_bytecode(
            "alice.hks",
            r#"
            let timer = clock.countdown(10)
            narrate("Alice")
            log(timer.remaining().toString())
            narrate("Bob")
            log(timer.remaining().toString())
        "#,
        )
        .expect("timer story");
        let mut live = StoryRuntime::new(code.clone()).expect("live");
        wait(&mut live);
        assert!(live.advance_time(Duration::from_secs(2)));
        live.resume(Value::Unit).expect("advance Alice");
        assert_eq!(log(&mut live), 8.0);
        let (trace, unrecordable) = live.take_native_trace();
        assert!(!unrecordable);
        assert_eq!(trace.len(), 1);
        let snapshot = live.snapshot().expect("snapshot");
        let mut restored = StoryRuntime::restore(code.clone(), snapshot).expect("restore");
        let mut replay = StoryRuntime::new(code).expect("replay");
        replay.replay_inputs(trace);
        wait(&mut replay);
        replay.resume(Value::Unit).expect("replay advance");
        assert_eq!(log(&mut replay), 8.0);
        replay.finish_replay();
        for runtime in [&mut restored, &mut replay] {
            wait(runtime);
            assert!(runtime.advance_time(Duration::from_secs(1)));
            runtime.resume(Value::Unit).expect("advance Bob");
            assert_eq!(log(runtime), 7.0);
        }
    }

    #[test]
    fn pause_restore_resume_has_no_wall_time_catch_up() {
        let mut host = StoryNativeHost::new();
        let timer = factory::countdown(&mut host.context, 10.0).expect("countdown");
        host.advance_time(Duration::from_secs(2));
        methods::pause(&mut host.context, timer).expect("pause");
        host.advance_time(Duration::from_secs(60));
        let snapshot = host.snapshot();
        let encoded = hiraku_script::bhson::to_vec(&snapshot).expect("encode timer state");
        let decoded = hiraku_script::bhson::from_slice(&encoded).expect("decode timer state");
        assert_eq!(snapshot, decoded);
        let mut restored = StoryNativeHost::restore(decoded);
        assert_eq!(
            methods::remaining(&mut restored.context, timer).expect("remaining"),
            8.0
        );
        methods::resume(&mut restored.context, timer).expect("resume");
        restored.advance_time(Duration::from_secs(3));
        assert_eq!(
            methods::remaining(&mut restored.context, timer).expect("remaining"),
            5.0
        );
        restored.advance_time(Duration::from_secs(100));
        assert_eq!(
            methods::remaining(&mut restored.context, timer).expect("expired"),
            0.0
        );
    }

    #[test]
    fn reset_validation_is_atomic_and_disposal_invalidates_the_handle() {
        let mut host = StoryNativeHost::new();
        let timer = factory::countdown(&mut host.context, 5.0).expect("countdown");
        let snapshot = host.snapshot();
        for seconds in [-1.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(methods::reset(&mut host.context, timer, seconds).is_err());
            assert!(factory::countdown(&mut host.context, seconds).is_err());
            assert_eq!(host.snapshot(), snapshot);
        }
        methods::dispose(&mut host.context, timer).expect("dispose");
        assert!(methods::remaining(&mut host.context, timer).is_err());
        assert!(methods::resume(&mut host.context, timer).is_err());
    }

    #[test]
    fn countdown_api_is_statically_typed() {
        super::super::compile_story_bytecode("alice.hks",
            "let timer = clock.countdown(5).pause()\ntimer.resume()\nlet left: Float = timer.remaining()\ntimer.dispose()")
            .expect("typed timer API");
        assert!(
            super::super::compile_story_bytecode("bob.hks", "clock.countdown(\"five\")").is_err()
        );
    }
}
