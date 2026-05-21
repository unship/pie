//! Animated spinner that runs to stderr while a prompt is in flight. Owns its tokio task
//! lifetime — drop the `SpinnerHandle` to stop the animation cleanly.
//!
//! Goes to stderr deliberately so it doesn't interleave with stdout streaming text. Only
//! activates when stderr is a TTY; non-TTY runs (CI, pipes) get a silent no-op spinner.
//!
//! Not used by the current line-based REPL: `\r\x1b[2K` races against streamed stdout
//! output and corrupts scrollback. Kept here for the raw-mode TUI renderer (issue #2 main
//! deliverable) that will own the cursor and can drive the spinner safely.

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_MS: u64 = 80;

pub struct SpinnerHandle {
    stop: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl SpinnerHandle {
    /// Stop animation and wait for the task to drain. Always called on drop, but exposed for
    /// callers that want deterministic shutdown before printing more output.
    pub async fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }

    /// Shared stop flag — clone into a listener that fires the moment streamed output starts,
    /// so the spinner's `\r\x1b[2K` line-erase doesn't fight with the stream. The held
    /// `SpinnerHandle` can still call `stop().await` later as an idempotent final cleanup.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }
}

impl Drop for SpinnerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Best-effort: detach the task; it'll see the flag on next frame and exit.
    }
}

/// Start a spinner with `label`. Returns a `SpinnerHandle`; calling `.stop().await` clears
/// the spinner line. If stderr isn't a TTY, returns a no-op handle.
pub fn start(label: impl Into<String>) -> SpinnerHandle {
    let stop = Arc::new(AtomicBool::new(false));
    if !std::io::stderr().is_terminal() {
        return SpinnerHandle { stop, task: None };
    }
    let label = label.into();
    let stop_clone = stop.clone();
    let task = tokio::spawn(async move {
        let mut frame = 0usize;
        let mut printed_at_least_once = false;
        loop {
            if stop_clone.load(Ordering::SeqCst) {
                // Only erase the line if we owned it (i.e. printed at least one frame).
                // Otherwise we'd `\r\x1b[2K` a line that's currently holding streamed
                // assistant output and wipe the user's content. After the flag flips, the
                // next-line newline we emit here keeps subsequent output cleanly separated.
                if printed_at_least_once {
                    eprint!("\r\x1b[2K");
                    use std::io::Write;
                    let _ = std::io::stderr().flush();
                }
                break;
            }
            let icon = FRAMES[frame % FRAMES.len()];
            // \r returns to column 0; \x1b[2K erases the line; then we re-draw. Safe only
            // when nothing else is writing to this line — callers stop us before streaming
            // output begins (see Tui::AgentStart in main.rs).
            eprint!("\r\x1b[2K{icon} {label}");
            use std::io::Write;
            let _ = std::io::stderr().flush();
            printed_at_least_once = true;
            frame = frame.wrapping_add(1);
            tokio::time::sleep(Duration::from_millis(FRAME_MS)).await;
        }
    });
    SpinnerHandle {
        stop,
        task: Some(task),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_tty_handle_is_no_op() {
        // In test runs, stderr is rarely a TTY → spawn returns a None-task handle.
        let h = start("thinking");
        // No assertions on output (tty-dependent); just verify drop / stop don't panic.
        h.stop().await;
    }

    #[test]
    fn frames_are_non_empty() {
        assert!(!FRAMES.is_empty());
        assert!(FRAMES.iter().all(|f| !f.is_empty()));
    }
}
