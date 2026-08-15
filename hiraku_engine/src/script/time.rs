use super::*;
use ::time::OffsetDateTime;

fn host(ctx: &NativeCallContext) -> Result<ScriptHost, Box<EvalAltResult>> {
    ctx.tag()
        .and_then(|tag| tag.clone().try_cast::<ScriptHost>())
        .ok_or_else(|| runtime_error("hiraku script host is not available"))
}

#[allow(non_snake_case)]
#[export_module]
pub mod Time {
    use super::*;

    #[rhai_fn(return_raw)]
    pub fn timestamp(ctx: NativeCallContext) -> Result<INT, Box<EvalAltResult>> {
        current_timestamp(&ctx).map(|timestamp| timestamp as INT)
    }

    #[rhai_fn(return_raw)]
    pub fn date(ctx: NativeCallContext) -> Result<String, Box<EvalAltResult>> {
        format_timestamp(current_timestamp(&ctx)?, "date")
    }

    #[rhai_fn(return_raw)]
    pub fn time(ctx: NativeCallContext) -> Result<String, Box<EvalAltResult>> {
        format_timestamp(current_timestamp(&ctx)?, "time")
    }

    #[rhai_fn(return_raw)]
    pub fn datetime(ctx: NativeCallContext) -> Result<String, Box<EvalAltResult>> {
        format_timestamp(current_timestamp(&ctx)?, "datetime")
    }
}

fn current_timestamp(ctx: &NativeCallContext) -> Result<i64, Box<EvalAltResult>> {
    let host = host(ctx)?;
    if host.checkpoint("time", None, ctx.call_position()) == CheckpointDecision::ReplaySkip {
        let replayed = host.replay_input("time")?;
        let StoredValue::Int(value) = replayed else {
            return Err(runtime_error(
                "save replay recorded a non-integer time value",
            ));
        };
        return Ok(value);
    }

    let timestamp = new_time_seed();
    *host.time_seed.lock().unwrap() = timestamp;
    host.record_input(StoredValue::Int(timestamp));
    Ok(timestamp)
}

fn format_timestamp(timestamp: i64, kind: &str) -> Result<String, Box<EvalAltResult>> {
    let datetime = OffsetDateTime::from_unix_timestamp(timestamp)
        .map_err(|_| runtime_error("timestamp is outside the supported date range"))?;
    match kind {
        "date" => Ok(datetime.date().to_string()),
        "time" => Ok(datetime.time().to_string()),
        "datetime" => Ok(format!("{} {}", datetime.date(), datetime.time())),
        _ => unreachable!("time formatting kind is internal"),
    }
}
