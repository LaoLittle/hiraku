//! Offline decoder diagnostic; does not open an audio device or window.
use bevy::audio::{Decodable, Source};
use hiraku_engine::EngineAudioSource;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args_os().skip(1) {
        let asset = EngineAudioSource::from_bytes(std::fs::read(&path)?)?;
        let decoder = asset.decoder();
        let channels = decoder.channels().get() as usize;
        let duration = decoder.total_duration().ok_or("unknown audio duration")?;
        let limit = (duration.as_secs_f64()
            * f64::from(decoder.sample_rate().get())
            * channels as f64) as usize
            * 3;
        // Exercise the same buffering, repetition and format conversion as Bevy.
        let (mixer, mut output) = rodio::mixer::mixer(decoder.channels(), decoder.sample_rate());
        mixer.add(decoder.repeat_infinite());
        let chunk = channels * 960;
        let start = Instant::now();
        let mut worst = std::time::Duration::ZERO;
        let mut samples = 0u64;
        let mut invalid = 0u64;
        let mut peak = 0.0f32;
        loop {
            let tick = Instant::now();
            let mut count = 0;
            for sample in output.by_ref().take(chunk) {
                count += 1;
                invalid += u64::from(!sample.is_finite());
                peak = peak.max(sample.abs());
            }
            worst = worst.max(tick.elapsed());
            samples += count;
            if count == 0 && samples < limit as u64 {
                return Err("looping audio ended prematurely".into());
            }
            if samples >= limit as u64 {
                break;
            }
        }
        println!(
            "{}: samples={samples}, nonfinite={invalid}, peak={peak}, elapsed={:?}, worst_20ms={worst:?}",
            std::path::Path::new(&path).display(),
            start.elapsed()
        );
        if invalid > 0 {
            return Err("mixer produced non-finite PCM".into());
        }
    }
    Ok(())
}
