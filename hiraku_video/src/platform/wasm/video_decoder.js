export function hirakuCreateWebVideoDecoder(codec, width, height, onFrame, onError) {
    const state = {
        decoder: null,
        copies: Promise.resolve(),
        pendingCopies: 0,
        firstTimestamp: null,
        storage: null,
    };
    state.decoder = new VideoDecoder({
        output(frame) {
            state.pendingCopies += 1;
            state.copies = state.copies.then(async () => {
                try {
                    const width = frame.codedWidth;
                    const height = frame.codedHeight;
                    const chromaWidth = Math.ceil(width / 2);
                    const chromaHeight = Math.ceil(height / 2);
                    const color = frame.colorSpace;
                    if (state.firstTimestamp === null) state.firstTimestamp = frame.timestamp;
                    const decoded = {
                        timestamp: frame.timestamp - state.firstTimestamp,
                        width,
                        height,
                        fullRange: color.fullRange === true,
                        matrix: color.matrix || "bt709",
                        transfer: color.transfer || "bt709",
                    };
                    try {
                        if (frame.format !== "I420") {
                            throw new DOMException(
                                `native frame format ${frame.format} is not I420`,
                                "NotSupportedError",
                            );
                        }
                        const yLength = width * height;
                        const uLength = chromaWidth * chromaHeight;
                        const options = {
                            rect: frame.codedRect,
                            layout: [
                                { offset: 0, stride: width },
                                { offset: yLength, stride: chromaWidth },
                                { offset: yLength + uLength, stride: chromaWidth },
                            ],
                        };
                        const byteLength = frame.allocationSize(options);
                        if (!state.storage || state.storage.byteLength < byteLength) {
                            state.storage = new Uint8Array(byteLength);
                        }
                        const storage = state.storage.subarray(0, byteLength);
                        await frame.copyTo(storage, options);
                        decoded.y = storage.subarray(0, yLength);
                        decoded.u = storage.subarray(yLength, yLength + uLength);
                        decoded.v = storage.subarray(yLength + uLength);
                    } catch (i420Error) {
                        // Hardware-backed VideoFrames on Safari and some Chromium
                        // configurations cannot be copied directly as I420. Keep
                        // the native YUV path fast, but fall back to browser-owned
                        // RGB conversion instead of failing playback.
                        try {
                            const options = {
                                format: "RGBA",
                                rect: frame.codedRect,
                                layout: [{ offset: 0, stride: width * 4 }],
                            };
                            const byteLength = frame.allocationSize(options);
                            if (!state.storage || state.storage.byteLength < byteLength) {
                                state.storage = new Uint8Array(byteLength);
                            }
                            const storage = state.storage.subarray(0, byteLength);
                            await frame.copyTo(storage, options);
                            decoded.rgba = storage;
                        } catch (rgbaError) {
                            const canvas = typeof OffscreenCanvas !== "undefined"
                                ? new OffscreenCanvas(width, height)
                                : Object.assign(document.createElement("canvas"), { width, height });
                            const context = canvas.getContext("2d", {
                                alpha: false,
                                willReadFrequently: true,
                            });
                            if (!context) throw rgbaError;
                            context.drawImage(frame, 0, 0, width, height);
                            decoded.rgba = new Uint8Array(
                                context.getImageData(0, 0, width, height).data.buffer,
                            );
                        }
                    }
                    onFrame(decoded);
                } catch (error) {
                    onError(String(error));
                } finally {
                    frame.close();
                    state.pendingCopies -= 1;
                }
            });
        },
        error(error) { onError(String(error)); },
    });
    state.decoder.configure({
        codec,
        codedWidth: width,
        codedHeight: height,
        hardwareAcceleration: "prefer-hardware",
        optimizeForLatency: true,
    });
    return state;
}

export function hirakuWebVideoDecode(state, data, timestamp, duration, key) {
    state.decoder.decode(new EncodedVideoChunk({
        type: key ? "key" : "delta",
        timestamp,
        duration,
        data,
    }));
}

export function hirakuWebVideoQueueSize(state) {
    return state.decoder.decodeQueueSize + state.pendingCopies;
}
export function hirakuWebVideoFlush(state) {
    return state.decoder.flush().then(() => state.copies);
}
export function hirakuWebVideoClose(state) {
    if (state && state.decoder && state.decoder.state !== "closed") state.decoder.close();
}
export function hirakuWebYield() { return new Promise(resolve => setTimeout(resolve, 0)); }

export function hirakuCreateWebAudio(bytes) {
    const blob = new Blob([bytes], { type: "audio/webm; codecs=opus" });
    const url = URL.createObjectURL(blob);
    const audio = document.createElement("audio");
    audio.src = url;
    audio.preload = "auto";
    return {
        audio,
        url,
        fallback: false,
        playing: false,
        clockSeconds: 0,
        lastTick: performance.now(),
    };
}
export function hirakuWebAudioPlay(state) {
    if (!state.playing) {
        state.playing = true;
        state.lastTick = performance.now();
    }
    const result = state.audio.play();
    if (result) result.catch(error => {
        state.fallback = true;
        console.warn("Hiraku movie audio could not start; using a silent clock:", error);
    });
}
export function hirakuWebAudioPause(state) {
    if (state.playing) {
        state.clockSeconds += (performance.now() - state.lastTick) / 1000;
        state.playing = false;
    }
    state.audio.pause();
}
export function hirakuWebAudioPosition(state) {
    if (!state.fallback && state.audio.currentTime > 0) return state.audio.currentTime;
    return state.clockSeconds + (state.playing ? (performance.now() - state.lastTick) / 1000 : 0);
}
export function hirakuWebAudioEnded(state) { return state.fallback || state.audio.ended; }
export function hirakuWebAudioClose(state) {
    if (!state) return;
    state.audio.pause();
    state.audio.removeAttribute("src");
    state.audio.load();
    URL.revokeObjectURL(state.url);
}