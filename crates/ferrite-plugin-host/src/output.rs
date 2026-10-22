//! Guest output handling (design 05 §5): stdout/stderr are captured with a size cap and
//! logged after the call; the `log` import is size- and count-limited and sanitised.

use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::AsyncWrite;
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi_io::poll::Pollable;
use wasmtime_wasi_io::streams::{OutputStream, StreamResult};

/// Most bytes kept per stream per call; the rest is counted and dropped.
pub(crate) const STREAM_CAP: usize = 64 * 1024;
/// Longest `log.log` message kept, in bytes.
const LOG_MESSAGE_CAP: usize = 4 * 1024;
/// `log.log` calls honoured per plugin call.
const LOG_CALLS_MAX: u32 = 100;

#[derive(Default)]
struct Buffer {
    kept: Vec<u8>,
    discarded: u64,
}

/// A stdout/stderr replacement that keeps the first [`STREAM_CAP`] bytes and silently
/// discards (but counts) the rest. It never reports an error or a closed stream:
/// `wasmtime_wasi::p2::pipe::MemoryOutputPipe` fails the write past its capacity and
/// closes the stream, which makes Rust's `println!` panic inside the guest.
#[derive(Clone, Default)]
pub(crate) struct CappedPipe {
    buf: Arc<Mutex<Buffer>>,
}

impl CappedPipe {
    fn lock(&self) -> MutexGuard<'_, Buffer> {
        self.buf.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, bytes: &[u8]) {
        let mut buf = self.lock();
        let room = STREAM_CAP.saturating_sub(buf.kept.len());
        let take = room.min(bytes.len());
        buf.kept.extend_from_slice(&bytes[..take]);
        buf.discarded += (bytes.len() - take) as u64;
    }

    /// Captured text (lossy UTF-8) and the number of bytes that did not fit.
    fn snapshot(&self) -> (String, u64) {
        let buf = self.lock();
        (
            String::from_utf8_lossy(&buf.kept).into_owned(),
            buf.discarded,
        )
    }

    /// Like [`Self::snapshot`] but escaped, safe to put in a log line.
    fn loggable(&self) -> (String, u64) {
        let (text, discarded) = self.snapshot();
        (escape_text(&text), discarded)
    }
}

#[async_trait::async_trait]
impl Pollable for CappedPipe {
    async fn ready(&mut self) {}
}

impl OutputStream for CappedPipe {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.push(&bytes);
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(STREAM_CAP)
    }
}

impl AsyncWrite for CappedPipe {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.push(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl IsTerminal for CappedPipe {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for CappedPipe {
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }

    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

/// Characters that could forge log lines or disguise text in a terminal or log viewer:
/// control chars, line/paragraph separators and bidi overrides/isolates.
fn needs_escape(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Replace [`needs_escape`] characters with their `\n` / `\u{..}` forms.
fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if needs_escape(c) {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Truncate to [`LOG_MESSAGE_CAP`] bytes (on a char boundary) and escape control
/// characters, so a message cannot forge extra log lines or terminal sequences.
pub(crate) fn sanitize_log_message(message: &str) -> String {
    let mut end = message.len().min(LOG_MESSAGE_CAP);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end);
    out.push_str(&escape_text(&message[..end]));
    if end < message.len() {
        out.push_str("…[truncated]");
    }
    out
}

/// Everything a guest can emit besides return values, for one plugin call. Dropping it
/// (when the store goes away, whether the call succeeded or not) logs the captured
/// stdout/stderr and reports dropped `log.log` calls.
pub(crate) struct GuestOutput {
    plugin: Arc<str>,
    pub(crate) stdout: CappedPipe,
    pub(crate) stderr: CappedPipe,
    log_calls: u32,
}

impl GuestOutput {
    pub(crate) fn new(plugin: Arc<str>) -> Self {
        Self {
            plugin,
            stdout: CappedPipe::default(),
            stderr: CappedPipe::default(),
            log_calls: 0,
        }
    }

    /// Count one `log.log` call; `false` once [`LOG_CALLS_MAX`] were already made.
    pub(crate) fn admit_log(&mut self) -> bool {
        let admitted = self.log_calls < LOG_CALLS_MAX;
        self.log_calls = self.log_calls.saturating_add(1);
        admitted
    }
}

impl Drop for GuestOutput {
    fn drop(&mut self) {
        let plugin = &*self.plugin;
        for (stream, pipe) in [("stdout", &self.stdout), ("stderr", &self.stderr)] {
            let (text, truncated_bytes) = pipe.loggable();
            if !text.is_empty() || truncated_bytes > 0 {
                tracing::info!(plugin, stream, truncated_bytes, "{text}");
            }
        }
        if let Some(dropped) = self.log_calls.checked_sub(LOG_CALLS_MAX).filter(|n| *n > 0) {
            tracing::warn!(
                plugin,
                dropped,
                "log calls over the per-call limit were dropped"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_pipe_keeps_prefix_and_counts_rest() {
        let mut pipe = CappedPipe::default();
        let chunk = Bytes::from(vec![b'a'; 40 * 1024]);
        assert_eq!(pipe.check_write().ok(), Some(STREAM_CAP));
        for _ in 0..3 {
            pipe.write(chunk.clone()).expect("never errors");
        }
        let (text, discarded) = pipe.snapshot();
        assert_eq!(text.len(), STREAM_CAP);
        assert_eq!(discarded, (3 * 40 * 1024 - STREAM_CAP) as u64);
        // Still writable (not "closed") once full.
        assert!(pipe.write(Bytes::from_static(b"more")).is_ok());
        assert_eq!(pipe.check_write().ok(), Some(STREAM_CAP));
        assert_eq!(pipe.snapshot().1, discarded + 4);
    }

    #[test]
    fn log_messages_are_truncated() {
        // 3-byte chars: 4096 = 1365 * 3 + 1, so the cut lands mid-character.
        let long = "€".repeat(LOG_MESSAGE_CAP);
        let out = sanitize_log_message(&long);
        let kept = out.strip_suffix("…[truncated]").expect("truncation marker");
        assert!(kept.len() <= LOG_MESSAGE_CAP);
        assert_eq!(kept.chars().count(), LOG_MESSAGE_CAP / 3);
        assert!(kept.chars().all(|c| c == '€'));
        assert_eq!(sanitize_log_message("short"), "short");
        let exact = "x".repeat(LOG_MESSAGE_CAP);
        assert_eq!(sanitize_log_message(&exact), exact);
    }

    #[test]
    fn captured_output_is_escaped() {
        let pipe = CappedPipe::default();
        pipe.push("ok\nINFO forged\n\u{1b}[31mred\u{202e}evil\u{2028}".as_bytes());
        let (text, _) = pipe.loggable();
        assert_eq!(
            text,
            "ok\\nINFO forged\\n\\u{1b}[31mred\\u{202e}evil\\u{2028}"
        );
        assert!(!text.contains(['\n', '\u{1b}', '\u{202e}', '\u{2028}']));
    }

    #[test]
    fn log_messages_are_escaped() {
        assert_eq!(
            sanitize_log_message("a\nINFO fake\r\t\u{1b}[31m"),
            "a\\nINFO fake\\r\\t\\u{1b}[31m"
        );
        assert_eq!(
            sanitize_log_message("\u{2067}x\u{2029}"),
            "\\u{2067}x\\u{2029}"
        );
    }

    #[test]
    fn log_calls_are_capped_at_100() {
        let mut out = GuestOutput::new("t".into());
        let admitted = (0..250).filter(|_| out.admit_log()).count();
        assert_eq!(admitted, 100);
        assert_eq!(out.log_calls, 250);
    }
}
