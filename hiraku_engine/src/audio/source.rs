//! Container loading and Bevy playback integration. Opus keeps compressed
//! packets in the asset and only one 120 ms PCM buffer per playback instance.
use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    audio::{Decodable, Source},
    prelude::{Asset, TypePath},
};
use hiraku_opus::OpusDecoder;
use std::{
    io::{self, Cursor},
    sync::Arc,
    time::Duration,
};

type AudioDecoder = Box<dyn Source<Item = f32> + Send>;

#[derive(Asset, TypePath, Clone, Debug)]
pub struct EngineAudioSource(AudioData);

#[derive(Clone, Debug)]
enum AudioData {
    Standard(bevy::audio::AudioSource),
    Opus(Arc<OpusData>),
}

#[derive(Debug)]
struct OpusData {
    packets: Vec<Vec<u8>>,
    channels: u16,
    skip: usize,
    end: u64,
    gain: f32,
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

impl EngineAudioSource {
    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        if bytes.starts_with(b"OggS") {
            let mut reader = ogg::PacketReader::new(Cursor::new(&bytes));
            let head = reader
                .read_packet()
                .map_err(invalid)?
                .ok_or_else(|| invalid("empty Ogg stream"))?;
            if head.data.starts_with(b"OpusHead") {
                if head.data.len() != 19
                    || head.data[8] > 15
                    || !matches!(head.data[9], 1 | 2)
                    || head.data[18] != 0
                {
                    return Err(invalid(
                        "unsupported Opus header: only mapping family 0, mono/stereo are supported",
                    ));
                }
                let channels = u16::from(head.data[9]);
                let skip = u16::from_le_bytes([head.data[10], head.data[11]]) as usize;
                let gain = 10.0_f32.powf(
                    f32::from(i16::from_le_bytes([head.data[16], head.data[17]])) / (256.0 * 20.0),
                );
                let serial = head.stream_serial();
                let tags = reader
                    .read_packet()
                    .map_err(invalid)?
                    .ok_or_else(|| invalid("missing OpusTags"))?;
                if tags.stream_serial() != serial || !valid_tags(&tags.data) {
                    return Err(invalid("missing OpusTags or multiplexed stream"));
                }
                let mut decoder = OpusDecoder::new(48000, channels as usize).map_err(invalid)?;
                let mut scratch = vec![0.0; 5760 * channels as usize];
                let mut decoded = 0u64;
                let mut end = None;
                let mut packets = Vec::new();
                while let Some(packet) = reader.read_packet().map_err(invalid)? {
                    if end.is_some() || packet.stream_serial() != serial {
                        return Err(invalid(
                            "chained/multiplexed Opus streams are not supported",
                        ));
                    }
                    if packet.data.is_empty() {
                        return Err(invalid("empty Opus audio packet"));
                    }
                    let count = decoder
                        .decode(&packet.data, 5760, &mut scratch)
                        .map_err(invalid)?;
                    if scratch[..count * channels as usize]
                        .iter()
                        .any(|sample| !(*sample * gain).is_finite())
                    {
                        return Err(invalid("Opus decoder produced non-finite PCM"));
                    }
                    decoded += count as u64;
                    if packet.last_in_stream() {
                        let position = packet.absgp_page();
                        if position < skip as u64
                            || position > decoded
                            || position < decoded - count as u64
                        {
                            return Err(invalid("unsupported or invalid Opus end granule"));
                        }
                        end = Some(position);
                    }
                    packets.push(packet.data);
                }
                let end = end
                    .ok_or_else(|| invalid("truncated Opus stream: missing end-of-stream page"))?;
                return Ok(Self(AudioData::Opus(Arc::new(OpusData {
                    packets,
                    channels,
                    skip,
                    end,
                    gain,
                }))));
            }
        }
        let audio = bevy::audio::AudioSource {
            bytes: bytes.into(),
        };
        // Bevy's AudioSource::decoder unwraps; validate at the fallible loader boundary.
        standard_decoder(audio.clone()).map_err(invalid)?;
        Ok(Self(AudioData::Standard(audio)))
    }
}

fn valid_tags(data: &[u8]) -> bool {
    if !data.starts_with(b"OpusTags") {
        return false;
    }
    let mut rest = &data[8..];
    fn number(rest: &mut &[u8]) -> Option<usize> {
        let bytes: [u8; 4] = rest.get(..4)?.try_into().ok()?;
        *rest = &rest[4..];
        Some(u32::from_le_bytes(bytes) as usize)
    }
    let Some(vendor) = number(&mut rest) else {
        return false;
    };
    let Some(after_vendor) = rest.get(vendor..) else {
        return false;
    };
    rest = after_vendor;
    let Some(count) = number(&mut rest) else {
        return false;
    };
    if count > rest.len() / 4 {
        return false;
    }
    for _ in 0..count {
        let Some(length) = number(&mut rest) else {
            return false;
        };
        let Some(after_comment) = rest.get(length..) else {
            return false;
        };
        rest = after_comment;
    }
    true
}

fn standard_decoder(
    audio: bevy::audio::AudioSource,
) -> Result<rodio::Decoder<Cursor<bevy::audio::AudioSource>>, rodio::decoder::DecoderError> {
    rodio::Decoder::builder()
        .with_byte_len(audio.bytes.len() as u64)
        .with_data(Cursor::new(audio))
        .build()
}

impl Decodable for EngineAudioSource {
    type Decoder = AudioDecoder;
    fn decoder(&self) -> Self::Decoder {
        let result: Result<AudioDecoder, String> = match &self.0 {
            AudioData::Standard(audio) => standard_decoder(audio.clone())
                .map(|d| Box::new(d) as AudioDecoder)
                .map_err(|e| e.to_string()),
            AudioData::Opus(data) => {
                OpusPlayback::new(data.clone()).map(|d| Box::new(d) as AudioDecoder)
            }
        };
        match result {
            Ok(decoder) => decoder,
            Err(error) => {
                bevy::log::error!("failed to initialize validated audio: {error}");
                Box::new(rodio::source::Empty::new())
            }
        }
    }
}

struct OpusPlayback {
    data: Arc<OpusData>,
    decoder: OpusDecoder,
    buffer: Vec<f32>,
    packet: usize,
    cursor: usize,
    length: usize,
    decoded_frames: u64,
    emitted_samples: u64,
}

impl OpusPlayback {
    fn new(data: Arc<OpusData>) -> Result<Self, String> {
        let decoder = OpusDecoder::new(48000, data.channels as usize).map_err(|e| e.to_string())?;
        Ok(Self {
            buffer: vec![0.0; 5760 * data.channels as usize],
            data,
            decoder,
            packet: 0,
            cursor: 0,
            length: 0,
            decoded_frames: 0,
            emitted_samples: 0,
        })
    }
}

impl Iterator for OpusPlayback {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let total = (self.data.end - self.data.skip as u64) * u64::from(self.data.channels);
        if self.emitted_samples >= total {
            return None;
        }
        while self.cursor >= self.length {
            let packet = self.data.packets.get(self.packet)?;
            self.packet += 1;
            let count = match self.decoder.decode(packet, 5760, &mut self.buffer) {
                Ok(count) => count,
                Err(error) => {
                    bevy::log::error!("Opus playback decode failed: {error}");
                    self.packet = self.data.packets.len();
                    return None;
                }
            };
            let start = self.decoded_frames;
            self.decoded_frames += count as u64;
            self.cursor = (self.data.skip as u64)
                .saturating_sub(start)
                .min(count as u64) as usize
                * self.data.channels as usize;
            self.length = self.data.end.saturating_sub(start).min(count as u64) as usize
                * self.data.channels as usize;
        }
        let sample = self.buffer[self.cursor] * self.data.gain;
        self.cursor += 1;
        self.emitted_samples += 1;
        Some(sample)
    }
}

impl Source for OpusPlayback {
    fn current_span_len(&self) -> Option<usize> {
        let total = (self.data.end - self.data.skip as u64) * u64::from(self.data.channels);
        // Bevy's LOOP uses rodio::Repeat -> Buffered. Buffered eagerly consumes
        // a whole span on the mixer thread. Advertising the whole track makes
        // it decode up to 32768 samples in one callback (~341 ms stereo).
        // Limit read-ahead to 20 ms (decoding still operates on whole packets).
        Some(
            total
                .saturating_sub(self.emitted_samples)
                .min(960 * u64::from(self.data.channels)) as usize,
        )
    }
    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::new(self.data.channels).expect("validated mono/stereo channels")
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(48000).expect("nonzero Opus output rate")
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            (self.data.end - self.data.skip as u64) as f64 / 48000.0,
        ))
    }
}

#[derive(Default, TypePath)]
pub struct EngineAudioLoader;

impl AssetLoader for EngineAudioLoader {
    type Asset = EngineAudioSource;
    type Settings = ();
    type Error = io::Error;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> io::Result<Self::Asset> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        EngineAudioSource::from_bytes(bytes)
    }
    fn extensions(&self) -> &[&str] {
        &["opus", "ogg", "oga", "wav", "mp3", "flac", "spx"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogg::writing::{PacketWriteEndInfo, PacketWriter};

    fn fixture(channels: u8, skip: u16, end: u64, gain: i16, packet: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut writer = PacketWriter::new(&mut output);
        let mut head = b"OpusHead".to_vec();
        head.extend([1, channels]);
        head.extend(skip.to_le_bytes());
        head.extend(48000u32.to_le_bytes());
        head.extend(gain.to_le_bytes());
        head.push(0);
        writer
            .write_packet(head.into_boxed_slice(), 7, PacketWriteEndInfo::EndPage, 0)
            .expect("write header");
        let mut tags = b"OpusTags".to_vec();
        tags.extend([0; 8]);
        writer
            .write_packet(tags.into_boxed_slice(), 7, PacketWriteEndInfo::EndPage, 0)
            .expect("write tags");
        writer
            .write_packet(packet.into(), 7, PacketWriteEndInfo::EndStream, end)
            .expect("write audio");
        output
    }

    #[test]
    fn opus_preskip_end_trim_and_independent_playback() {
        for channels in [1, 2] {
            let toc = if channels == 1 { 0xf8 } else { 0xfc };
            let asset =
                EngineAudioSource::from_bytes(fixture(channels, 120, 900, 0, &[toc, 0xff, 0xfe]))
                    .expect("load synthetic Opus");
            let first = asset.decoder();
            assert_eq!(first.channels().get(), channels as u16);
            assert_eq!(first.sample_rate().get(), 48000);
            assert_eq!(
                first.total_duration(),
                Some(Duration::from_secs_f64(780.0 / 48000.0))
            );
            let samples: Vec<_> = first.collect();
            assert_eq!(samples.len(), 780 * channels as usize);
            assert_eq!(samples, asset.decoder().collect::<Vec<_>>());
            assert!(samples.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn opus_header_gain_is_applied() {
        let asset = EngineAudioSource::from_bytes(fixture(1, 0, 960, 1536, &[0xf8, 0xff, 0xfe]))
            .expect("load gain fixture");
        let AudioData::Opus(data) = asset.0 else {
            panic!("expected Opus");
        };
        assert!((data.gain - 10.0f32.powf(6.0 / 20.0)).abs() < 0.00001);
    }

    #[test]
    fn malformed_audio_returns_load_errors() {
        assert!(EngineAudioSource::from_bytes(b"not audio".to_vec()).is_err());
        assert!(EngineAudioSource::from_bytes(fixture(3, 0, 960, 0, &[0xf8, 0xff, 0xfe])).is_err());
        assert!(
            EngineAudioSource::from_bytes(fixture(1, 0, 2000, 0, &[0xf8, 0xff, 0xfe])).is_err()
        );
        assert!(EngineAudioSource::from_bytes(fixture(1, 0, 960, 0, &[])).is_err());
        let mut truncated = fixture(1, 0, 960, 0, &[0xf8, 0xff, 0xfe]);
        truncated.pop();
        assert!(EngineAudioSource::from_bytes(truncated).is_err());
        assert!(!valid_tags(b"OpusTags\xff\xff\xff\xff"));
    }

    #[test]
    fn opus_prelude_and_loop_keep_one_source() {
        let asset = EngineAudioSource::from_bytes(fixture(1, 120, 900, 0, &[0xf8, 0xff, 0xfe]))
            .expect("load");
        let combined = crate::audio::PreludeLoopAudio::new(asset.clone(), asset);
        assert_eq!(combined.decoder().take(780 * 3).count(), 780 * 3);
    }

    #[test]
    fn opus_loop_keeps_mixer_and_other_sources_alive() {
        let asset = EngineAudioSource::from_bytes(fixture(2, 120, 900, 0, &[0xfc, 0xff, 0xfe]))
            .expect("load stereo fixture");
        let decoder = asset.decoder();
        let (mixer, mut output) = rodio::mixer::mixer(decoder.channels(), decoder.sample_rate());
        mixer.add(decoder.repeat_infinite());
        for _ in 0..20 {
            mixer.add(rodio::buffer::SamplesBuffer::new(
                rodio::ChannelCount::new(2).expect("stereo"),
                rodio::SampleRate::new(48000).expect("sample rate"),
                vec![0.25; 200],
            ));
            let samples: Vec<_> = output.by_ref().take(1560).collect();
            assert_eq!(samples.len(), 1560);
            assert!(samples.iter().all(|value| value.is_finite()));
            assert!(
                samples.iter().any(|value| *value > 0.2),
                "other sinks remain audible"
            );
        }
    }

    #[test]
    fn opus_spans_bound_buffered_decode_work() {
        let data = Arc::new(OpusData {
            packets: vec![vec![0xfc, 0xff, 0xfe]; 100],
            channels: 2,
            skip: 120,
            end: 96000,
            gain: 1.0,
        });
        let mut playback = OpusPlayback::new(data).expect("decoder");
        assert_eq!(playback.current_span_len(), Some(1920));
        for _ in 0..(96000 - 120) * 2 {
            assert!(playback.current_span_len().is_some_and(|n| n <= 1920));
            assert!(playback.next().is_some());
        }
        assert_eq!(playback.current_span_len(), Some(0));
        assert_eq!(playback.next(), None);
        assert_eq!(playback.next(), None);
    }

    #[test]
    fn opus_extension_loads_through_asset_server() {
        use bevy::{
            asset::{
                AssetApp, AssetPlugin, AssetServer, Assets,
                io::{
                    AssetSourceBuilder,
                    memory::{Dir, MemoryAssetReader},
                },
            },
            prelude::{App, MinimalPlugins},
        };
        let dir = Dir::default();
        dir.insert_asset(
            std::path::Path::new("alice.opus"),
            fixture(1, 120, 900, 0, &[0xf8, 0xff, 0xfe]),
        );
        let mut app = App::new();
        app.register_asset_source(
            "audio-test",
            AssetSourceBuilder::new(move || Box::new(MemoryAssetReader { root: dir.clone() })),
        );
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<EngineAudioSource>()
            .init_asset_loader::<EngineAudioLoader>();
        let handle = app
            .world()
            .resource::<AssetServer>()
            .load::<EngineAudioSource>("audio-test://alice.opus");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            if let Some(asset) = app
                .world()
                .resource::<Assets<EngineAudioSource>>()
                .get(&handle)
            {
                assert_eq!(asset.decoder().count(), 780);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Opus asset did not load"
            );
            std::thread::yield_now();
        }
    }
}
