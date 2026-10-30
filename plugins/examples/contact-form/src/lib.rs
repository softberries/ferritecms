//! Contact form example plugin (plan P45): a form page at `/_p/contact-form/`, its target
//! `POST /_p/contact-form/submit` (a theme posts its own form there too), and an inbox page
//! at `/_p/contact-form/inbox` for the users listed in the `viewers` setting (P46).
//!
//! A valid post is mailed to the site's active admins (`mail.notify-admins`, Bcc; the host
//! adds the site title to the subject and a footer) and, unless `store_messages` is off,
//! kept in one KV blob, `inbox`, the latest [`inbox::KEEP`] messages newest first (see
//! [`inbox`]). A mail that cannot be sent is logged: the message is still stored and the
//! visitor still thanked. Only when it is neither stored nor mailed is the visitor told so
//! (503).
//!
//! **Abuse.** The host limits posts to 10 an hour per client (the manifest) and a body to
//! 1 MiB; the plugin reads at most [`form::MAX_BODY_BYTES`]. A filled honeypot field
//! (`website`) gets the thank-you page and nothing else. CR and LF are stripped from every
//! field but the message, and the mail subject is made one line again before it is sent,
//! so nothing a visitor types reaches a mail header. Every value shown on a page is
//! HTML-escaped. `return` is used only when it is a path on this site.
//!
//! **Fuel.** A call is bounded in guest instructions, and decoding and encoding JSON is the
//! cost. So the inbox is stored as JSON lines: a submit adds its line without decoding the
//! stored ones, and the inbox page decodes only the [`PER_PAGE`] messages it shows (see
//! [`inbox`]). At the default limit of 50 million the dearest submit (a full blob plus a
//! message of the largest size) needs about 9 million and the dearest inbox page about 19
//! million.

mod clock;
mod form;
mod inbox;
mod page;

use ferrite_sdk::{
    Ctx, FormError, Kv, Level, Result, RouteRequest, RouteResponse, mail, options,
    parse_urlencoded, route, viewers_allow,
};

use form::Submission;
use inbox::{Message, Stored};

/// The default of the `subject_prefix` setting (the manifest declares it too).
const DEFAULT_PREFIX: &str = "[Contact]";
/// Characters of the message that stand in for an empty subject.
const SUBJECT_FROM_MESSAGE: usize = 60;
/// The host's limit on a plugin's mail subject, in characters (before it adds the site
/// title).
const MAX_SUBJECT_CHARS: usize = 200;
/// The subject when everything else is empty.
const FALLBACK_SUBJECT: &str = "Contact form message";
/// Longest `Referer` kept as the message's page, in characters.
const MAX_REFERER_CHARS: usize = 512;
/// Messages per inbox page. Ten messages of the dearest kind (every character a `"`, which
/// is `\"` to decode and `&quot;` to show) cost about 19 million fuel and about 360 KB of
/// HTML, inside the default 50 million and the host's 1 MiB response.
const PER_PAGE: usize = 10;

fn warn(msg: &str) {
    ferrite_sdk::log(Level::Warn, &format!("contact-form: {msg}"));
}

fn html(status: u16, body: String) -> RouteResponse {
    route::html(status, &body)
}

// ---- settings -------------------------------------------------------------------------

/// A text setting; `None` when unset or not a string.
fn setting_string(ctx: &Ctx, key: &str) -> Option<String> {
    serde_json::from_str::<String>(&ctx.setting(key)?).ok()
}

/// `store_messages`: true unless set to `false`.
fn store_messages(ctx: &Ctx) -> bool {
    ctx.setting("store_messages")
        .and_then(|json| serde_json::from_str::<bool>(&json).ok())
        .unwrap_or(true)
}

fn site_title() -> String {
    options::get("site.title").unwrap_or_default()
}

// ---- the form -------------------------------------------------------------------------

fn form_page(_req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    Ok(html(200, page::form(&site_title(), None, &[])))
}

fn header<'a>(req: &'a RouteRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn submit(req: RouteRequest, ctx: &Ctx) -> Result<RouteResponse> {
    if !header(&req, "content-type").is_some_and(form::is_urlencoded) {
        return Ok(html(
            415,
            page::problem(
                "Unsupported form",
                "This form must be sent as application/x-www-form-urlencoded.",
            ),
        ));
    }
    if req.body.len() > form::MAX_BODY_BYTES {
        return Ok(html(
            413,
            page::problem("Message too large", "The form sent is too large."),
        ));
    }
    let pairs = match parse_urlencoded(&req.body) {
        Ok(pairs) => pairs,
        Err(e) => {
            let text = match e {
                FormError::TooManyPairs => "The form sent has too many fields.",
                FormError::BadEscape | FormError::InvalidUtf8 => "The form sent could not be read.",
            };
            return Ok(html(400, page::problem("Bad request", text)));
        }
    };
    let sub = Submission::from_pairs(&pairs);
    let back = sub.back.clone().unwrap_or_else(|| "/".to_owned());
    let thanks = || {
        let text = setting_string(ctx, "success_message")
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| page::DEFAULT_THANKS.to_owned());
        html(200, page::thanks(&text, &back))
    };

    // A bot that filled the hidden field is thanked and ignored, whatever else it sent.
    if sub.honeypot {
        ferrite_sdk::log(Level::Info, "contact-form: honeypot filled, post dropped");
        return Ok(thanks());
    }
    let problems = sub.problems();
    if !problems.is_empty() {
        return Ok(html(422, page::form(&site_title(), Some(&sub), &problems)));
    }

    let message = Message {
        at: clock::now(),
        page: sub.back.clone().unwrap_or_else(|| referer(&req)),
        name: sub.name,
        email: sub.email,
        subject: sub.subject,
        message: sub.message,
    };
    let stored = store_messages(ctx) && store(&ctx.kv(), &message);
    let mailed = send_mail(ctx, &message);
    if !stored && !mailed {
        return Ok(html(
            503,
            page::problem(
                "Message not sent",
                "Your message could not be sent right now. Please try again later.",
            ),
        ));
    }
    Ok(thanks())
}

/// The `Referer` as text on one line, cut to [`MAX_REFERER_CHARS`]; empty without one.
fn referer(req: &RouteRequest) -> String {
    header(req, "referer")
        .unwrap_or_default()
        .chars()
        .filter(|&c| !form::is_break_like(c))
        .take(MAX_REFERER_CHARS)
        .collect()
}

/// True when the `inbox` key exists (a listing, which does not read the value).
fn inbox_exists(kv: &Kv) -> bool {
    kv.list_keys(inbox::KEY, 1).iter().any(|k| k == inbox::KEY)
}

/// Add `message` to the stored inbox as its newest line. False (and logged) when it was
/// not stored.
fn store(kv: &Kv, message: &Message) -> bool {
    let line = match message.line() {
        Ok(line) => line,
        Err(e) => {
            warn(&format!("the message was not stored: {e}"));
            return false;
        }
    };
    let stored = kv.get(inbox::KEY);
    let older = match stored.as_deref().map(Stored::open) {
        None => {
            // The host answers `None` for a missing key and for a read that failed;
            // writing a fresh blob over one that exists would lose every message in it.
            if inbox_exists(kv) {
                warn("the inbox exists but could not be read; the message was not stored");
                return false;
            }
            Vec::new()
        }
        Some(Stored::Lines(lines)) => lines,
        Some(Stored::Newer) => {
            warn("the inbox was written by a newer version; the message was not stored");
            return false;
        }
        Some(Stored::Corrupt) => {
            warn("the stored inbox is not a blob of this plugin; starting it again");
            Vec::new()
        }
    };
    match kv.set(inbox::KEY, &inbox::blob(&line, &older)) {
        Ok(()) => true,
        Err(e) => {
            warn(&format!("the message was not stored: {e}"));
            false
        }
    }
}

/// Mail `message` to the admins. False (and logged) when the host refused it: mail not
/// configured, the hourly limit, a full queue.
fn send_mail(ctx: &Ctx, message: &Message) -> bool {
    let prefix = setting_string(ctx, "subject_prefix").unwrap_or_else(|| DEFAULT_PREFIX.into());
    let subject = mail_subject(&prefix, &message.subject, &message.message);
    let body = format!(
        "A message was sent with the contact form.\n\n\
         Name: {}\nE-mail: {}\nSubject: {}\nPage: {}\nTime: {}\n\n{}",
        message.name,
        message.email,
        if message.subject.is_empty() {
            "(none)"
        } else {
            &message.subject
        },
        if message.page.is_empty() {
            "(unknown)"
        } else {
            &message.page
        },
        message.at,
        message.message,
    );
    match mail::notify_admins(&subject, &body) {
        Ok(()) => true,
        Err(e) => {
            warn(&format!("mail not sent: {e}"));
            false
        }
    }
}

/// `<prefix> <subject>`, or the first 60 characters of the message for an empty subject,
/// on one line (format characters dropped, control characters and line separators become
/// spaces, runs of whitespace one space) and cut to the host's 200 characters.
fn mail_subject(prefix: &str, subject: &str, message: &str) -> String {
    let topic = if subject.is_empty() {
        one_line(message)
            .chars()
            .take(SUBJECT_FROM_MESSAGE)
            .collect()
    } else {
        subject.to_owned()
    };
    let line = one_line(&format!("{prefix} {topic}"));
    let cut: String = line.chars().take(MAX_SUBJECT_CHARS).collect();
    let cut = cut.trim_end();
    if cut.is_empty() {
        FALLBACK_SUBJECT.to_owned()
    } else {
        cut.to_owned()
    }
}

/// `s` on one line: stripped format characters ([`form::is_stripped`]) dropped, control characters and line separators made
/// spaces, runs of whitespace one space.
fn one_line(s: &str) -> String {
    let spaced: String = s
        .chars()
        .filter(|&c| !form::is_stripped(c))
        .map(|c| if form::is_break_like(c) { ' ' } else { c })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- the inbox ------------------------------------------------------------------------

/// `?page=`: the first value as a page number, 1 when it is not a positive number.
fn page_number(req: &RouteRequest) -> usize {
    req.query
        .iter()
        .find(|(k, _)| k == "page")
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .map_or(1, |n| n.max(1))
}

fn inbox_page(req: RouteRequest, ctx: &Ctx) -> Result<RouteResponse> {
    let Some(user) = req.user.as_deref() else {
        return Ok(html(403, page::sign_in_required()));
    };
    let viewers = setting_string(ctx, "viewers");
    if !viewers_allow(viewers.as_deref(), Some(user)) {
        return Ok(html(403, page::not_a_viewer(user)));
    }
    let kv = ctx.kv();
    let Some(bytes) = kv.get(inbox::KEY) else {
        let text = if inbox_exists(&kv) {
            "The inbox could not be read right now. Try again in a moment."
        } else {
            "No messages yet."
        };
        return Ok(html(200, page::inbox_notice(text)));
    };
    let lines = match Stored::open(&bytes) {
        Stored::Lines(lines) => lines,
        Stored::Newer => {
            return Ok(html(
                200,
                page::inbox_notice(
                    "The inbox was written by a newer version of this plugin and is not \
                     shown. Update the plugin to read it.",
                ),
            ));
        }
        Stored::Corrupt => {
            warn("the stored inbox is not a blob of this plugin");
            return Ok(html(
                200,
                page::inbox_notice(
                    "The stored inbox could not be read. The next message starts a new one.",
                ),
            ));
        }
    };
    if lines.is_empty() {
        return Ok(html(200, page::inbox_notice("No messages yet.")));
    }
    // Only the page's messages are decoded. A line that is not a message is shown as such.
    let pages = lines.len().div_ceil(PER_PAGE);
    let page_no = page_number(&req).min(pages);
    let start = (page_no - 1) * PER_PAGE;
    let shown: Vec<(usize, Option<Message>)> = lines
        .iter()
        .enumerate()
        .skip(start)
        .take(PER_PAGE)
        .map(|(i, line)| (i + 1, Message::decode(line)))
        .collect();
    Ok(html(200, page::inbox(&shown, lines.len(), page_no, pages)))
}

ferrite_sdk::export_plugin! {
    filters: {},
    routes: { "/" => form_page, "/submit" => submit, "/inbox" => inbox_page },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_are_one_line_and_bounded() {
        assert_eq!(mail_subject("[Contact]", "Hi", "x"), "[Contact] Hi");
        assert_eq!(
            mail_subject("[A\r\nB]", "", "one\ntwo\r\n\tthree"),
            "[A B] one two three"
        );
        assert_eq!(mail_subject("", "Hi", "x"), "Hi");
        assert_eq!(mail_subject("", "", " \n "), FALLBACK_SUBJECT);
        let long = mail_subject(&"p".repeat(300), "Hi", "x");
        assert_eq!(long, "p".repeat(200));
        let msg = "m".repeat(100);
        assert_eq!(mail_subject("P", "", &msg), format!("P {}", "m".repeat(60)));
        let s = mail_subject("ł", &"ż".repeat(250), "x");
        assert_eq!(s.chars().count(), 200);
        // Format characters from the prefix or the message do not reach the subject.
        assert_eq!(
            mail_subject("[\u{202E}C]", "", "Pay \u{200B}now\u{2066}!"),
            "[C] Pay now!"
        );
    }
}
