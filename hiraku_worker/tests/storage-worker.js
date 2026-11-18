// Absolute URLs also work when the browser test runner wraps Worker in a blob.
const base = self.location.origin + "/";
try {
    importScripts(base + "hiraku-storage-worker.js");
    wasm_bindgen(base + "hiraku-storage-worker_bg.wasm").catch(error => {
        self.postMessage({ fatal: String(error) });
    });
} catch (error) {
    self.postMessage({ fatal: String(error) });
}
