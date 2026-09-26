# Hiraku Codec

A Rust codec API following Web Codecs standard [WebCodecs working draft (27 August 2026)](https://www.w3.org/TR/2026/WD-webcodecs-20260827/).

This crate accepts encoded chunks and produces decoded video frames or interleaved
PCM.

## Decoders

`VideoDecoder` and `AudioDecoder` expose:

- `is_config_supported(&config).await`: query backend support.
- `configure(config)`: enqueue configuration.
- `decode(chunk)`: enqueue a timestamped key/delta chunk.
- `decode_queue_size()` and `pending_output()`: host backpressure.
- `flush()`: enqueue a drain barrier and return its `FlushId`.
- `poll()`: receive `Output(frame)`, `Flushed(id)` or `Error(error)`.
- `reset()`: discard pending work, output and flush barriers; become unconfigured.
- `close()`: release resources and permanently close this decoder.

`flush` is nonblocking: the matching event follows all preceding output.
A key chunk is required after configure or flush. Reset preserves monotonically
increasing flush IDs, so an old barrier cannot be mistaken for a new one.
Fatal errors are delivered by polling and close the decoder. Dropping a decoder
cancels its worker without joining or blocking the calling thread.

Timestamps use signed microseconds, including negative preroll. Encoded bytes
and PCM use reference-counted storage. Callers own returned frames and release
them through ordinary Rust ownership.

```rust
use hiraku_codec::{AudioDecoder, AudioDecoderConfig, DecoderEvent};

let mut decoder = AudioDecoder::new()?;
decoder.configure(AudioDecoderConfig::new("opus", 48_000, 2)?)?;
// Feed EncodedAudioChunk values supplied by your demuxer or network transport.
let barrier = decoder.flush()?;

// Call again on later updates when poll returns None.
while let Some(event) = decoder.poll() {
    match event {
        DecoderEvent::Output(pcm) => consume(pcm),
        DecoderEvent::Flushed(id) if id == barrier => break,
        DecoderEvent::Flushed(_) => {}
        DecoderEvent::Error(error) => return Err(error),
    }
}
```

This is a Rust adaptation of the decoder processing model, not an implementation
of the entire W3C surface. Polling replaces JavaScript callbacks/promises, reset
discards outstanding flush tokens, and `Drop` handles resource release. Image
decoders and GPU-native frame handles are not implemented yet.

## Encoder contracts

`VideoEncoder` / `AudioEncoder` provide configuration, support queries, `encode`,
`encode_queue_size`, `poll`, `flush`, `reset` and `close`. Output events carry
encoded chunks and decoder-configuration metadata. Video options include key
frames, bitrate mode, latency mode and alpha preservation. No encoder adapter is
implemented yet: support queries return false and configure returns Unsupported.
This is an interface, not a promise that a platform's encoder is wired up.

## Backend support

`Codec` is a parsed enum with AV1/VP9 profile and color metadata. Constructors
accept `TryInto<Codec>`: either a registry string or a `Codec` value. Parsing uses
`FromStr`/`TryFrom`, not infallible `From`; malformed and unsupported identifiers
return errors. Parsing success does not imply adapter support.

| Backend | Current support |
| --- | --- |
| Software | AV1 profiles 0/1/2, 8/10/12-bit planar output via rav1d; mono/stereo Opus via hiraku-opus |
| macOS / iOS | AV1, VP9, AVC and HEVC VideoToolbox, subject to device capability |
| Windows | AV1, VP8, VP9, AVC and HEVC hardware Media Foundation transforms |
| Android | AV1, VP8, VP9, AVC and HEVC MediaCodec byte-buffer output |
| Linux | AV1 and VP9 VA-API through cros-codecs |
| Web | Configuration and chunks forwarded to browser WebCodecs; support depends on the browser |

Native work runs on dedicated workers with bounded output queues and cancellable
sends. Browser bindings are private to `platform/wasm`, including the audio types
that web-sys gates as unstable; no `web_sys_unstable_apis` rustc flag is required.
Frames support planar 8/10/12-bit, NV12, P010 and RGBA. VideoToolbox exposes P010;
WebCodecs exposes its planar high-depth formats. Windows/Android/VA-API bridges
currently reject high-depth output before input submission, allowing AV1 software
fallback. PQ/HLG and BT.2020 conversion are supported by the video renderer, which
tone maps to its SDR output; HDR display/swapchain output is not implemented.

The default `hardware` feature enables native platform backends.
Native default selection tries platform hardware, then the AV1 software adapter,
then returns an error including both failures. PreferSoftware reverses that order.
No codec is rejected just because the software adapter does not implement it.
Support queries use the same adapter creation/negotiation as configure; native
queries can allocate a temporary hardware session. No fallback occurs after input
has been consumed. Use `--no-default-features --features software` for software only.
With no features, video decoding reports unsupported; native Opus audio remains
available. Video feature flags never disable audio decoding or the synchronous
Opus packet decoder. Web's asynchronous audio decoder continues to use WebCodecs.

VP9 profiles 0–3 parse successfully; support depends on the adapter and its output
bridge. VP9 requires no description, following the WebCodecs registration.
Web forwards validated codec strings and acceleration preferences to WebCodecs; the
browser does not expose the adapter identity or guarantee hardware execution.
Android NDK codec selection rejects known Android/Google software implementations;
vendor codec hardware classification is not exposed by the supported NDK API level.

Windows uses synchronous/asynchronous MFTs with CPU-readable NV12 output.
D3D-only transforms needing a device manager and GPU surface sharing are not
supported yet. Windows x86 builds with rav1d assembly require NASM.

## AVC / HEVC input

AVC uses `avc1.PPCCLL` / `avc3.PPCCLL`; HEVC uses `hvc1` / `hev1` registry
identifiers with profile-space, compatibility, tier/level and constraint bytes.
Names such as `h264` and `h265` are not WebCodecs codec strings and are rejected.
The parsed Rust representations are `AvcCodec` and `HevcCodec`.

Following the registrations, a present description means an avcC/hvcC record
and length-prefixed access units; absent description means Annex B. Windows and
Android convert configuration records and input NALs to Annex B. VideoToolbox
accepts description records directly, or creates its session from parameter sets
in the first Annex B key chunk. This deferred path cannot validate a specific
stream's hardware profile until the first key chunk arrives. In-band format
changes drain delayed output before replacing the session.

There is no AVC/HEVC software fallback. Linux VA-API AVC/HEVC integration remains
pending and returns Unsupported. Windows/Android still negotiate only 8-bit CPU
output; their high-depth bridges remain pending. No FFmpeg dependency is used.

## API coverage

All requested interface names are exported: AudioData, AudioDecoder, AudioEncoder,
EncodedAudioChunk, EncodedVideoChunk, ImageDecoder, ImageTrack, ImageTrackList,
VideoDecoder, VideoEncoder, VideoColorSpace and VideoFrame.

ImageDecoder and encoders are interface-only: queries return false and creation
or configuration returns Unsupported. Image decoding uses MIME types and async
decode results; no empty successful output is fabricated. VideoColorSpace keeps
nullable registry metadata independently of resolved renderer color transforms.
It is currently a standalone metadata contract, not a decoder override.
AudioData exposes validated f32 buffer construction and f32/interleaved-or-planar
copying; other sample conversion formats are declared but return Unsupported.
Encoded chunks expose byte_length/copy_to. Rust ownership and consuming close
replace JS detached-object lifetimes. VideoFrame buffer constructors/copy formats
and streaming ImageDecoder input are not yet a complete Web IDL implementation.

References: [WebCodecs](https://www.w3.org/TR/webcodecs/),
[AVC registration](https://www.w3.org/TR/webcodecs-avc-codec-registration/),
[HEVC registration](https://www.w3.org/TR/webcodecs-hevc-codec-registration/).
