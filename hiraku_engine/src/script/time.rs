use super::*;

const SECONDS_PER_DAY: i64 = 86_400;

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
        current_timestamp(&ctx).map(format_date)
    }

    #[rhai_fn(return_raw)]
    pub fn time(ctx: NativeCallContext) -> Result<String, Box<EvalAltResult>> {
        current_timestamp(&ctx).map(format_time)
    }

    #[rhai_fn(return_raw)]
    pub fn datetime(ctx: NativeCallContext) -> Result<String, Box<EvalAltResult>> {
        let timestamp = current_timestamp(&ctx)?;
        Ok(format!(
            "{} {}",
            format_date(timestamp),
            format_time(timestamp)
        ))
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

fn format_date(timestamp: i64) -> String {
    let (year, month, day) = civil_from_days(timestamp.div_euclid(SECONDS_PER_DAY));
    format!("{year:04}-{month:02}-{day:02}")
}

fn format_time(timestamp: i64) -> String {
    let seconds = timestamp.rem_euclid(SECONDS_PER_DAY);
    let hour = seconds / 3600;
    let minute = seconds % 3600 / 60;
    let second = seconds % 60;
    format!("{hour:02}:{minute:02}:{second:02}")
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}
