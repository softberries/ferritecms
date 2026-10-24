//! Outgoing mail (M2a-2 P17): the [`Mailer`] trait the SMTP transport
//! (`ferrite_http::mail::SmtpMailer`) and the test [`MemoryMailer`] implement, the
//! header-injection rules every mailer applies ([`OutgoingMail::check`]), and the
//! node's [`MailQueue`], which sends from bounded background tasks so no caller (a
//! plugin call, a request) waits for SMTP.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore, watch};

/// Most recipients (`to` + `bcc`) of one mail.
pub const MAX_RECIPIENTS: usize = 50;
/// Longest subject, in characters (a plugin subject is at most 200 plus the site-title
/// prefix).
pub const MAX_SUBJECT_CHARS: usize = 400;
/// Largest text body, in bytes.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
/// Longest address, in bytes (RFC 5321 path limit).
pub const MAX_ADDRESS_BYTES: usize = 254;
/// Most mails one node sends at once; [`MailQueue`] refuses more.
pub const MAIL_IN_FLIGHT: usize = 16;
/// How long one queued send may take in total (the SMTP timeout).
pub const MAIL_SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// One plain-text mail. Every address in `to` sees the others; `bcc` recipients get the
/// mail without appearing in it (with an empty `to`, the SMTP mailer shows its own
/// `from` address as `To`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMail {
    pub to: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: String,
}

/// A refused or failed send. The text never contains a body, a password or the SMTP
/// URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MailError(pub String);

impl MailError {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// A mail transport.
#[async_trait::async_trait]
pub trait Mailer: Send + Sync {
    /// Sends `mail`; implementations call [`OutgoingMail::check`] first.
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError>;
}

/// Whether `s` is safe inside a header: no control character (CR, LF, NUL, DEL, C1)
/// and no Unicode line or paragraph separator. Unsafe text is refused, never folded.
pub fn header_safe(s: &str) -> bool {
    !s.chars()
        .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
}

/// A bare address `local@domain`: 1..=[`MAX_ADDRESS_BYTES`] bytes, header-safe, no
/// whitespace and none of `<>()[],;:"\`, exactly one `@` with text on both sides.
pub fn check_address(addr: &str) -> Result<(), MailError> {
    let bad = |why: &str| Err(MailError::new(format!("invalid e-mail address: {why}")));
    if addr.is_empty() || addr.len() > MAX_ADDRESS_BYTES {
        return bad("empty or too long");
    }
    if !header_safe(addr)
        || addr
            .chars()
            .any(|c| c.is_whitespace() || "<>()[],;:\"\\".contains(c))
    {
        return bad("forbidden character");
    }
    match addr.split_once('@') {
        Some((local, domain))
            if !local.is_empty() && !domain.is_empty() && !domain.contains('@') =>
        {
            Ok(())
        }
        _ => bad("expected local@domain"),
    }
}

impl OutgoingMail {
    /// The rules every [`Mailer`] applies before sending: 1..=[`MAX_RECIPIENTS`]
    /// addresses, each passing [`check_address`]; a non-empty header-safe subject of at
    /// most [`MAX_SUBJECT_CHARS`]; a body of at most [`MAX_TEXT_BYTES`].
    pub fn check(&self) -> Result<(), MailError> {
        let count = self.to.len() + self.bcc.len();
        if count == 0 {
            return Err(MailError::new("a mail needs at least one recipient"));
        }
        if count > MAX_RECIPIENTS {
            return Err(MailError::new(format!(
                "a mail has at most {MAX_RECIPIENTS} recipients"
            )));
        }
        for addr in self.to.iter().chain(&self.bcc) {
            check_address(addr)?;
        }
        if self.subject.trim().is_empty() {
            return Err(MailError::new("the subject is empty"));
        }
        if !header_safe(&self.subject) {
            return Err(MailError::new("the subject contains control characters"));
        }
        if self.subject.chars().count() > MAX_SUBJECT_CHARS {
            return Err(MailError::new(format!(
                "the subject is longer than {MAX_SUBJECT_CHARS} characters"
            )));
        }
        if self.text.len() > MAX_TEXT_BYTES {
            return Err(MailError::new(format!(
                "the body is larger than {MAX_TEXT_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// A [`Mailer`] that records what it is sent (tests in every crate).
#[derive(Default)]
pub struct MemoryMailer {
    sent: Mutex<Vec<OutgoingMail>>,
    changed: Notify,
}

impl MemoryMailer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every mail accepted so far, in order.
    pub fn sent(&self) -> Vec<OutgoingMail> {
        self.lock().clone()
    }

    /// Waits until at least `n` mails were accepted or `timeout` passed, then returns
    /// [`sent`](Self::sent) (F32: sends run in background tasks). A test expecting no
    /// mail waits for a positive case first, then calls this with a short grace.
    pub async fn wait_for(&self, n: usize, timeout: Duration) -> Vec<OutgoingMail> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let sent = self.sent();
            if sent.len() >= n {
                return sent;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return self.sent();
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<OutgoingMail>> {
        // Every update is one push: the list stays consistent after a panic.
        self.sent.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl Mailer for MemoryMailer {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError> {
        mail.check()?;
        self.lock().push(mail);
        self.changed.notify_waiters();
        Ok(())
    }
}

/// The node's outgoing-mail queue: a [`Mailer`] whose `send` checks the mail, takes one
/// of [`MAIL_IN_FLIGHT`] slots (none free: refused) and returns, while a background
/// task sends through the wrapped mailer, bounded by [`MAIL_SEND_TIMEOUT`]. The
/// outcome of the send is logged (recipient count and error, never the body or the
/// addresses). [`shutdown`](Self::shutdown) refuses new mail, cancels sends still
/// running and waits for their tasks.
pub struct MailQueue {
    mailer: Arc<dyn Mailer>,
    slots: Arc<Semaphore>,
    capacity: usize,
    timeout: Duration,
    stop: watch::Sender<bool>,
}

impl MailQueue {
    pub fn new(mailer: Arc<dyn Mailer>) -> Self {
        Self::with_limits(mailer, MAIL_IN_FLIGHT, MAIL_SEND_TIMEOUT)
    }

    /// A queue with `capacity` (at least 1) concurrent sends of at most `timeout` each.
    pub fn with_limits(mailer: Arc<dyn Mailer>, capacity: usize, timeout: Duration) -> Self {
        let capacity = capacity.clamp(1, Semaphore::MAX_PERMITS);
        Self {
            mailer,
            slots: Arc::new(Semaphore::new(capacity)),
            capacity,
            timeout,
            stop: watch::Sender::new(false),
        }
    }

    /// Refuses new mail, cancels the sends still running and waits up to `grace` for
    /// their tasks to end.
    pub async fn shutdown(&self, grace: Duration) {
        self.stop.send_replace(true);
        let all = u32::try_from(self.capacity).unwrap_or(u32::MAX);
        if tokio::time::timeout(grace, self.slots.acquire_many(all))
            .await
            .is_err()
        {
            tracing::warn!("mail sends did not stop within {grace:?}");
        }
    }
}

#[async_trait::async_trait]
impl Mailer for MailQueue {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError> {
        mail.check()?;
        if *self.stop.borrow() {
            return Err(MailError::new("mail is shutting down"));
        }
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            return Err(MailError::new(
                "too many mails are being sent; try again later",
            ));
        };
        let mailer = self.mailer.clone();
        let timeout = self.timeout;
        let mut stop = self.stop.subscribe();
        let recipients = mail.to.len() + mail.bcc.len();
        tokio::spawn(async move {
            let _slot = slot;
            tokio::select! {
                _ = stop.wait_for(|stopped| *stopped) => {
                    tracing::warn!(recipients, "mail send cancelled by shutdown");
                }
                sent = tokio::time::timeout(timeout, mailer.send(mail)) => match sent {
                    Ok(Ok(())) => tracing::info!(recipients, "mail sent"),
                    Ok(Err(e)) => tracing::warn!(recipients, error = %e, "mail send failed"),
                    Err(_) => tracing::warn!(recipients, "mail send timed out after {timeout:?}"),
                },
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(subject: &str) -> OutgoingMail {
        OutgoingMail {
            to: vec!["a@example.com".into()],
            bcc: Vec::new(),
            subject: subject.into(),
            text: "body".into(),
        }
    }

    #[test]
    fn header_injection_is_refused() {
        assert_eq!(mail("Hello").check(), Ok(()));
        for subject in [
            "Hi\r\nBcc: victim@evil.test",
            "Hi\nX: y",
            "Hi\rX",
            "nul\0",
            "del\u{7f}",
            "c1\u{85}",
            "ls\u{2028}",
            "ps\u{2029}",
            "",
            "   ",
        ] {
            assert!(mail(subject).check().is_err(), "{subject:?}");
        }
        assert!(mail(&"é".repeat(MAX_SUBJECT_CHARS)).check().is_ok());
        assert!(mail(&"é".repeat(MAX_SUBJECT_CHARS + 1)).check().is_err());
        for addr in [
            "a@example.com\r\nBcc: x@evil.test",
            "a@example.com\n",
            "a b@example.com",
            "a@example.com, b@evil.test",
            "Name <a@example.com>",
            "a@b@c",
            "@example.com",
            "a@",
            "",
        ] {
            let m = OutgoingMail {
                to: Vec::new(),
                bcc: vec![addr.into()],
                ..mail("Hi")
            };
            assert!(m.check().is_err(), "{addr:?}");
        }
        let none = OutgoingMail {
            to: Vec::new(),
            ..mail("Hi")
        };
        assert!(none.check().is_err());
        let many = OutgoingMail {
            to: Vec::new(),
            bcc: vec!["a@example.com".into(); MAX_RECIPIENTS + 1],
            ..mail("Hi")
        };
        assert!(many.check().is_err());
        let big = OutgoingMail {
            text: "x".repeat(MAX_TEXT_BYTES + 1),
            ..mail("Hi")
        };
        assert!(big.check().is_err());
    }

    /// Never answers: a stuck SMTP server.
    struct Stuck;

    #[async_trait::async_trait]
    impl Mailer for Stuck {
        async fn send(&self, _mail: OutgoingMail) -> Result<(), MailError> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn queue_sends_in_the_background() {
        let memory = Arc::new(MemoryMailer::new());
        let queue = MailQueue::new(memory.clone());
        assert_eq!(queue.send(mail("One")).await, Ok(()));
        let sent = memory.wait_for(1, Duration::from_secs(5)).await;
        assert_eq!(sent, [mail("One")]);
        // Checked before queueing: the caller sees the refusal.
        assert!(queue.send(mail("Bad\r\nX: y")).await.is_err());
        queue.shutdown(Duration::from_secs(1)).await;
        assert_eq!(
            queue.send(mail("Late")).await,
            Err(MailError::new("mail is shutting down"))
        );
        assert_eq!(memory.wait_for(2, Duration::from_millis(50)).await.len(), 1);
    }

    #[tokio::test]
    async fn queue_caps_in_flight_sends_and_times_out() {
        let queue = MailQueue::with_limits(Arc::new(Stuck), 2, Duration::from_millis(100));
        assert_eq!(queue.send(mail("1")).await, Ok(()));
        assert_eq!(queue.send(mail("2")).await, Ok(()));
        assert!(queue.send(mail("3")).await.is_err(), "both slots taken");
        // The send timeout frees the slots.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(queue.send(mail("4")).await, Ok(()));
    }

    #[tokio::test]
    async fn shutdown_cancels_stuck_sends_promptly() {
        let queue = MailQueue::with_limits(Arc::new(Stuck), 4, Duration::from_secs(3600));
        for i in 0..4 {
            assert_eq!(queue.send(mail(&i.to_string())).await, Ok(()));
        }
        let started = std::time::Instant::now();
        queue.shutdown(Duration::from_secs(5)).await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "cancelled, not timed out"
        );
        assert_eq!(queue.slots.available_permits(), 4, "every task ended");
    }
}
