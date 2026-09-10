# Android and Linux backend work

## Android MediaCodec

The `hardware` feature enables an NDK MediaCodec backend on Android (API 28+).
It consumes AV1 chunks on the existing codec worker and emits owned VideoFrame
data, with no container parsing, JNI application context or rendering surface.
Configuration failures fall back to rav1d; PreferSoftware bypasses MediaCodec.
MediaCodec's MIME lookup may select a platform software codec: this is not a
promise that every Android device has hardware AV1 decoding.

The initial bridge supports linear 8-bit I420 and NV12, checked output ranges,
strides, even-origin crop rectangles, SDR color metadata, EOS draining and
continuing after flush with a keyframe. Flexible, opaque, tiled and HDR output
are rejected rather than misinterpreted. Errors after stream submission are
reported, not silently restarted with a software decoder that lacks references.
GPU surface sharing and broader pixel-format negotiation remain future work.

The decoder has bounded dequeue waits with cancellation checks. A vendor call
that itself hangs cannot be interrupted by Rust. Codec resources remain on
their worker thread; copying occurs before returning buffers to the codec.

Validation so far: host unit tests for checked plane copies, overflow, short
buffers and odd image sizes. Full Android compilation is blocked locally by
the missing NDK `aarch64-linux-android-clang`; no Android playback has been
verified. Do not treat this initial backend as device-tested.

## Linux VA-API

Enable with `--features hiraku-media/vaapi` from a workspace consumer, or
`cargo test -p hiraku-media --features vaapi --lib` in this workspace.
The feature is opt-in because it needs Linux libva/GBM/DRM development libraries,
pkg-config and Clang/libclang. The x86 rav1d fallback also needs NASM.

No FFmpeg/libavcodec dependency or implementation is included. The backend uses
cros-codecs 0.0.6 and cros-libva 0.0.12, both BSD-3-Clause. The resolved Rust
dependency tree was inspected: no GPL/LGPL dependency was found. Retain upstream
notices when distributing; separately review the system drivers you package.

The adapter enumerates DRM render nodes, checks AV1 Profile 0/VLD capability,
and probes NV12 GBM allocation/import before choosing the device. PreferSoftware
bypasses the probe; unavailable devices fall back to rav1d before accepting
input. Some desktop GBM implementations do not support upstream's video decode
allocation flags; those devices also fall back rather than assuming every
VA-API driver is compatible with this first backend.

AV1 configuration OBUs are extracted from av1C. Partial OBU consumption, format
events and pool replacement are handled explicitly. The upstream surface pool
retains codec reference frames and returns reusable buffers on handle release.
All driver calls occur on the existing worker; no extra C2 worker is created.
NV12 is copied through checked plane layouts to owned I420 output before the
driver buffers are released. This is not GPU zero-copy. Signed timestamps and
SDR sequence color metadata are preserved; HDR and unsupported layouts fail.

Flush drains before returning Flushed. Midstream failures are reported, not
replayed through a software decoder lacking reference state. Cancellation is
checked between operations, but a hung synchronous driver call cannot be
interrupted. GPU output remains a future optimization.

Host validation: 11 media tests passed, including av1C extraction and malformed
plane/progress checks. Full Linux cross-compilation is not yet verified: the
local environment lacks NASM and Linux system development libraries, and its
Docker daemon is unavailable. A Linux CI job is provided for feature-on/off
compilation and headless tests; it has not been run remotely by this change.
Real VA-API playback still requires a supported device and driver.

References:
- https://developer.android.com/ndk/reference/group/media
- https://github.com/chromeos/cros-codecs
- https://github.com/chromeos/cros-libva
