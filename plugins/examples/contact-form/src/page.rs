//! The plugin's HTML pages. They are served with the route CSP (`sandbox allow-forms;
//! form-action 'self'`, no script), and every value that came from a visitor, a setting or
//! the store goes through [`escape`] anyway.

use std::fmt::Write;

use ferrite_sdk::html::escape;

use crate::form::Submission;
use crate::inbox::Message;

/// Where the form posts.
pub const SUBMIT: &str = "/_p/contact-form/submit";
/// The thank-you text when the `success_message` setting is empty.
pub const DEFAULT_THANKS: &str = "Your message was sent. Thank you for getting in touch.";

const STYLE: &str = "\
:root{color-scheme:light dark;--bg:#fff;--fg:#1c2128;--muted:#59636e;--line:#d1d9e0;--soft:#f6f8fa;--accent:#2f6fdb;--bad:#c62828}\
@media(prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#e6edf3;--muted:#9198a1;--line:#3d444d;--soft:#161b22;--accent:#5b9bf5;--bad:#ff7b72}}\
*{box-sizing:border-box}\
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.5 system-ui,-apple-system,Segoe UI,sans-serif}\
main{max-width:40rem;margin:0 auto;padding:1.5rem 1rem 3rem}\
main.wide{max-width:56rem}\
h1{font-size:1.5rem;margin:0 0 1rem}h2{font-size:1.05rem;margin:0}\
a{color:var(--accent)}\
label{display:block;font-weight:600;margin:1rem 0 .3rem}\
.opt{font-weight:400;color:var(--muted)}\
input,textarea{width:100%;font:inherit;color:inherit;background:var(--bg);border:1px solid var(--line);border-radius:.4rem;padding:.5rem .6rem}\
textarea{min-height:10rem;resize:vertical}\
input:focus,textarea:focus{outline:2px solid var(--accent);outline-offset:1px}\
button{margin-top:1.25rem;font:inherit;font-weight:600;color:#fff;background:var(--accent);border:0;border-radius:.4rem;padding:.6rem 1.4rem;cursor:pointer}\
.hp{position:absolute;left:-10000px;width:1px;height:1px;overflow:hidden}\
.errors{border:1px solid var(--bad);border-radius:.5rem;padding:.75rem 1rem;margin:0 0 1rem}\
.errors h2{color:var(--bad)}.errors ul{margin:.4rem 0 0;padding-left:1.2rem}\
.note{white-space:pre-line}\
.muted{color:var(--muted)}\
.back{display:inline-block;margin-top:1.5rem}\
article{border:1px solid var(--line);border-radius:.6rem;padding:.9rem 1rem;margin:1rem 0;background:var(--soft)}\
article header{display:flex;flex-wrap:wrap;gap:.25rem .75rem;align-items:baseline}\
article h2{margin:.4rem 0 .2rem}\
.meta{color:var(--muted);font-size:.875rem;margin:0;overflow-wrap:anywhere}\
.text{white-space:pre-wrap;overflow-wrap:anywhere;margin:.6rem 0 0}\
nav{display:flex;gap:1rem;margin-top:1rem}\
code{font:.9em ui-monospace,SFMono-Regular,Menlo,monospace}";

/// A document with `body` inside `<main>`.
fn document(title: &str, class: &str, data_page: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style>{STYLE}</style></head>\
         <body><main class=\"{class}\" data-page=\"{data_page}\">{body}</main></body></html>",
        escape(title)
    )
}

/// The link back to the site. `href` is a checked path or `/`; escaped all the same.
fn back(out: &mut String, href: &str) {
    let _ = write!(
        out,
        "<p><a class=\"back\" href=\"{}\">Back to the site</a></p>",
        escape(href)
    );
}

/// "Contact" and the site title.
fn heading(site_title: &str) -> String {
    let title = site_title.trim();
    if title.is_empty() {
        "Contact".to_owned()
    } else {
        format!("Contact {title}")
    }
}

/// The form: empty (`GET /`), or filled in again above the `problems` (the 422 page).
pub fn form(site_title: &str, filled: Option<&Submission>, problems: &[&str]) -> String {
    let empty = Submission::default();
    let s = filled.unwrap_or(&empty);
    let title = heading(site_title);
    let mut out = String::new();
    let _ = write!(out, "<h1>{}</h1>", escape(&title));
    if !problems.is_empty() {
        out.push_str(
            "<div class=\"errors\" role=\"alert\"><h2>Please correct the following</h2><ul>",
        );
        for p in problems {
            let _ = write!(out, "<li>{}</li>", escape(p));
        }
        out.push_str("</ul></div>");
    }
    let _ = write!(out, "<form method=\"post\" action=\"{SUBMIT}\">");
    if let Some(ret) = &s.back {
        let _ = write!(
            out,
            "<input type=\"hidden\" name=\"return\" value=\"{}\">",
            escape(ret)
        );
    }
    let _ = write!(
        out,
        "<label for=\"cf-name\">Name</label>\
         <input id=\"cf-name\" name=\"name\" autocomplete=\"name\" required value=\"{}\">\
         <label for=\"cf-email\">E-mail</label>\
         <input id=\"cf-email\" name=\"email\" type=\"email\" autocomplete=\"email\" required value=\"{}\">\
         <label for=\"cf-subject\">Subject <span class=\"opt\">(optional)</span></label>\
         <input id=\"cf-subject\" name=\"subject\" value=\"{}\">\
         <label for=\"cf-message\">Message</label>\
         <textarea id=\"cf-message\" name=\"message\" required>{}</textarea>\
         <div class=\"hp\" aria-hidden=\"true\"><label for=\"cf-website\">Leave this field empty</label>\
         <input id=\"cf-website\" name=\"website\" tabindex=\"-1\" autocomplete=\"off\"></div>\
         <button type=\"submit\">Send</button></form>",
        escape(&s.name),
        escape(&s.email),
        escape(&s.subject),
        escape(&s.message),
    );
    back(&mut out, s.back.as_deref().unwrap_or("/"));
    document(&title, "", "form", &out)
}

/// The page after a successful post (and after a filled honeypot).
pub fn thanks(text: &str, back_to: &str) -> String {
    let mut out = String::from("<h1>Thank you</h1>");
    let _ = write!(out, "<p class=\"note\">{}</p>", escape(text));
    back(&mut out, back_to);
    document("Thank you", "", "thanks", &out)
}

/// A request the form cannot take (400, 413, 415, 503).
pub fn problem(title: &str, text: &str) -> String {
    let mut out = String::new();
    let _ = write!(out, "<h1>{}</h1><p>{}</p>", escape(title), escape(text));
    back(&mut out, "/");
    document(title, "", "problem", &out)
}

/// The inbox page for a visitor who is not signed in.
pub fn sign_in_required() -> String {
    document(
        "Contact inbox",
        "",
        "denied",
        "<h1>Contact inbox</h1><p>Sign in to the admin first, then open this page again.</p>\
         <p><a href=\"/admin/\">Go to the admin</a></p>",
    )
}

/// The inbox page for a signed-in user who is not in the `viewers` setting.
pub fn not_a_viewer(user: &str) -> String {
    document(
        "Contact inbox",
        "",
        "denied",
        &format!(
            "<h1>Contact inbox</h1><p>You are signed in, but this page is only for the users \
             listed under Viewers in the contact-form plugin's settings.</p>\
             <p>Your user id is <code>{}</code>. To allow yourself, open the admin, go to \
             Plugins, then contact-form, and add this id to Viewers (several ids are separated \
             by commas or new lines).</p>",
            escape(user)
        ),
    )
}

/// The inbox page with a notice instead of messages (empty, unreadable, newer).
pub fn inbox_notice(text: &str) -> String {
    document(
        "Contact inbox",
        "wide",
        "inbox",
        &format!("<h1>Contact inbox</h1><p>{}</p>", escape(text)),
    )
}

/// One page of the inbox: `shown` are its messages with their 1-based position in the
/// inbox (newest first; `None` for a stored line that is not a message), out of `total`;
/// `page` is 1-based and within `1..=pages`.
pub fn inbox(
    shown: &[(usize, Option<Message>)],
    total: usize,
    page: usize,
    pages: usize,
) -> String {
    let mut out = String::from("<h1>Contact inbox</h1>");
    let _ = write!(
        out,
        "<p class=\"muted\">{total} message{}, newest first. Page {page} of {pages}. Times are \
         UTC.</p>",
        if total == 1 { "" } else { "s" }
    );
    for (i, m) in shown {
        let Some(m) = m else {
            let _ = write!(
                out,
                "<article data-message=\"{i}\"><p class=\"meta\">This stored message could \
                 not be read.</p></article>"
            );
            continue;
        };
        let subject = if m.subject.is_empty() {
            "(no subject)"
        } else {
            m.subject.as_str()
        };
        let _ = write!(
            out,
            "<article data-message=\"{}\"><header><strong>{}</strong>\
             <span>&lt;{}&gt;</span><time class=\"meta\">{}</time></header>\
             <h2>{}</h2>",
            i,
            escape(&m.name),
            escape(&m.email),
            escape(&m.at),
            escape(subject),
        );
        if !m.page.is_empty() {
            let _ = write!(
                out,
                "<p class=\"meta\">Sent from <code>{}</code></p>",
                escape(&m.page)
            );
        }
        let _ = write!(
            out,
            "<p class=\"text\">{}</p></article>",
            escape(&m.message)
        );
    }
    if pages > 1 {
        out.push_str("<nav aria-label=\"Pages\">");
        if page > 1 {
            let _ = write!(out, "<a href=\"?page={}\">Newer</a>", page - 1);
        }
        if page < pages {
            let _ = write!(out, "<a href=\"?page={}\">Older</a>", page + 1);
        }
        out.push_str("</nav>");
    }
    document("Contact inbox", "wide", "inbox", &out)
}
