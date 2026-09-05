# Hiraku Media

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
use hiraku_media::{AudioDecoder, AudioDecoderConfig, DecoderEvent};

let mut decoder = AudioDecoder::new()?;
decoder.configure(AudioDecoderConfig::new("opus", 48_000, 2))?;
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
discards outstanding flush tokens, and `Drop` handles resource release. Encoders,
image decoders and GPU-native frame handles are not implemented yet.

## Backend support

Codec identifiers are open strings, not an exhaustive codec enum. New codecs can
be added in backend dispatch without changing the API.

| Backend | Current support |
| --- | --- |
| Software | AV1 profile 0, 8-bit 4:2:0 via rav1d; mono/stereo Opus via hiraku-opus |
| macOS | AV1 VideoToolbox with software fallback |
| Windows | AV1 Media Foundation with software fallback |
| Web | Configuration and chunks forwarded to browser WebCodecs; support depends on the browser |

Native work runs on dedicated workers with bounded output queues and cancellable
sends. Browser bindings are private to `platform/wasm`, including the audio types
that web-sys gates as unstable; no `web_sys_unstable_apis` rustc flag is required.
Raw-frame copy conversion currently supports SDR I420/NV12 or RGBA; HDR tone
mapping and higher-bit-depth output are not implemented.

The default `hardware` feature enables native platform backends.
`PreferHardware` and `PreferSoftware` are hints. Disable default features to use
native software decoding directly.

Windows uses synchronous/asynchronous MFTs with CPU-readable NV12 output.
D3D-only transforms needing a device manager and GPU surface sharing are not
supported yet. Windows x86 builds with rav1d assembly require NASM.
