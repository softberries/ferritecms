//! Plugin mail (M2a-2 P17/P18, Review Focus 3): `notify-admins` reaches active admins
//! only (in Bcc), `mail-flood` is cut at one mail per call and 10 per hour, header
//! injection is refused, render filters cannot mail, and a site without `[mail]`
//! answers the guest with an error. Plus the SMTP mailer against an in-test SMTP sink.

use std::sync::Arc;
use std::time::Duration;

use ferrite_config::MailConfig;
use ferrite_core::mail::{MailQueue, Mailer, MemoryMailer, OutgoingMail};
use ferrite_domain::{ContentStatus, Role, Secret, User, UserStatus};
use ferrite_http::{
    PluginReloader, PluginRequest, RenderTrace, StoreHostServices, mail::SmtpMailer,
};
use ferrite_plugin_host::{ActionEvent, HostConfig, PluginHost};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::common::{HOUR, SITE_URL, Site};
use crate::profile_tests;

profile_tests!(
    notify_admins_reaches_admins_only,
    notifier_cuts_long_titles,
    site_title_prefix_is_bounded,
    mail_flood_is_capped,
    mail_header_injection_is_refused,
    mail_refused_on_render_path,
    mail_unconfigured_is_an_error_to_the_guest,
);

const MAIL_SITE_TITLE: &str = "Test Site";

/// A plugin host over services with mail (`[mail]` set): what the queue delivered
/// lands in `mailer`.
struct Mail {
    host: Arc<PluginHost>,
    mailer: Arc<MemoryMailer>,
}

/// Installs example plugin `name` and loads it into a mail-enabled host.
async fn mail_host(site: &Site, name: &str) -> Mail {
    mail_host_titled(site, name, MAIL_SITE_TITLE).await
}

/// [`mail_host`] with `title` as the configured site title.
async fn mail_host_titled(site: &Site, name: &str, title: &str) -> Mail {
    let mailer = Arc::new(MemoryMailer::new());
    let services = Arc::new(
        StoreHostServices::new(site.store.clone(), site.reader.clone(), SITE_URL)
            .with_mailer(Arc::new(MailQueue::new(mailer.clone())), title.to_owned()),
    );
    let host = Arc::new(PluginHost::new(HostConfig::default(), services).unwrap());
    site.install(name).await;
    let report = PluginReloader::new(site.store.clone(), site.media.clone(), host.clone())
        .reload()
        .await
        .unwrap();
    assert_eq!(ferrite_http::LoadSummary::from(&report).loaded, [name]);
    Mail { host, mailer }
}

const WAIT: Duration = Duration::from_secs(5);
/// How long a "no further mail" check waits after the expected mail arrived.
const GRACE: Duration = Duration::from_millis(300);

fn event(subject: &str) -> ActionEvent {
    ActionEvent {
        subject: Some(subject.to_owned()),
        payload: serde_json::json!({}),
    }
}

async fn kv(site: &Site, key: &str) -> String {
    let v = site
        .store
        .read()
        .plugin_kv()
        .get("mail-flood", key)
        .await
        .unwrap()
        .unwrap_or_default();
    String::from_utf8(v).unwrap()
}

async fn set_status(site: &Site, user: &User, status: UserStatus) {
    let mut u = user.clone();
    u.status = status;
    site.store.read().users().update(&u).await.unwrap();
}

async fn notify_admins_reaches_admins_only(site: Site) -> Site {
    site.user("zoe", Role::Admin).await;
    site.user("amy", Role::Admin).await;
    site.user("ed", Role::Editor).await;
    let off = site.user("bob", Role::Admin).await;
    set_status(&site, &off, UserStatus::Disabled).await;
    let author = site.user("ada", Role::Author).await;
    let id = site
        .post(
            &author,
            "hello",
            "Hello",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    let m = mail_host(&site, "notifier").await;

    m.host
        .dispatch_action("notifier", "post.published", event(&id.to_string()))
        .await
        .unwrap();
    let sent = m.mailer.wait_for(1, WAIT).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    let mail = &sent[0];
    // One mail, admins in Bcc (they do not see each other), by login.
    assert!(mail.to.is_empty(), "{mail:?}");
    assert_eq!(mail.bcc, ["amy@example.com", "zoe@example.com"]);
    assert_eq!(
        mail.subject,
        format!("[{MAIL_SITE_TITLE}] Published: Hello")
    );
    assert_eq!(
        mail.text,
        format!("The post \"Hello\" was published.\nPost id: {id}\n\n-- \nSent by plugin notifier")
    );
    assert_eq!(m.mailer.wait_for(2, GRACE).await.len(), 1);
    site
}

/// Review M4: the example cuts a long post title so its subject stays within 200.
async fn notifier_cuts_long_titles(site: Site) -> Site {
    site.user("root", Role::Admin).await;
    let author = site.user("ada", Role::Author).await;
    let title = "Ü".repeat(250);
    let id = site
        .post(
            &author,
            "long",
            &title,
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    let m = mail_host(&site, "notifier").await;
    m.host
        .dispatch_action("notifier", "post.published", event(&id.to_string()))
        .await
        .unwrap();
    let sent = m.mailer.wait_for(1, WAIT).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].subject,
        format!("[{MAIL_SITE_TITLE}] Published: {}…", "Ü".repeat(180))
    );
    site
}

/// Review M3: a header-unsafe site title gives no prefix (instead of breaking every
/// plugin mail); a long one is cut to 100 characters.
async fn site_title_prefix_is_bounded(site: Site) -> Site {
    site.user("root", Role::Admin).await;
    let bad = mail_host_titled(&site, "mail-flood", "Bad\r\nBcc: victim@evil.test").await;
    bad.host
        .dispatch_action("mail-flood", "post.published", event("Hello"))
        .await
        .unwrap();
    assert_eq!(kv(&site, "inject").await, "ok");
    let sent = bad.mailer.wait_for(1, WAIT).await;
    assert_eq!(sent[0].subject, "Hello");

    let long = mail_host_titled(&site, "mail-flood", &"T".repeat(300)).await;
    let max = "x".repeat(200);
    long.host
        .dispatch_action("mail-flood", "post.published", event(&max))
        .await
        .unwrap();
    assert_eq!(kv(&site, "inject").await, "ok");
    let sent = long.mailer.wait_for(1, WAIT).await;
    assert_eq!(sent[0].subject, format!("[{}] {max}", "T".repeat(100)));
    site
}

/// The hour window is fixed (clock floored to the hour): start well inside one so the
/// test's calls share it.
async fn inside_one_hour_window() {
    let now = ferrite_domain::Timestamp::now().as_micros();
    let into = now.rem_euclid(HOUR);
    let left = HOUR - into;
    if left < 60_000_000 {
        let wait = u64::try_from(left).unwrap() + 1_000_000;
        tokio::time::sleep(Duration::from_micros(wait)).await;
    }
}

/// Review Focus 3: across repeated calls exactly 10 mails in the hour; every further
/// call gets an error; inside one call the second send fails (and uses no slot).
async fn mail_flood_is_capped(site: Site) -> Site {
    site.user("root", Role::Admin).await;
    let m = mail_host(&site, "mail-flood").await;
    inside_one_hour_window().await;

    for call in 0..13 {
        m.host
            .dispatch_action("mail-flood", "post.saved", event("x"))
            .await
            .unwrap();
        let (first, second, accepted) = (
            kv(&site, "first").await,
            kv(&site, "second").await,
            kv(&site, "accepted").await,
        );
        if call < 10 {
            assert_eq!(first, "ok", "call {call}");
            assert_eq!(accepted, "1", "call {call}");
        } else {
            assert!(
                first.contains("10 mails per plugin per hour"),
                "call {call}: {first}"
            );
            assert_eq!(accepted, "0", "call {call}");
        }
        assert_eq!(second, "only one mail per call", "call {call}");
    }
    let sent = m.mailer.wait_for(10, WAIT).await;
    assert_eq!(sent.len(), 10);
    assert_eq!(m.mailer.wait_for(11, GRACE).await.len(), 10);
    assert_eq!(sent[0].subject, format!("[{MAIL_SITE_TITLE}] Spam 0"));
    assert_eq!(sent[0].bcc, ["root@example.com"]);
    // Refusals are answers, not plugin failures.
    assert!(m.host.failures().is_empty(), "{:?}", m.host.failures());
    assert!(m.host.is_enabled("mail-flood"));
    site
}

/// CR/LF and other control characters in a plugin's subject, and in an admin address
/// a user could set, are refused, never folded into headers.
async fn mail_header_injection_is_refused(site: Site) -> Site {
    site.user("root", Role::Admin).await;
    // An address written straight to the store (core would refuse it).
    let evil = site.user("evil", Role::Admin).await;
    let mut u = evil.clone();
    u.email = "evil@example.com\r\nBcc: victim@evil.test".into();
    site.store.read().users().update(&u).await.unwrap();
    let m = mail_host(&site, "mail-flood").await;
    inside_one_hour_window().await;

    let long = "x".repeat(201);
    for subject in [
        "Hi\r\nBcc: victim@evil.test",
        "Hi\nX-Evil: 1",
        "Hi\rX",
        "nul\0",
        "esc\u{1b}[31m",
        "c1\u{85}next",
        "ls\u{2028}next",
        "",
        long.as_str(),
    ] {
        m.host
            .dispatch_action("mail-flood", "post.published", event(subject))
            .await
            .unwrap();
        let answer = kv(&site, "inject").await;
        assert!(answer.contains("subject"), "{subject:?}: {answer}");
    }
    // A clean subject still goes out, to the valid admin address only.
    m.host
        .dispatch_action("mail-flood", "post.published", event("Clean"))
        .await
        .unwrap();
    assert_eq!(kv(&site, "inject").await, "ok");
    let sent = m.mailer.wait_for(1, WAIT).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].bcc, ["root@example.com"]);
    assert_eq!(sent[0].subject, format!("[{MAIL_SITE_TITLE}] Clean"));
    assert_eq!(m.mailer.wait_for(2, GRACE).await.len(), 1);
    // The refused attempts used no hourly slot: 9 more go out, then the limit.
    for _ in 0..9 {
        m.host
            .dispatch_action("mail-flood", "post.published", event("More"))
            .await
            .unwrap();
        assert_eq!(kv(&site, "inject").await, "ok");
    }
    m.host
        .dispatch_action("mail-flood", "post.published", event("Over"))
        .await
        .unwrap();
    assert!(kv(&site, "inject").await.contains("per hour"));
    site
}

async fn mail_refused_on_render_path(site: Site) -> Site {
    site.user("root", Role::Admin).await;
    let m = mail_host(&site, "mail-flood").await;
    let mut trace = RenderTrace::default();
    let html = ferrite_http::page_head(
        &m.host,
        site.store.as_ref(),
        crate::SITE,
        ferrite_http::HeadPage {
            item: None,
            path: "/",
        },
        &PluginRequest::public("/", "en", None),
        &mut trace,
    )
    .await;
    assert!(
        html.contains(
            "<meta name=\"mail-flood\" content=\"mail is not allowed during render filters\">"
        ),
        "{html}"
    );
    // Prove the path works, then that the render call sent nothing.
    m.host
        .dispatch_action("mail-flood", "post.published", event("Action"))
        .await
        .unwrap();
    let sent = m.mailer.wait_for(1, WAIT).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, format!("[{MAIL_SITE_TITLE}] Action"));
    assert_eq!(m.mailer.wait_for(2, GRACE).await.len(), 1);
    site
}

async fn mail_unconfigured_is_an_error_to_the_guest(site: Site) -> Site {
    site.user("root", Role::Admin).await;
    // Services without a mailer: a site without `[mail]`.
    let services = Arc::new(StoreHostServices::new(
        site.store.clone(),
        site.reader.clone(),
        SITE_URL,
    ));
    let host = Arc::new(PluginHost::new(HostConfig::default(), services).unwrap());
    site.install("mail-flood").await;
    PluginReloader::new(site.store.clone(), site.media.clone(), host.clone())
        .reload()
        .await
        .unwrap();
    host.dispatch_action("mail-flood", "post.published", event("Hello"))
        .await
        .unwrap();
    assert_eq!(kv(&site, "inject").await, "mail is not configured");
    // No hourly slot was used either.
    let hit = site
        .store
        .read()
        .rate_limits()
        .hit("mail:plugin:mail-flood", Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(hit.count, 1);
    site
}

/// What the in-test SMTP sink received.
#[derive(Debug, Default)]
struct Received {
    mail_from: String,
    rcpt_to: Vec<String>,
    data: String,
}

/// A minimal SMTP server for one session: `EHLO`/`HELO`, `MAIL`, `RCPT`, `DATA`, `QUIT`.
async fn smtp_sink() -> (u16, tokio::task::JoinHandle<Received>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut lines = BufReader::new(read);
        let mut got = Received::default();
        write.write_all(b"220 sink ESMTP\r\n").await.unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if lines.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let cmd = line.trim_end().to_owned();
            let upper = cmd.to_ascii_uppercase();
            let reply: &[u8] = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                b"250 sink\r\n"
            } else if upper.starts_with("MAIL FROM:") {
                got.mail_from = cmd[10..].to_owned();
                b"250 ok\r\n"
            } else if upper.starts_with("RCPT TO:") {
                got.rcpt_to.push(cmd[8..].to_owned());
                b"250 ok\r\n"
            } else if upper == "DATA" {
                write.write_all(b"354 go\r\n").await.unwrap();
                loop {
                    line.clear();
                    lines.read_line(&mut line).await.unwrap();
                    if line == ".\r\n" {
                        break;
                    }
                    got.data.push_str(&line);
                }
                b"250 queued\r\n"
            } else if upper == "QUIT" {
                write.write_all(b"221 bye\r\n").await.unwrap();
                break;
            } else {
                b"502 no\r\n"
            };
            write.write_all(reply).await.unwrap();
        }
        got
    });
    (port, task)
}

fn mail_config(port: u16) -> MailConfig {
    MailConfig {
        smtp_url: Secret::new(format!("smtp://127.0.0.1:{port}")),
        from: "Site <noreply@example.com>".into(),
    }
}

#[tokio::test]
async fn smtp_mailer_sends_to_a_fake_server() {
    let (port, sink) = smtp_sink().await;
    let mailer = SmtpMailer::new(&mail_config(port)).unwrap();
    mailer
        .send(OutgoingMail {
            to: Vec::new(),
            bcc: vec!["amy@example.com".into(), "zoe@example.com".into()],
            subject: "[Test] Hello admins".into(),
            text: "Line one\nLine two".into(),
        })
        .await
        .unwrap();
    let got = tokio::time::timeout(WAIT, sink).await.unwrap().unwrap();
    assert_eq!(got.mail_from, "<noreply@example.com>");
    // The envelope: the admins only, not the `To: <from>` shown in the headers.
    assert_eq!(got.rcpt_to, ["<amy@example.com>", "<zoe@example.com>"]);
    assert!(
        got.data.contains("Subject: [Test] Hello admins\r\n"),
        "{}",
        got.data
    );
    assert!(
        got.data.contains("From: Site <noreply@example.com>\r\n"),
        "{}",
        got.data
    );
    assert!(
        got.data.contains("To: Site <noreply@example.com>\r\n"),
        "{}",
        got.data
    );
    assert!(
        !got.data.contains("Bcc"),
        "Bcc is not disclosed: {}",
        got.data
    );
    assert!(!got.data.contains("zoe@"), "{}", got.data);
    assert!(got.data.contains("Line one"), "{}", got.data);
}

/// Review M2: a non-ASCII subject and `from` name go out as RFC 2047 encoded words; a
/// hostile body (bare LF, lone dots, fake SMTP commands) is CRLF-normalised and
/// dot-stuffed, so it cannot end DATA or start another transaction.
#[tokio::test]
async fn smtp_mailer_encodes_subjects_and_stuffs_hostile_bodies() {
    let (port, sink) = smtp_sink().await;
    let mut config = mail_config(port);
    config.from = "Sïte <noreply@example.com>".into();
    let mailer = SmtpMailer::new(&config).unwrap();
    let body =
        "line one\n.\nMAIL FROM:<evil@evil.test>\r\n.\r\nRCPT TO:<victim@evil.test>\n..two\nend";
    mailer
        .send(OutgoingMail {
            to: Vec::new(),
            bcc: vec!["amy@example.com".into()],
            subject: "[Tést] Héllo ✓".into(),
            text: body.into(),
        })
        .await
        .unwrap();
    let got = tokio::time::timeout(WAIT, sink).await.unwrap().unwrap();
    assert_eq!(got.mail_from, "<noreply@example.com>");
    assert_eq!(got.rcpt_to, ["<amy@example.com>"], "no injected recipient");
    let data = &got.data;
    assert!(data.contains("Subject: =?utf-8?"), "{data}");
    assert!(!data.contains("Héllo"), "encoded, not raw: {data}");
    assert!(data.contains("From: =?utf-8?"), "{data}");
    // Every line ends in CRLF (no bare LF), and body lines starting with "." are
    // stuffed: the sink (which ends DATA at a lone ".") received the whole body.
    assert!(!data.replace("\r\n", "").contains('\n'), "{data:?}");
    assert!(
        data.contains("\r\n..\r\nMAIL FROM:<evil@evil.test>\r\n..\r\n"),
        "{data:?}"
    );
    assert!(data.contains("\r\n...two\r\nend"), "{data:?}");
}

#[tokio::test]
async fn smtp_mailer_refuses_injection_before_connecting() {
    // Nothing listens on the port: a refused mail never gets as far as connecting.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mailer = SmtpMailer::new(&mail_config(port)).unwrap();
    let bad = |subject: &str, to: &str| OutgoingMail {
        to: vec![to.to_owned()],
        bcc: Vec::new(),
        subject: subject.to_owned(),
        text: String::new(),
    };
    for mail in [
        bad("Hi\r\nBcc: victim@evil.test", "a@example.com"),
        bad("Hi", "a@example.com\r\nBcc: victim@evil.test"),
        bad("Hi", "Name <a@example.com>"),
    ] {
        let e = mailer.send(mail).await.unwrap_err();
        assert!(!e.0.contains("connect"), "{e}");
    }
    // A valid mail does try to connect, and the error never names the URL.
    let e = mailer.send(bad("Hi", "a@example.com")).await.unwrap_err();
    assert!(!e.0.contains(&port.to_string()), "{e}");
}

#[test]
fn smtp_mailer_rejects_bad_config_without_echoing_it() {
    // R16 holds for a `MailConfig` built by hand, too.
    let cfg = |url: &str, from: &str| MailConfig {
        smtp_url: Secret::new(url),
        from: from.into(),
    };
    for (url, from) in [
        ("ftp://u:topsecretpw@h", "a@example.com"),
        ("smtp://u:topsecretpw@h?tls=sometimes", "a@example.com"),
        ("smtp://u:topsecretpw@127.0.0.1", "not a mailbox"),
        ("smtp://u:topsecretpw@mail.example.com", "a@example.com"),
        ("smtp://u:topsecretpw@h?tls=opportunistic", "a@example.com"),
    ] {
        let e = SmtpMailer::new(&cfg(url, from)).err().expect(url);
        assert!(!e.0.contains("topsecretpw"), "{e}");
    }
}
