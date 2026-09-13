//! Build-only reporting. Encoding itself provides no intra-image percentage.
use std::{
    io::Write,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
mod platform;

#[derive(Clone, Debug)]
pub struct PackProgress {
    pub phase: &'static str,
    pub completed: usize,
    pub total: usize,
    pub detail: String,
}
impl PackProgress {
    pub fn new(
        phase: &'static str,
        completed: usize,
        total: usize,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            completed,
            total,
            detail: detail.into(),
        }
    }
    fn label(&self) -> String {
        let count = if self.total == 0 {
            String::new()
        } else {
            format!(" {}/{}", self.completed, self.total)
        };
        // A filename cannot inject terminal escape sequences or log lines.
        let detail: String = self
            .detail
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        format!("[HDP {}{}] {}", self.phase, count, detail)
    }
}

enum Message {
    Update(PackProgress),
    Stop(Option<String>),
}

/// One background reporter, never one thread per texture. Progress failures
/// cannot fail a build. Drop joins promptly, including on early error/unwind.
pub struct TerminalProgress {
    sender: mpsc::Sender<Message>,
    worker: Option<thread::JoinHandle<()>>,
}
impl TerminalProgress {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let output = platform::output();
        let worker = thread::Builder::new()
            .name("hdp-progress".into())
            .spawn(move || render(receiver, output))
            .ok();
        Self { sender, worker }
    }
    pub fn update(&self, progress: PackProgress) {
        let _ = self.sender.send(Message::Update(progress));
    }
    pub fn finish(mut self, error: Option<String>) {
        let _ = self.sender.send(Message::Stop(error));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Default for TerminalProgress {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for TerminalProgress {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.sender.send(Message::Stop(Some(
                "build interrupted before completion".into(),
            )));
            let _ = worker.join();
        }
    }
}

fn render(receiver: mpsc::Receiver<Message>, mut output: impl Write) {
    let started = Instant::now();
    let mut stage_started = started;
    let mut last_print = started;
    let mut current: Option<PackProgress> = None;
    loop {
        match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Message::Update(next)) => {
                let changed_phase = current.as_ref().is_none_or(|old| old.phase != next.phase);
                if changed_phase {
                    // Concurrent workers interleave phases. A new event does
                    // not mean the previous worker's operation has finished.
                    stage_started = Instant::now();
                }
                if changed_phase || last_print.elapsed() >= Duration::from_secs(2) {
                    let _ = writeln!(
                        output,
                        "{} | total {:.1}s",
                        next.label(),
                        started.elapsed().as_secs_f64()
                    );
                    let _ = output.flush();
                    last_print = Instant::now();
                }
                current = Some(next);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(current) = &current {
                    let _ = writeln!(
                        output,
                        "{} | still waiting: stage {:.0}s, total {:.0}s (no internal encoder progress available)",
                        current.label(),
                        stage_started.elapsed().as_secs_f64(),
                        started.elapsed().as_secs_f64()
                    );
                    let _ = output.flush();
                }
            }
            Ok(Message::Stop(error)) => {
                if let Some(error) = error {
                    let detail = PackProgress::new("FAILED", 0, 0, error).label();
                    let _ = writeln!(output, "{detail} | {:.1}s", started.elapsed().as_secs_f64());
                    if let Some(current) = current {
                        let _ = writeln!(output, "Last operation: {}", current.label());
                    }
                }
                let _ = output.flush();
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_escapes_paths_and_reports_completed_count_not_fake_encode_percentage() {
        let value = PackProgress::new("encode", 2, 8, "alice\n\x1b[31m.png");
        assert_eq!(value.label(), "[HDP encode 2/8] alice  [31m.png");
        assert!(!value.label().contains('%'));
    }
    #[test]
    fn worker_stops_without_waiting_for_heartbeat() {
        let (tx, rx) = mpsc::channel();
        tx.send(Message::Update(PackProgress::new(
            "encode",
            0,
            1,
            "alice.png",
        )))
        .expect("update");
        tx.send(Message::Stop(Some("synthetic failure".into())))
            .expect("stop");
        let mut output = Vec::new();
        render(rx, &mut output);
        let output = String::from_utf8(output).expect("UTF-8 progress log");
        assert!(output.contains("[HDP encode 0/1] alice.png"));
        assert!(output.contains("[HDP FAILED] synthetic failure"));
        assert!(output.contains("Last operation: [HDP encode 0/1] alice.png"));
    }
}
