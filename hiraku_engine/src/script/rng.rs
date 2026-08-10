use rand::RngExt;

use super::*;

fn host(ctx: &NativeCallContext) -> Result<ScriptHost, Box<EvalAltResult>> {
    ctx.tag()
        .and_then(|tag| tag.clone().try_cast::<ScriptHost>())
        .ok_or_else(|| runtime_error("hiraku script host is not available"))
}

#[allow(non_snake_case)]
#[export_module]
pub mod RNG {
    use super::*;

    #[rhai_fn(return_raw)]
    pub fn chance(ctx: NativeCallContext, p: INT) -> Result<bool, Box<EvalAltResult>> {
        Ok(rand_min_max(ctx, 0, 100)? <= p.clamp(0, 100))
    }

    #[rhai_fn(name = "rand", return_raw)]
    pub fn rand_max(ctx: NativeCallContext, max: INT) -> Result<INT, Box<EvalAltResult>> {
        if max < 0 {
            return Err(runtime_error("rand(max) requires max >= 0"));
        }
        rand_min_max(ctx, 0, max)
    }

    #[rhai_fn(name = "rand", return_raw)]
    pub fn rand_min_max(
        ctx: NativeCallContext,
        min: INT,
        max: INT,
    ) -> Result<INT, Box<EvalAltResult>> {
        if min > max {
            return Err(runtime_error("rand(min, max) requires min <= max"));
        }
        let host = host(&ctx)?;
        let generated = host.rng.lock().unwrap().random_range(min..=max);
        if host.checkpoint("rand", None, ctx.call_position()) == CheckpointDecision::ReplaySkip {
            let replayed = host.replay_input("rand")?;
            let StoredValue::Int(value) = replayed else {
                return Err(runtime_error(
                    "save replay recorded a non-integer rand value",
                ));
            };
            if value != generated {
                return Err(runtime_error(format!(
                    "save replay rand mismatch: recorded {value}, generated {generated}"
                )));
            }
            return Ok(value);
        }

        host.record_input(StoredValue::Int(generated));
        Ok(generated)
    }
}
