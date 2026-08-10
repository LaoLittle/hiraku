use super::*;

fn host(ctx: &NativeCallContext) -> Result<ScriptHost, Box<EvalAltResult>> {
    ctx.tag()
        .and_then(|tag| tag.clone().try_cast::<ScriptHost>())
        .ok_or_else(|| runtime_error("hiraku script host is not available"))
}

#[allow(non_snake_case)]
#[export_module]
pub mod HirakuEngine {
    use super::*;

    #[rhai_fn(return_raw)]
    pub fn log(ctx: NativeCallContext, message: ImmutableString) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::Log(message.to_string()))
    }

    #[rhai_fn(return_raw)]
    pub fn seq(ctx: NativeCallContext, callback: FnPtr) -> Result<String, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        host.begin_batch(BatchMode::Sequence)?;
        match callback.call_within_context::<Dynamic>(&ctx, ()) {
            Ok(_) => host.finish_batch(),
            Err(err) => {
                host.cancel_batch();
                Err(err)
            }
        }
    }

    #[rhai_fn(return_raw)]
    pub fn par(ctx: NativeCallContext, callback: FnPtr) -> Result<String, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        host.begin_batch(BatchMode::Parallel)?;
        match callback.call_within_context::<Dynamic>(&ctx, ()) {
            Ok(_) => host.finish_batch(),
            Err(err) => {
                host.cancel_batch();
                Err(err)
            }
        }
    }

    #[rhai_fn(name = "pause", return_raw)]
    pub fn pause_seconds(
        ctx: NativeCallContext,
        seconds: FLOAT,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_seconds(seconds)?;
        run_blocking_or_collected(&host, "pause", |animation_id, done| ScriptCommand::Wait {
            duration,
            animation_id,
            done: done.expect("blocking or collected waits always provide a completion sender"),
        })
    }

    #[rhai_fn(name = "pause", return_raw)]
    pub fn pause_int(ctx: NativeCallContext, seconds: INT) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_seconds(seconds as FLOAT)?;
        run_blocking_or_collected(&host, "pause", |animation_id, done| ScriptCommand::Wait {
            duration,
            animation_id,
            done: done.expect("blocking or collected waits always provide a completion sender"),
        })
    }

    #[rhai_fn(return_raw)]
    pub fn pause_ms(ctx: NativeCallContext, ms: i64) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "pause", |animation_id, done| ScriptCommand::Wait {
            duration,
            animation_id,
            done: done.expect("blocking or collected waits always provide a completion sender"),
        })
    }

    #[rhai_fn(name = "wait", return_raw)]
    pub fn wait_handle(
        ctx: NativeCallContext,
        handle: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        if host.is_batch_mode() {
            return Err(runtime_error(
                "wait cannot be used inside seq/par; wait after the block returns a handle",
            ));
        }
        host.wait_for_handle(handle.to_string())
    }

    #[rhai_fn(name = "wait", return_raw)]
    pub fn wait_handles(ctx: NativeCallContext, handles: Array) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        if host.is_batch_mode() {
            return Err(runtime_error(
                "wait cannot be used inside seq/par; wait after the block returns a handle",
            ));
        }
        let handles = parse_animation_ids(handles)?;
        host.wait_for_handles(handles)
    }

    #[rhai_fn(return_raw)]
    pub fn cancel(
        ctx: NativeCallContext,
        handle: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        if host.is_batch_mode() {
            return Err(runtime_error("cancel cannot be used inside seq/par"));
        }
        host.cancel_handle(handle.to_string())
    }

    pub fn save_point(ctx: NativeCallContext, label: ImmutableString) {
        if let Ok(host) = host(&ctx) {
            let _ = host.checkpoint("save_point", Some(label.to_string()), ctx.call_position());
        }
    }

    pub fn checkpoint(ctx: NativeCallContext, label: ImmutableString) {
        if let Ok(host) = host(&ctx) {
            let _ = host.checkpoint("save_point", Some(label.to_string()), ctx.call_position());
        }
    }

    #[rhai_fn(return_raw)]
    pub fn say(
        ctx: NativeCallContext,
        speaker: ImmutableString,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        say_with_inline_commands(&ctx, &host, speaker.to_string(), text.to_string())
    }

    #[rhai_fn(return_raw)]
    pub fn narrate(
        ctx: NativeCallContext,
        text: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        say_with_inline_commands(&ctx, &host, String::new(), text.to_string())
    }

    #[rhai_fn(return_raw)]
    pub fn clear_text(ctx: NativeCallContext) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::ClearDialogue)
    }

    #[rhai_fn(return_raw)]
    pub fn bg(ctx: NativeCallContext, path: ImmutableString) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let path = host
            .resolve_background_path(&path)
            .map_err(vfs_to_rhai_error)?;
        host.send(ScriptCommand::SetBackground {
            path,
            fade: None,
            animation_id: None,
            done: None,
        })
    }

    #[rhai_fn(return_raw)]
    pub fn bg_fade(
        ctx: NativeCallContext,
        path: ImmutableString,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let fade = duration_from_millis(ms)?;
        let path = host
            .resolve_background_path(&path)
            .map_err(vfs_to_rhai_error)?;
        run_blocking_or_collected(&host, "bg-fade", |animation_id, done| {
            ScriptCommand::SetBackground {
                path,
                fade: Some(fade),
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn bg_rule(
        ctx: NativeCallContext,
        path: ImmutableString,
        rule_path: ImmutableString,
        ms: i64,
        vague: FLOAT,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        let path = host
            .resolve_background_path(&path)
            .map_err(vfs_to_rhai_error)?;
        let rule_path = host.resolve_path(&rule_path);
        run_blocking_or_collected(&host, "bg-rule", |animation_id, done| {
            ScriptCommand::RuleTransitionBg {
                path,
                rule_path,
                duration,
                vague: normalize_vague(vague),
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn effect(ctx: NativeCallContext, options: Map) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let options = parse_custom_effect_options(&host, options)?;
        run_blocking_or_collected(&host, "effect", |animation_id, done| {
            ScriptCommand::PlayCustomEffect {
                options,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(name = "show", return_raw)]
    pub fn show_sprite(
        ctx: NativeCallContext,
        id: ImmutableString,
        path: ImmutableString,
        x: FLOAT,
        y: FLOAT,
    ) -> Result<(), Box<EvalAltResult>> {
        show_sprite_impl(ctx, id, path, x, y, 1.0, 0.0)
    }

    #[rhai_fn(name = "show", return_raw)]
    pub fn show_sprite_scaled(
        ctx: NativeCallContext,
        id: ImmutableString,
        path: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
    ) -> Result<(), Box<EvalAltResult>> {
        show_sprite_impl(ctx, id, path, x, y, scale, 0.0)
    }

    #[rhai_fn(name = "show", return_raw)]
    pub fn show_sprite_layered(
        ctx: NativeCallContext,
        id: ImmutableString,
        path: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        layer: FLOAT,
    ) -> Result<(), Box<EvalAltResult>> {
        show_sprite_impl(ctx, id, path, x, y, scale, layer)
    }

    #[rhai_fn(return_raw)]
    fn show_sprite_impl(
        ctx: NativeCallContext,
        id: ImmutableString,
        path: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        layer: FLOAT,
    ) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let path = host.resolve_path(&path);
        let scale = positive_scale(scale)?;
        host.send(ScriptCommand::ShowSprite {
            id: id.to_string(),
            path,
            position: Vec2::new(x as f32, y as f32),
            layer: layer as f32,
            scale,
            fade: None,
            animation_id: None,
            done: None,
        })
    }

    #[rhai_fn(name = "show_fade", return_raw)]
    pub fn show_fade(
        ctx: NativeCallContext,
        id: ImmutableString,
        path: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        show_fade_impl(ctx, id, path, x, y, 1.0, 0.0, ms)
    }

    #[rhai_fn(name = "show_fade", return_raw)]
    pub fn show_fade_scaled(
        ctx: NativeCallContext,
        id: ImmutableString,
        path: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        show_fade_impl(ctx, id, path, x, y, scale, 0.0, ms)
    }

    #[rhai_fn(name = "show_fade", return_raw)]
    pub fn show_fade_layered(
        ctx: NativeCallContext,
        id: ImmutableString,
        path: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        layer: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        show_fade_impl(ctx, id, path, x, y, scale, layer, ms)
    }

    #[rhai_fn(return_raw)]
    fn show_fade_impl(
        ctx: NativeCallContext,
        id: ImmutableString,
        path: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        layer: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let fade = duration_from_millis(ms)?;
        let path = host.resolve_path(&path);
        let scale = positive_scale(scale)?;
        run_blocking_or_collected(&host, "show", |animation_id, done| {
            ScriptCommand::ShowSprite {
                id: id.to_string(),
                path,
                position: Vec2::new(x as f32, y as f32),
                layer: layer as f32,
                scale,
                fade: Some(fade),
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn hide(ctx: NativeCallContext, id: ImmutableString) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::HideSprite {
            id: id.to_string(),
            fade: None,
            animation_id: None,
            done: None,
        })
    }

    #[rhai_fn(name = "show_character", return_raw)]
    pub fn show_character(
        ctx: NativeCallContext,
        name: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let actor_id = name.to_string();
        show_character_impl(
            ctx,
            actor_id.clone().into(),
            actor_id.into(),
            x,
            y,
            scale,
            None,
        )
    }

    #[rhai_fn(name = "show_character", return_raw)]
    pub fn show_character_fade(
        ctx: NativeCallContext,
        name: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let actor_id = name.to_string();
        show_character_impl(
            ctx,
            actor_id.clone().into(),
            actor_id.into(),
            x,
            y,
            scale,
            Some(ms),
        )
    }

    #[rhai_fn(name = "show_character", return_raw)]
    pub fn show_character_as(
        ctx: NativeCallContext,
        actor_id: ImmutableString,
        name: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        show_character_impl(ctx, actor_id, name, x, y, scale, None)
    }

    #[rhai_fn(name = "show_character", return_raw)]
    pub fn show_character_as_fade(
        ctx: NativeCallContext,
        actor_id: ImmutableString,
        name: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        show_character_impl(ctx, actor_id, name, x, y, scale, Some(ms))
    }

    fn show_character_impl(
        ctx: NativeCallContext,
        actor_id: ImmutableString,
        character_name: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        ms: Option<i64>,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let scale = positive_scale(scale)?;
        let fade = ms.map(duration_from_millis).transpose()?;
        run_blocking_or_collected(&host, "character-show", |animation_id, done| {
            ScriptCommand::ShowCharacter {
                actor_id: actor_id.to_string(),
                character_name: character_name.to_string(),
                position: Vec2::new(x as f32, y as f32),
                scale,
                fade,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn hide_character(
        ctx: NativeCallContext,
        actor_id: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::HideCharacter {
            actor_id: actor_id.to_string(),
        })
    }

    #[rhai_fn(return_raw)]
    pub fn animate(
        ctx: NativeCallContext,
        actor_id: ImmutableString,
        keyframes: Array,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let actor_id = actor_id.to_string();
        let current = host.character_position(&actor_id)?;
        let keyframes = parse_character_animation_keyframes(keyframes, current)?;
        run_blocking_or_collected(&host, "character-animate", |animation_id, done| {
            ScriptCommand::AnimateCharacter {
                actor_id,
                keyframes,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn jump_character(
        ctx: NativeCallContext,
        actor_id: ImmutableString,
        height: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "character-jump", |animation_id, done| {
            ScriptCommand::JumpCharacter {
                actor_id: actor_id.to_string(),
                height: non_negative_amplitude(height),
                duration,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn shake_character(
        ctx: NativeCallContext,
        actor_id: ImmutableString,
        amplitude: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "character-shake", |animation_id, done| {
            ScriptCommand::ShakeCharacter {
                actor_id: actor_id.to_string(),
                amplitude: non_negative_amplitude(amplitude),
                duration,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn hide_fade(
        ctx: NativeCallContext,
        id: ImmutableString,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let fade = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "hide", |animation_id, done| {
            ScriptCommand::HideSprite {
                id: id.to_string(),
                fade: Some(fade),
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn screen_fade(
        ctx: NativeCallContext,
        alpha: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let fade = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "screen-fade", |animation_id, done| {
            ScriptCommand::SetOverlay {
                alpha: alpha.clamp(0.0, 1.0) as f32,
                fade: Some(fade),
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn move_sprite(
        ctx: NativeCallContext,
        id: ImmutableString,
        x: FLOAT,
        y: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "move", |animation_id, done| {
            ScriptCommand::MoveSprite {
                id: id.to_string(),
                position: Vec2::new(x as f32, y as f32),
                duration,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn move_sprite_by(
        ctx: NativeCallContext,
        id: ImmutableString,
        dx: FLOAT,
        dy: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        let current = host
            .scene_state
            .lock()
            .unwrap()
            .sprites
            .iter()
            .find(|sprite| sprite.id == id.as_str())
            .map(|sprite| Vec2::new(sprite.x, sprite.y))
            .ok_or_else(|| runtime_error(format!("sprite `{}` not found", id)))?;
        run_blocking_or_collected(&host, "move", |animation_id, done| {
            ScriptCommand::MoveSprite {
                id: id.to_string(),
                position: current + Vec2::new(dx as f32, dy as f32),
                duration,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn scale_sprite(
        ctx: NativeCallContext,
        id: ImmutableString,
        scale: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        let scale = positive_scale(scale)?;
        run_blocking_or_collected(&host, "scale", |animation_id, done| {
            ScriptCommand::ScaleSprite {
                id: id.to_string(),
                scale,
                duration,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn fade_sprite(
        ctx: NativeCallContext,
        id: ImmutableString,
        alpha: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "fade", |animation_id, done| {
            ScriptCommand::FadeSprite {
                id: id.to_string(),
                alpha: alpha.clamp(0.0, 1.0) as f32,
                duration,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn shake(
        ctx: NativeCallContext,
        ms: i64,
        amplitude: FLOAT,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "shake", |animation_id, done| ScriptCommand::Shake {
            duration,
            amplitude: non_negative_amplitude(amplitude),
            animation_id,
            done,
        })
    }

    #[rhai_fn(name = "play_bgm", return_raw)]
    pub fn play_bgm(
        ctx: NativeCallContext,
        path: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        play_bgm_impl(ctx, path, 1.0, None).map(|_| ())
    }

    #[rhai_fn(name = "play_bgm", return_raw)]
    pub fn play_bgm_volume(
        ctx: NativeCallContext,
        path: ImmutableString,
        volume: FLOAT,
    ) -> Result<(), Box<EvalAltResult>> {
        play_bgm_impl(ctx, path, volume, None).map(|_| ())
    }

    #[rhai_fn(name = "play_bgm", return_raw)]
    pub fn play_bgm_fade(
        ctx: NativeCallContext,
        path: ImmutableString,
        volume: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        play_bgm_impl(ctx, path, volume, Some(ms))
    }

    fn play_bgm_impl(
        ctx: NativeCallContext,
        path: ImmutableString,
        volume: FLOAT,
        ms: Option<i64>,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        reject_known_unsupported_audio_path(&path)?;
        let path = host.resolve_bgm_path(&path).map_err(vfs_to_rhai_error)?;
        let fade_in = ms.map(duration_from_millis).transpose()?;
        match fade_in {
            Some(fade_in) => run_blocking_or_collected(&host, "bgm", |animation_id, done| {
                ScriptCommand::PlayBgm {
                    path,
                    volume: clamp_volume(volume),
                    fade_in: Some(fade_in),
                    animation_id,
                    done,
                }
            }),
            None => {
                host.send(ScriptCommand::PlayBgm {
                    path,
                    volume: clamp_volume(volume),
                    fade_in: None,
                    animation_id: None,
                    done: None,
                })?;
                Ok(Dynamic::UNIT)
            }
        }
    }

    #[rhai_fn(return_raw)]
    pub fn set_bgm_volume(ctx: NativeCallContext, volume: FLOAT) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::SetBgmVolume {
            volume: clamp_volume(volume),
        })
    }

    #[rhai_fn(return_raw)]
    pub fn fade_bgm(
        ctx: NativeCallContext,
        volume: FLOAT,
        ms: i64,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let duration = duration_from_millis(ms)?;
        run_blocking_or_collected(&host, "bgm-fade", |animation_id, done| {
            ScriptCommand::FadeBgm {
                volume: clamp_volume(volume),
                duration,
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn stop_bgm(ctx: NativeCallContext) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::StopBgm)
    }

    #[rhai_fn(name = "play_voice", return_raw)]
    pub fn play_voice(
        ctx: NativeCallContext,
        path: ImmutableString,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        play_voice_impl(ctx, path, 1.0)
    }

    #[rhai_fn(name = "play_voice", return_raw)]
    pub fn play_voice_volume(
        ctx: NativeCallContext,
        path: ImmutableString,
        volume: FLOAT,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        play_voice_impl(ctx, path, volume)
    }

    fn play_voice_impl(
        ctx: NativeCallContext,
        path: ImmutableString,
        volume: FLOAT,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        reject_known_unsupported_audio_path(&path)?;
        let path = host.resolve_voice_path(&path).map_err(vfs_to_rhai_error)?;
        run_blocking_or_collected(&host, "voice", |animation_id, done| {
            ScriptCommand::PlayVoice {
                path,
                volume: clamp_volume(volume),
                animation_id,
                done,
            }
        })
    }

    #[rhai_fn(return_raw)]
    pub fn stop_voice(ctx: NativeCallContext) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::StopVoice)
    }

    #[rhai_fn(name = "play_sfx", return_raw)]
    pub fn play_sfx(
        ctx: NativeCallContext,
        path: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        play_sfx_impl(ctx, path, 1.0)
    }

    #[rhai_fn(name = "play_sfx", return_raw)]
    pub fn play_sfx_volume(
        ctx: NativeCallContext,
        path: ImmutableString,
        volume: FLOAT,
    ) -> Result<(), Box<EvalAltResult>> {
        play_sfx_impl(ctx, path, volume)
    }

    fn play_sfx_impl(
        ctx: NativeCallContext,
        path: ImmutableString,
        volume: FLOAT,
    ) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        reject_known_unsupported_audio_path(&path)?;
        let path = host
            .resolve_soundeffect_path(&path)
            .map_err(vfs_to_rhai_error)?;
        host.send(ScriptCommand::PlaySfx {
            path,
            volume: clamp_volume(volume),
        })
    }

    #[rhai_fn(name = "choice", return_raw)]
    pub fn choice(ctx: NativeCallContext, options: Array) -> Result<Dynamic, Box<EvalAltResult>> {
        choice_impl(ctx, String::new(), options)
    }

    #[rhai_fn(name = "choice", return_raw)]
    pub fn choice_prompt(
        ctx: NativeCallContext,
        prompt: ImmutableString,
        options: Array,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        choice_impl(ctx, prompt.to_string(), options)
    }

    fn choice_impl(
        ctx: NativeCallContext,
        prompt: String,
        options: Array,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let options = parse_choice_options(options)?;
        let selection = host.request_choice(ctx.call_position(), prompt, options)?;
        Ok(stored_value_to_dynamic(selection))
    }

    #[rhai_fn(return_raw)]
    pub fn screen(ctx: NativeCallContext, screen: Map) -> Result<Dynamic, Box<EvalAltResult>> {
        let host = host(&ctx)?;
        if host.checkpoint("screen", None, ctx.call_position()) == CheckpointDecision::ReplaySkip {
            return host.replay_input("screen").map(stored_value_to_dynamic);
        }
        let screen = parse_screen_spec(&host, screen)?;
        match host.send_and_wait(|done| ScriptCommand::ShowScreen { screen, done })? {
            ScriptResponse::Choice(value) => {
                host.record_input(value.clone());
                Ok(stored_value_to_dynamic(value))
            }
            ScriptResponse::Continue => Err(runtime_error(
                "engine returned unexpected continue response",
            )),
        }
    }

    #[rhai_fn(name = "show_overlay", return_raw)]
    pub fn show_named_overlay(
        ctx: NativeCallContext,
        name: ImmutableString,
        screen: Map,
    ) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let screen = parse_screen_spec(&host, screen)?;
        host.send(ScriptCommand::ShowOverlay {
            name: name.to_string(),
            screen,
        })
    }

    #[rhai_fn(name = "show_overlay", return_raw)]
    pub fn show_overlay(ctx: NativeCallContext, screen: Map) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        let screen = parse_screen_spec(&host, screen)?;
        host.send(ScriptCommand::ShowOverlay {
            name: "default".to_string(),
            screen,
        })
    }

    #[rhai_fn(name = "hide_overlay", return_raw)]
    pub fn hide_named_overlay(
        ctx: NativeCallContext,
        name: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::HideOverlay {
            name: name.to_string(),
        })
    }

    #[rhai_fn(name = "hide_overlay", return_raw)]
    pub fn hide_overlay(ctx: NativeCallContext) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::HideOverlay {
            name: "default".to_string(),
        })
    }

    #[rhai_fn(return_raw)]
    pub fn set_global(
        ctx: NativeCallContext,
        name: ImmutableString,
        value: Dynamic,
    ) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.set_global(&name, value)
    }

    pub fn get_global(ctx: NativeCallContext, name: ImmutableString) -> Dynamic {
        host(&ctx)
            .map(|host| host.get_global(&name))
            .unwrap_or(Dynamic::UNIT)
    }

    pub fn get_global_or(
        ctx: NativeCallContext,
        name: ImmutableString,
        fallback: Dynamic,
    ) -> Dynamic {
        host(&ctx)
            .map(|host| host.get_global_or(&name, fallback.clone()))
            .unwrap_or(fallback)
    }

    pub fn has_global(ctx: NativeCallContext, name: ImmutableString) -> bool {
        host(&ctx)
            .map(|host| host.has_global(&name))
            .unwrap_or(false)
    }

    pub fn remove_global(ctx: NativeCallContext, name: ImmutableString) -> bool {
        host(&ctx)
            .map(|host| host.remove_global(&name))
            .unwrap_or(false)
    }

    pub fn clear_globals(ctx: NativeCallContext) {
        if let Ok(host) = host(&ctx) {
            host.clear_globals();
        }
    }

    #[rhai_fn(return_raw)]
    pub fn get_setting(
        ctx: NativeCallContext,
        name: ImmutableString,
    ) -> Result<FLOAT, Box<EvalAltResult>> {
        Ok(host(&ctx)?.user_setting(&name)? as FLOAT)
    }

    #[rhai_fn(return_raw)]
    pub fn set_setting(
        ctx: NativeCallContext,
        name: ImmutableString,
        value: FLOAT,
    ) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.set_user_setting(&name, value as f32)
    }

    #[rhai_fn(return_raw)]
    pub fn set_ui_style(ctx: NativeCallContext, options: Map) -> Result<(), Box<EvalAltResult>> {
        let style = parse_ui_style_patch(options)?;
        host(&ctx)?.send(ScriptCommand::ApplyUiStyle(style))
    }

    #[rhai_fn(return_raw)]
    pub fn reset_ui_style(ctx: NativeCallContext) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::ResetUiStyle)
    }

    #[rhai_fn(return_raw)]
    pub fn set_text_effect(ctx: NativeCallContext, options: Map) -> Result<(), Box<EvalAltResult>> {
        let effect = parse_text_effect_spec(options)?;
        host(&ctx)?.send(ScriptCommand::SetTextEffect(effect))
    }

    #[rhai_fn(return_raw)]
    pub fn reset_text_effect(ctx: NativeCallContext) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::ResetTextEffect)
    }

    pub fn character_names(ctx: NativeCallContext) -> Array {
        host(&ctx)
            .map(|host| {
                host.character_names()
                    .into_iter()
                    .map(Dynamic::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn character_exists(ctx: NativeCallContext, name: ImmutableString) -> bool {
        host(&ctx)
            .map(|host| host.character_exists(&name))
            .unwrap_or(false)
    }

    #[rhai_fn(return_raw)]
    pub fn load_text(
        ctx: NativeCallContext,
        path: ImmutableString,
    ) -> Result<String, Box<EvalAltResult>> {
        host(&ctx)?.read_text(&path)
    }

    #[rhai_fn(return_raw)]
    pub fn load_bytes(
        ctx: NativeCallContext,
        path: ImmutableString,
    ) -> Result<Blob, Box<EvalAltResult>> {
        host(&ctx)?.read_bytes(&path)
    }

    pub fn exists(ctx: NativeCallContext, path: ImmutableString) -> bool {
        host(&ctx)
            .map(|host| {
                let path = host.resolve_path(&path);
                host.vfs.exists(&path)
            })
            .unwrap_or(false)
    }

    pub fn save_exists(ctx: NativeCallContext, slot: ImmutableString) -> bool {
        host(&ctx)
            .map(|host| host.save_exists(&slot).unwrap_or(false))
            .unwrap_or(false)
    }

    #[rhai_fn(name = "save", return_raw)]
    pub fn save(ctx: NativeCallContext, slot: ImmutableString) -> Result<(), Box<EvalAltResult>> {
        if host(&ctx)?.is_replaying() {
            return Ok(());
        }
        Err(save_script_signal(slot.to_string(), None))
    }

    #[rhai_fn(name = "save", return_raw)]
    pub fn save_with_resume(
        ctx: NativeCallContext,
        slot: ImmutableString,
        resume_script: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let host = host(&ctx)?;
        if host.is_replaying() {
            return Ok(());
        }
        let resume_script = host.resolve_path(&resume_script);
        Err(save_script_signal(slot.to_string(), Some(resume_script)))
    }

    #[rhai_fn(return_raw)]
    pub fn load(ctx: NativeCallContext, slot: ImmutableString) -> Result<(), Box<EvalAltResult>> {
        let resume_script = host(&ctx)?.load_game(&slot)?;
        Err(jump_script_signal(resume_script))
    }

    #[rhai_fn(return_raw)]
    pub fn load_script(
        ctx: NativeCallContext,
        path: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let resolved = host(&ctx)?.resolve_path(&path);
        Err(jump_script_signal(resolved))
    }

    #[rhai_fn(return_raw)]
    pub fn jump_script(
        ctx: NativeCallContext,
        path: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let resolved = host(&ctx)?.resolve_path(&path);
        Err(jump_script_signal(resolved))
    }

    #[rhai_fn(return_raw)]
    pub fn call_script(
        ctx: NativeCallContext,
        path: ImmutableString,
    ) -> Result<(), Box<EvalAltResult>> {
        let resolved = host(&ctx)?.resolve_path(&path);
        Err(call_script_signal(resolved))
    }

    #[rhai_fn(return_raw)]
    pub fn return_script() -> Result<(), Box<EvalAltResult>> {
        Err(return_script_signal())
    }

    #[rhai_fn(return_raw)]
    pub fn return_to_title(ctx: NativeCallContext) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::ReturnToTitle)?;
        Err(return_to_title_signal())
    }

    #[rhai_fn(return_raw)]
    pub fn quit(ctx: NativeCallContext) -> Result<(), Box<EvalAltResult>> {
        host(&ctx)?.send(ScriptCommand::Exit)
    }

    pub fn current_script(ctx: NativeCallContext) -> String {
        host(&ctx)
            .map(|host| host.current_script_path())
            .unwrap_or_default()
    }
}
