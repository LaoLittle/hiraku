# UASTC textures

`hiraku-uastc` registers an Image loader for `.uastc.ktx2`. HirakuEngine installs
the plugin automatically. Runtime transcoding uses the pinned pure-Rust
[Hiraku basisu fork](https://github.com/Hiraku-Project/basisu), without its zstd
feature or any C++ encoder dependency.

2D UASTC LDR textures select ASTC 4x4, then BC7, then RGBA8 according to device
support. Odd-sized base levels use RGBA8 to preserve pixel dimensions without
violating GPU compressed-block alignment. sRGB/linear transfer comes from the
KTX2 DFD. Arrays, cubemaps, video and other Basis codecs are rejected. The current
dimension limit is 16384 per axis. The runtime accepts mip chains; the build
pipeline currently writes only the base level to avoid atlas mip bleeding.

The loader uses Bevy's `ImageLoaderSettings`, including sampler and asset usage.
Setting `is_srgb = false` selects linear sampling of the same blocks (needed for
curtain/dissolve masks), overriding an sRGB DFD without changing pixel bytes.
Explicit texture-format overrides may select the linear/sRGB counterpart but
cannot change block layout. Array-layout reinterpretation is rejected explicitly.

## Build-time pipeline

Use `hiraku-uastc-build` (the `build/` package) **only as a build dependency**.
It depends on `basis-universal` for encoding; this C++ dependency never enters
the engine's runtime/wasm dependency graph.

`pack_directory(root, output, PackOptions)`:

1. Reads `.texture.hson` descriptors and resolves their relative `image` paths.
2. Deduplicates canonical source paths and assigns deterministic encoded paths.
3. Rewrites only packaged HSON, preserving source descriptors and region values.
4. Encodes each referenced image once as UASTC; wraps the encoder's one-slice
   `.basis` payload in KTX2 with no internal supercompression.
5. Feeds KTX2 bytes to the HDP streaming writer. Unreferenced image files are
   excluded. Scripts, HSON, audio and other non-image files remain included.

No intermediate `.basis` or `.ktx2` files are produced. Each active encoder needs
its source texture and internal buffers in memory. Completed outputs queue in
memory without a memory cap until the single writer reaches their input index.
HDP stores bounded
compressed chunks in a temporary spool, then writes headers/index and copies
those chunks into desktop or web volumes. The spool is removed on ordinary
success/error; process termination can leave it behind.

Inputs currently supported by the encoder are PNG/JPEG/WebP/BMP/TGA and existing
`.uastc.ktx2` payloads (passed through). Texture references must remain within
the source package; external URLs and symlinks are rejected. Existing KTX2
payloads must satisfy the runtime loader's constraints.

Manosabars uses this pipeline for wasm builds in `build.rs`. Changing source assets triggers
re-encoding on the next build; no persistent texture encoding cache is provided.

## Terminal build progress

Manosabars enables `TerminalProgress` around `pack_directory_with_progress`.
It reports discovery, manifest planning, ordinary asset compression, image
decoding, UASTC encoding, KTX2 compression and volume publication. Texture counts
mean written textures, not encoder-internal percentages. Concurrent encoding
events identify individual files without presenting their index as a completed
count. The worker-plan event reports the CPU/thread allocation. A five-second
heartbeat identifies the latest reported stage
during long operations. A heartbeat means the reporter is alive, not proof that
the native encoder is making progress. Failures include the last operation.

Cargo normally captures build-script stdout/stderr, so interactive progress
writes directly to the host's controlling terminal (`/dev/tty` or `CONOUT$`).
In CI/no-terminal environments, use `cargo build -vv` to see the stderr fallback.
Set `HIRAKU_BUILD_PROGRESS=stderr` to force captured logs, or `off` to disable
progress. Platform-specific console access is isolated in `progress/platform`.
The quiet `pack_directory` API remains available for library callers/tests.

## Encoder performance

Encoding uses UASTC effort level 3. The CPU budget is the minimum of host
parallelism and Cargo's `NUM_JOBS`. When possible, one CPU is reserved for HDP
compression; the remainder is distributed across image workers, aiming for four
threads per image (up to eight when there are few images). Each worker repeatedly
claims another image without waiting for earlier images to finish. The writer
consumes outputs in sorted source order, preserving stable package ordering and
descriptor deduplication. Queued output memory is not capped. Errors stop new
jobs and join active encoders before returning; failed encoding is never published.
Manosabars optimizes build dependencies even in debug builds. Other consuming
workspaces should set `profile.dev/release.build-override.opt-level = 3`, or at
least `profile.dev/release.package.basis-universal-sys.opt-level = 3`: Cargo
otherwise leaves build dependencies unoptimized, including the C++ encoder.
Dependency-local profile settings do not propagate to consuming workspaces.

Use `cargo run -p hiraku-uastc-build --example encode_timing -- IMAGE [THREADS]`
for an isolated image decode/encode timing. It writes no assets and launches no
game. Timing and output size are not directly comparable with newer `basisu`
executables with KTX2 Zstd enabled: this build dependency pins an older encoder,
and our KTX2 payload remains uncompressed until HDP chunk compression.
