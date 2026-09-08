//! These checks describe the build host, never the game's target platform.
use std::io::Write;

#[cfg(unix)]
fn terminal() -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).open("/dev/tty")
}
#[cfg(windows)]
fn terminal() -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).open("CONOUT$")
}
#[cfg(not(any(unix, windows)))]
fn terminal() -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no terminal device",
    ))
}

pub(super) fn output() -> Box<dyn Write + Send> {
    // Ordinary Cargo hides build-script stdout/stderr. The controlling console
    // remains visible. Explicit stderr mode is useful for `cargo build -vv`/CI.
    if std::env::var("HIRAKU_BUILD_PROGRESS").as_deref() == Ok("off") {
        return Box::new(std::io::sink());
    }
    if std::env::var("HIRAKU_BUILD_PROGRESS").as_deref() != Ok("stderr") {
        if let Ok(file) = terminal() {
            return Box::new(file);
        }
    }
    Box::new(std::io::stderr())
}
