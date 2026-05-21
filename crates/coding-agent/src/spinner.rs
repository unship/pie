//! Active-prompt spinner. Animates a braille frame to stderr while the agent is thinking
//! (between the user's submission and the first streamed event), then clears its line and
//! exits the moment any agent output is emitted.
//!
//! Previous attempt was racy: the animation task only checked its stop flag on the next
//! tick (up to 80ms later), by which time the cursor had moved to the assistant's line
//! and the spinner's `\r\x1b[2K` cleanup wiped the wrong line. Fix is `stop_sync()`: the
//! listener that flips the flag also emits the clearing escape **immediately**, so by the
//! time the listener's own output starts the cursor is on a clean stderr line.

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_MS: u64 = 80;

#[derive(Clone)]
pub struct SpinnerHandle {
    stop: Arc<AtomicBool>,
    enabled: bool,
}

impl SpinnerHandle {
    /// Flip the stop flag AND emit the line-clear escape directly. Called on the same
    /// thread that's about to print agent output — guarantees the cursor sits at column 0
    /// of a clean stderr line before output starts.
    pub fn stop_sync(&self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            // Already stopped — no-op.
            return;
        }
        if self.enabled {
            use std::io::Write;
            let mut err = std::io::stderr();
            let _ = err.write_all(b"\r\x1b[2K");
            let _ = err.flush();
        }
    }
}

impl Drop for SpinnerHandle {
    fn drop(&mut self) {
        self.stop_sync();
    }
}

/// Start the spinner. Returns a clonable handle — clone one copy into a Drop-on-event
/// listener and keep another in the REPL for a final cleanup.
pub fn start(label: impl Into<String>) -> SpinnerHandle {
    let enabled = std::io::stderr().is_terminal();
    let stop = Arc::new(AtomicBool::new(false));
    if !enabled {
        return SpinnerHandle { stop, enabled };
    }
    let label = label.into();
    let stop_clone = stop.clone();
    tokio::spawn(async move {
        let mut frame = 0usize;
        loop {
            // The animation task only writes when the flag is false. The `stop_sync` call
            // path already cleared the line, so we never emit a stale frame after stop.
            if stop_clone.load(Ordering::SeqCst) {
                break;
            }
            let icon = FRAMES[frame % FRAMES.len()];
            use std::io::Write;
            let mut err = std::io::stderr();
            let _ = write!(err, "\r\x1b[2K{icon} {label}");
            let _ = err.flush();
            frame = frame.wrapping_add(1);
            tokio::time::sleep(Duration::from_millis(FRAME_MS)).await;
        }
    });
    SpinnerHandle { stop, enabled }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_sync_is_idempotent() {
        let h = start("thinking");
        h.stop_sync();
        h.stop_sync(); // must not panic / write twice
    }

    #[test]
    fn frames_present() {
        assert!(!FRAMES.is_empty());
    }
}
