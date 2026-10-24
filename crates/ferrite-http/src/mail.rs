//! The SMTP [`Mailer`] (M2a-2 P17) over lettre's async transport: `smtp://` (plain, or
//! STARTTLS with `?tls=required`, which never falls back to plaintext), `smtps://`
//! (implicit TLS, rustls + webpki roots), credentials in the URL only over TLS or to a
//! loopback relay (R16), a 10 s timeout per SMTP command.
//!
//! Errors never contain the URL, the password, the body or the addresses. Every mail
//! passes [`OutgoingMail::check`] first, so header-unsafe text is refused before a
//! connection is opened. Recipients in `bcc` get the mail without a `Bcc` header; a mail
//! with only `bcc` recipients shows the configured `from` as `To`, and the envelope is
//! exactly `to` + `bcc` (the `from` address gets no copy).

use std::time::Duration;

use ferrite_config::MailConfig;
use ferrite_core::mail::{MailError, Mailer, OutgoingMail};
use lettre::address::{Address, Envelope};
use lettre::message::{Mailbox, header::ContentType};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

/// lettre's timeout for each SMTP step (connect, command, reply).
pub const SMTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Sends through one SMTP server. Not `Debug`: the transport holds the credentials.
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    /// Parses `mail.smtp_url` and `mail.from` with lettre (at `serve` boot, F30).
    pub fn new(config: &MailConfig) -> Result<Self, MailError> {
        // The config rule again (R16: credentials only over TLS or to loopback), for a
        // `MailConfig` not built by `ferrite_config::load`.
        ferrite_config::check_smtp_url(config.smtp_url.expose())
            .map_err(|why| MailError(format!("mail.smtp_url {why}")))?;
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(config.smtp_url.expose())
            .map_err(|_| {
                MailError(
                    "mail.smtp_url is not a usable SMTP URL (smtps://host[:port] or \
                     smtp://host[:port][?tls=required])"
                        .to_owned(),
                )
            })?
            .timeout(Some(SMTP_TIMEOUT))
            .build();
        let from = config
            .from
            .parse::<Mailbox>()
            .map_err(|_| MailError("mail.from is not a valid mailbox".to_owned()))?;
        Ok(Self { transport, from })
    }
}

fn address(addr: &str) -> Result<Address, MailError> {
    addr.parse::<Address>()
        .map_err(|_| MailError("invalid e-mail address".to_owned()))
}

/// A short description of a failed send, without server-supplied text.
fn describe(e: &lettre::transport::smtp::Error) -> String {
    let code = e.status().map(|c| format!(" ({c})")).unwrap_or_default();
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_permanent() {
        "refused by the server"
    } else if e.is_transient() {
        "temporarily refused by the server"
    } else {
        "connection or protocol error"
    };
    format!("SMTP send failed: {kind}{code}")
}

#[async_trait::async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError> {
        mail.check()?;
        let to = mail
            .to
            .iter()
            .map(|a| address(a))
            .collect::<Result<Vec<_>, _>>()?;
        let bcc = mail
            .bcc
            .iter()
            .map(|a| address(a))
            .collect::<Result<Vec<_>, _>>()?;
        let envelope = Envelope::new(
            Some(self.from.email.clone()),
            to.iter().chain(&bcc).cloned().collect(),
        )
        .map_err(|_| MailError("invalid envelope".to_owned()))?;
        let mut builder = Message::builder().from(self.from.clone());
        if to.is_empty() {
            builder = builder.to(self.from.clone());
        }
        for a in to {
            builder = builder.to(Mailbox::new(None, a));
        }
        let message = builder
            .envelope(envelope)
            .subject(mail.subject)
            .header(ContentType::TEXT_PLAIN)
            .body(mail.text)
            .map_err(|_| MailError("the mail could not be built".to_owned()))?;
        self.transport
            .send(message)
            .await
            .map(drop)
            .map_err(|e| MailError(describe(&e)))
    }
}
