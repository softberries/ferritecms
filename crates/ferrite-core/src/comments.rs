//! Comments (M2a-2b P11–P14, R2; F18–F22): submitting, moderating, deleting and the
//! public thread of a post.
//!
//! **What is stored.** Plain text only, never HTML: the body, name, e-mail and URL are
//! trimmed and checked here and stored as typed; [`render_body`] escapes the body on
//! output (paragraphs and line breaks, no links). Limits: body 1..=[`MAX_COMMENT_BODY`]
//! characters (`\r\n` and `\r` become `\n`; no control characters but `\n` and `\t`),
//! name 1..=[`MAX_COMMENT_NAME`] characters without control or bidi characters
//! ([`is_bidi_control`], R13), e-mail at most [`MAX_COMMENT_EMAIL`] bytes
//! (required for guests), URL empty or at most [`MAX_COMMENT_URL`] characters of
//! `http`/`https` with a host and no user info. Only `post` items take comments, while
//! publicly visible and while `comments.enabled` holds (else `NotFound`, as for an
//! unknown item). Replies go to an approved comment of the same item, at most
//! [`MAX_COMMENT_DEPTH`] deep.
//!
//! **Spam (P13).** Per client address (IPv6 per /64) 3 a minute and 20 an hour, counted
//! in the database before anything else (R6), so the limits hold across nodes. A
//! signed-in commenter whose role holds `moderate_comments` (a session, or a token with
//! `comments:write`) is not counted (M2b-2 Task 14: replies from the moderation queue);
//! guests and other roles are. A filled
//! honeypot on input that passes every check is [`Submitted::Discarded`]: nothing is
//! written, and the caller answers exactly as for a pending comment (F18); invalid input
//! is refused whether or not the honeypot is filled. A discard is faster than a real
//! submit (no `known` lookups, no insert): timing can tell them apart, which is accepted
//! (the field is no secret; it only traps bots that fill every field). The stored address is [`AuthService::comment_ip_hash`],
//! keyed and not reversible.
//!
//! **Status (P11, R2, F19).** A signed-in commenter whose role holds
//! `moderate_comments` is approved. With `comments.moderation = "known"` (the default) a
//! comment with at most [`MAX_KNOWN_LINKS`] links is approved when its author has an
//! approved comment: a guest by the same e-mail and (trimmed) name on an approved guest
//! comment, never when the e-mail belongs to an account, and never through comments an
//! account wrote before it was deleted (the store keeps them apart); a signed-in
//! commenter by an approved comment of their own account. Everything else is pending.
//!
//! **Events.** An approved insert, and every moderation and delete, writes
//! `invalidate([post:<item>])` in the same `Tx`, so the item's public page is rendered
//! again. Moderation and delete always do, not only when approved-ness changes: the
//! status before the `UPDATE` is not read under a lock, and a stale page would show a
//! comment that is no longer approved (or delete a trashed parent's approved replies).
//! Moderation is not audited (P14).

use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    sync::Arc,
    time::Duration,
};

use ferrite_domain::{
    Comment, CommentId, CommentQuery, CommentStatus, ContentId, ContentStatus, Page, Timestamp,
};
use ferrite_store::{Repos, Store, StoreError};
use html_escape::encode_text;
use serde_json::Value;

use crate::changes::{Capture, ObjectRef, Op};
use crate::{
    Actor, AuthService, CoreError,
    actor::{Area, Capability, Scope, can, require, require_read, require_reader, require_scope},
    events::{self, post_key},
    is_bidi_control,
    permalink::Permalinks,
    site::{DepKeys, has_public_page, is_public, public_paths},
    tx::finish,
};

/// `comments.enabled` when unset (R2: on for posts).
pub const COMMENTS_ENABLED_DEFAULT: bool = true;
/// `comments.moderation` when unset (R2, F20).
pub const COMMENTS_MODERATION_DEFAULT: &str = "known";
/// The option holding whether comments are open.
pub const ENABLED_OPTION: &str = "comments.enabled";
/// The option holding the moderation mode.
pub const MODERATION_OPTION: &str = "comments.moderation";
/// The values of [`MODERATION_OPTION`].
pub const MODERATION_CHOICES: [&str; 2] = ["all", "known"];
/// The option holding how many top-level threads a public comment page shows (P34).
pub const PER_PAGE_OPTION: &str = "comments.per_page";
/// `comments.per_page` when unset (P34).
pub const COMMENTS_PER_PAGE_DEFAULT: u32 = 50;
/// The largest `comments.per_page`.
pub const MAX_COMMENTS_PER_PAGE: u32 = 100;
/// The query parameter of a public comment page (`?cpage=N`, P34).
pub const COMMENT_PAGE_PARAM: &str = "cpage";

/// Longest body, in characters.
pub const MAX_COMMENT_BODY: usize = 10_000;
/// Longest author name, in characters.
pub const MAX_COMMENT_NAME: usize = 100;
/// Longest e-mail, in bytes.
pub const MAX_COMMENT_EMAIL: usize = 254;
/// Longest author URL, in characters.
pub const MAX_COMMENT_URL: usize = 200;
/// Deepest reply (a top-level comment is depth 0).
pub const MAX_COMMENT_DEPTH: u8 = 5;
/// More links than this hold a comment for moderation under `known`.
pub const MAX_KNOWN_LINKS: usize = 2;
/// The only content type that takes comments (P11; others later, P29).
pub const COMMENT_KIND: &str = "post";
/// Where the theme form posts (`POST /_comment`, P12).
pub const COMMENT_FORM_PATH: &str = "/_comment";
/// The theme form's honeypot field: a bot that fills it is discarded (P13).
pub const HONEYPOT_FIELD: &str = "website";

/// Rate-limit bucket of the per-minute limit.
pub const COMMENT_MINUTE_SCOPE: &str = "comment";
/// Rate-limit bucket of the per-hour limit.
pub const COMMENT_HOUR_SCOPE: &str = "comment-hour";
/// Comments a client address may submit per minute.
pub const COMMENTS_PER_MINUTE: u32 = 3;
/// Comments a client address may submit per hour.
pub const COMMENTS_PER_HOUR: u32 = 20;
const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);

/// A comment as submitted (API or theme form).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentInput {
    pub item: ContentId,
    /// The content type the request named; must be the item's.
    pub kind: String,
    pub parent: Option<CommentId>,
    /// Ignored for a signed-in commenter (the account's display name).
    pub author_name: String,
    /// Ignored for a signed-in commenter (the account's e-mail).
    pub author_email: String,
    /// `""`: none.
    pub author_url: String,
    pub body: String,
    /// The honeypot field: anything but `""` discards the comment.
    pub honeypot: String,
}

/// Who submits a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commenter {
    Anonymous,
    /// A signed-in user (its credential needs `comments:write`).
    User(Actor),
}

/// What a submit did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submitted {
    /// Stored, pending or approved.
    Accepted(Comment),
    /// The honeypot was filled on otherwise valid input: nothing was stored. The comment
    /// is what a pending one would have been (status `pending`, a fresh id that is
    /// never stored), so the caller answers exactly as for a pending comment (F18).
    Discarded(Comment),
}

/// The item a comment is on, as the moderation queue shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentItem {
    pub id: ContentId,
    /// The content type.
    pub kind: String,
    pub title: String,
    /// The item's public path, only while it has a public page now (published, its
    /// publish time reached, not media); `None` otherwise.
    pub permalink: Option<String>,
    pub status: ContentStatus,
}

/// A comment with its item ([`CommentService::list`], [`CommentService::moderate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedComment {
    pub comment: Comment,
    pub item: CommentItem,
}

/// How many comments have each status ([`CommentService::counts`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommentCounts {
    pub pending: u64,
    pub approved: u64,
    pub spam: u64,
    pub trash: u64,
}

/// An approved comment as the public site shows it: no e-mail, no address hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentPublic {
    pub id: CommentId,
    /// The comment replied to, `None` at the top level and for a reply whose parent is
    /// not approved (a root of its own thread, P34), which is shown at depth 0.
    pub parent_id: Option<CommentId>,
    /// 0 at the top level.
    pub depth: u8,
    /// Plain text (escape it).
    pub author_name: String,
    /// `None` when the commenter gave none.
    pub author_url: Option<String>,
    /// The body rendered by [`render_body`] (safe HTML).
    pub html: String,
    pub date: Timestamp,
    /// Whether a reply to it would be accepted: the stored chain (any status, as
    /// [`CommentService::submit`] walks it) leaves room below [`MAX_COMMENT_DEPTH`].
    /// For a reply shown at depth 0 because its parent is hidden this is not `depth`.
    pub can_reply: bool,
}

/// One page of a post's public thread
/// ([`SiteReader::comments`](crate::SiteReader::comments), P34).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentThread {
    /// `comments.enabled`: whether the form is shown. A closed thread still lists its
    /// approved comments (as WordPress does).
    pub open: bool,
    /// The page's approved comments in thread order: all `per_page` top-level threads
    /// of the page, each root followed by its approved replies; past
    /// [`MAX_PAGE_ROWS`](ferrite_store::MAX_PAGE_ROWS) rows only replies are cut (the
    /// latest in thread order).
    pub items: Vec<CommentPublic>,
    /// Every approved comment of the post.
    pub total: u64,
    /// 1-based, at most `pages`.
    pub page: u32,
    /// At least 1.
    pub pages: u32,
    /// Top-level threads per page (`comments.per_page`).
    pub per_page: u32,
    /// The post's permalink for page `page - 1` ([`comment_page_url`]); `None` on page 1.
    /// No fragment: the comments section's anchor is the theme's markup, so the theme
    /// appends it (the `blog` theme links to `…#comments`, M2b-2 R4).
    pub prev_url: Option<String>,
    /// The post's permalink for page `page + 1`; `None` on the last page. No fragment,
    /// as for `prev_url`.
    pub next_url: Option<String>,
}

/// How comments are moderated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moderation {
    /// Every comment but a moderator's is held.
    All,
    /// A commenter with an approved comment posts directly.
    Known,
}

/// The comment settings in effect (stored values, else the defaults; F21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentSettings {
    pub enabled: bool,
    pub moderation: Moderation,
    /// Top-level threads per public page (1..=[`MAX_COMMENTS_PER_PAGE`]).
    pub per_page: u32,
}

/// Reads `comments.enabled`, `comments.moderation` and `comments.per_page` through
/// `repos`; an unset (or, against the validator, malformed) value is the default.
pub async fn comment_settings(repos: &dyn Repos) -> Result<CommentSettings, CoreError> {
    let options = repos.options();
    let enabled = options.get(ENABLED_OPTION).await?.map(|e| e.value);
    let moderation = options.get(MODERATION_OPTION).await?.map(|e| e.value);
    let per_page = options.get(PER_PAGE_OPTION).await?.map(|e| e.value);
    Ok(settings_from(
        enabled.as_ref(),
        moderation.as_ref(),
        per_page.as_ref(),
    ))
}

/// `comments.per_page` from its stored value: an integer in
/// 1..=[`MAX_COMMENTS_PER_PAGE`], else [`COMMENTS_PER_PAGE_DEFAULT`].
pub(crate) fn per_page_from(value: Option<&Value>) -> u32 {
    value
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (1..=MAX_COMMENTS_PER_PAGE).contains(n))
        .unwrap_or(COMMENTS_PER_PAGE_DEFAULT)
}

/// The URL of comment page `page` of the item at `path`: `path` itself for page 1,
/// else `path?cpage=N` (P34).
pub fn comment_page_url(path: &str, page: u32) -> String {
    if page <= 1 {
        path.to_owned()
    } else {
        format!("{path}?{COMMENT_PAGE_PARAM}={page}")
    }
}

/// [`CommentSettings`] from the three stored values.
pub fn settings_from(
    enabled: Option<&Value>,
    moderation: Option<&Value>,
    per_page: Option<&Value>,
) -> CommentSettings {
    let enabled = enabled
        .and_then(Value::as_bool)
        .unwrap_or(COMMENTS_ENABLED_DEFAULT);
    let mode = moderation
        .and_then(Value::as_str)
        .filter(|m| MODERATION_CHOICES.contains(m))
        .unwrap_or(COMMENTS_MODERATION_DEFAULT);
    CommentSettings {
        enabled,
        moderation: if mode == "all" {
            Moderation::All
        } else {
            Moderation::Known
        },
        per_page: per_page_from(per_page),
    }
}

/// The body as stored: `\r\n` and lone `\r` become `\n`, then trimmed.
pub(crate) fn normalize_body(body: &str) -> String {
    body.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_owned()
}

/// A plain-text body as HTML: escaped; runs of blank lines (only whitespace) separate
/// `<p>` paragraphs, single newlines become `<br>`. No links are made. `""` for a blank
/// body.
pub fn render_body(body: &str) -> String {
    let mut out = String::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let flush = |paragraph: &mut Vec<&str>, out: &mut String| {
        if paragraph.is_empty() {
            return;
        }
        out.push_str("<p>");
        for (i, line) in paragraph.iter().enumerate() {
            if i > 0 {
                out.push_str("<br>");
            }
            out.push_str(&encode_text(line));
        }
        out.push_str("</p>");
        paragraph.clear();
    };
    for line in body.split('\n') {
        if line.trim().is_empty() {
            flush(&mut paragraph, &mut out);
        } else {
            paragraph.push(line);
        }
    }
    flush(&mut paragraph, &mut out);
    out
}

/// Links in `body` for the `known` rule: each `http://`, `https://` and `www.` (ASCII
/// case-insensitive), a `www.` right after `://` counted with its scheme.
fn count_links(body: &str) -> usize {
    let lower = body.to_ascii_lowercase();
    lower.match_indices("http://").count()
        + lower.match_indices("https://").count()
        + lower
            .match_indices("www.")
            .filter(|(i, _)| !lower[..*i].ends_with("://"))
            .count()
}

fn check_body(body: &str) -> Result<String, CoreError> {
    let body = normalize_body(body);
    let len = body.chars().count();
    if len == 0
        || len > MAX_COMMENT_BODY
        || body
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(CoreError::invalid_field(
            "body",
            format!(
                "the comment must be 1 to {MAX_COMMENT_BODY} characters without control characters"
            ),
        ));
    }
    Ok(body)
}

fn check_name(name: &str) -> Result<String, CoreError> {
    let name = name.trim();
    let len = name.chars().count();
    if len == 0
        || len > MAX_COMMENT_NAME
        || name.chars().any(|c| c.is_control() || is_bidi_control(c))
    {
        return Err(CoreError::invalid_field(
            "author_name",
            format!(
                "the name must be 1 to {MAX_COMMENT_NAME} characters without control or bidi characters"
            ),
        ));
    }
    Ok(name.to_owned())
}

pub(crate) fn check_email(email: &str) -> Result<String, CoreError> {
    let email = email.trim();
    let shape_ok = match email.split_once('@') {
        Some((local, domain)) => !local.is_empty() && !domain.is_empty() && !domain.contains('@'),
        None => false,
    };
    if !shape_ok
        || email.len() > MAX_COMMENT_EMAIL
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(CoreError::invalid_field(
            "author_email",
            format!(
                "the e-mail must look like name@example.com (at most {MAX_COMMENT_EMAIL} bytes)"
            ),
        ));
    }
    Ok(email.to_owned())
}

/// `""` or an `http`/`https` URL with a host, no user info, no whitespace, control
/// characters, backslashes, quotes or angle brackets, at most [`MAX_COMMENT_URL`]
/// characters.
pub(crate) fn check_url(url: &str) -> Result<String, CoreError> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(String::new());
    }
    let bad = || {
        CoreError::invalid_field(
            "author_url",
            format!(
                "the website must be an http or https address of at most {MAX_COMMENT_URL} characters"
            ),
        )
    };
    if url.chars().count() > MAX_COMMENT_URL
        || url.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '\\' | '"' | '\'' | '<' | '>' | '`')
        })
    {
        return Err(bad());
    }
    let (scheme, rest) = url.split_once("://").ok_or_else(bad)?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(bad());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(bad());
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (addr, after) = v6.split_once(']').ok_or_else(bad)?;
        if addr.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(bad());
        }
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':').ok_or_else(bad)?),
        };
        (addr, port)
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    let host_ok = !host.is_empty()
        && (authority.starts_with('[')
            || host
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_')));
    let port_ok =
        port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    if !host_ok || !port_ok {
        return Err(bad());
    }
    Ok(url.to_owned())
}

/// Records and moderates comments, and finds the public page that shows one.
pub struct CommentService {
    store: Arc<dyn Store>,
    auth: Arc<AuthService>,
    /// For the item paths of the moderation queue.
    permalinks: Arc<dyn Permalinks>,
}

impl CommentService {
    pub fn new(
        store: Arc<dyn Store>,
        auth: Arc<AuthService>,
        permalinks: Arc<dyn Permalinks>,
    ) -> Self {
        Self {
            store,
            auth,
            permalinks,
        }
    }

    /// Submits a comment from `ip` (see the module docs), in this order: a signed-in
    /// commenter's `comments:write` scope (`Forbidden`); the per-IP limits
    /// (`RateLimited`, outside any `Tx`; not for a signed-in moderator); `comments.enabled`, then the item (public now,
    /// of kind `post` and of `input.kind`; else `NotFound`); the fields (`InvalidField`
    /// `body`, `author_name`, `author_email`, `author_url`, `parent_id`); the honeypot
    /// (`Discarded` with the would-be pending comment, nothing written); the status;
    /// then one `Tx` with the insert and, when approved, `invalidate([post:<item>])`.
    pub async fn submit(
        &self,
        input: CommentInput,
        who: Commenter,
        ip: IpAddr,
    ) -> Result<Submitted, CoreError> {
        if let Commenter::User(actor) = &who {
            require_scope(actor, Some(Scope::write(Area::Comments)))?;
        }
        let moderator =
            matches!(&who, Commenter::User(actor) if can(actor.role, Capability::ModerateComments));
        if !moderator {
            self.auth
                .hit_limit(COMMENT_MINUTE_SCOPE, Some(ip), COMMENTS_PER_MINUTE, MINUTE)
                .await?;
            self.auth
                .hit_limit(COMMENT_HOUR_SCOPE, Some(ip), COMMENTS_PER_HOUR, HOUR)
                .await?;
        }

        let repos = self.store.read();
        let settings = comment_settings(repos).await?;
        if !settings.enabled {
            return Err(CoreError::NotFound);
        }
        let now = Timestamp::now();
        let open = repos.content().get(input.item).await?.is_some_and(|item| {
            item.kind == COMMENT_KIND && item.kind == input.kind && is_public(&item, now)
        });
        if !open {
            return Err(CoreError::NotFound);
        }
        let body = check_body(&input.body)?;
        let author_url = check_url(&input.author_url)?;
        let (user, author_name, author_email) = match who {
            Commenter::Anonymous => (
                None,
                check_name(&input.author_name)?,
                check_email(&input.author_email)?,
            ),
            Commenter::User(actor) => {
                let account = repos
                    .users()
                    .get(actor.user)
                    .await?
                    .ok_or(CoreError::Unauthenticated)?;
                (Some(actor), account_name(&account), account.email)
            }
        };
        if let Some(parent) = input.parent {
            self.check_parent(repos, input.item, parent).await?;
        }
        // The honeypot is checked after every input check (M1), so a filled one never
        // changes whether input is refused; the would-be pending comment is returned.
        if !input.honeypot.is_empty() {
            return Ok(Submitted::Discarded(Comment {
                id: CommentId::new(),
                item_id: input.item,
                parent_id: input.parent,
                user_id: user.map(|a| a.user),
                author_email: ferrite_store::email_key(&author_email),
                author_name,
                author_url,
                body,
                status: CommentStatus::Pending,
                ip_hash: Some(self.auth.comment_ip_hash(ip)),
                created_at: now,
            }));
        }

        let links_ok = count_links(&body) <= MAX_KNOWN_LINKS;
        let approved = match user {
            Some(actor) if can(actor.role, Capability::ModerateComments) => true,
            _ if settings.moderation == Moderation::All || !links_ok => false,
            Some(actor) => repos.comments().has_approved_from_user(actor.user).await?,
            None => {
                repos.users().get_by_email(&author_email).await?.is_none()
                    && repos
                        .comments()
                        .has_approved_from(&author_email, &author_name)
                        .await?
            }
        };
        let comment = Comment {
            id: CommentId::new(),
            item_id: input.item,
            parent_id: input.parent,
            user_id: user.map(|a| a.user),
            author_name,
            author_email,
            author_url,
            body,
            status: if approved {
                CommentStatus::Approved
            } else {
                CommentStatus::Pending
            },
            ip_hash: Some(self.auth.comment_ip_hash(ip)),
            created_at: now,
        };
        let tx = self.store.begin().await?;
        let done = async {
            // A signed-in commenter acting through an assistant (MCP comment_reply) is
            // recorded (design 17 §3.3); anonymous comments never are.
            let mut cap = match user {
                Some(actor) => Capture::begin(tx.as_ref(), &actor, Op::CommentCreate, &[]).await?,
                None => Capture::inert(),
            };
            tx.comments()
                .insert(&comment)
                .await
                .map_err(gone_on_foreign_key)?;
            if approved {
                tx.events()
                    .append(&events::invalidate([post_key(comment.item_id)]))
                    .await?;
            }
            let by = user.map(|a| a.user);
            crate::webhooks::emit_comment(tx.as_ref(), "comment.created", &comment, None, by)
                .await?;
            cap.created(ObjectRef::Comment(comment.id));
            cap.finish(tx.as_ref()).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await?;
        // The stored row folds the e-mail's ASCII case.
        Ok(Submitted::Accepted(Comment {
            author_email: ferrite_store::email_key(&comment.author_email),
            ..comment
        }))
    }

    /// `Ok` if `parent` is an approved comment of `item` at depth below
    /// [`MAX_COMMENT_DEPTH`] (walking its ancestors), else `InvalidField(parent_id)`.
    async fn check_parent(
        &self,
        repos: &dyn Repos,
        item: ContentId,
        parent: CommentId,
    ) -> Result<(), CoreError> {
        let refused = |m: &str| CoreError::invalid_field("parent_id", m.to_owned());
        let found = repos.comments().get(parent).await?;
        let Some(found) =
            found.filter(|c| c.item_id == item && c.status == CommentStatus::Approved)
        else {
            return Err(refused(
                "the reply must be to an approved comment of the same post",
            ));
        };
        if !reply_allowed(repos, found.parent_id, &HashMap::new()).await? {
            return Err(refused(&format!(
                "replies nest at most {MAX_COMMENT_DEPTH} deep"
            )));
        }
        Ok(())
    }

    /// The moderation queue: comments newest first, filtered by status and item
    /// (`moderate_comments`, then `comments:read`), each with its item
    /// ([`CommentItem`]): one `get_many` for the page's items, then the paths of the
    /// public ones.
    pub async fn list(
        &self,
        actor: &Actor,
        query: CommentQuery,
    ) -> Result<Page<QueuedComment>, CoreError> {
        require_reader(actor, Capability::ModerateComments)?;
        let page = self.store.read().comments().list(&query).await?;
        Ok(Page {
            items: self.with_items(actor, page.items).await?,
            next_cursor: page.next_cursor,
        })
    }

    /// How many comments have each status, over every comment or `item`'s
    /// (`moderate_comments`, then `comments:read`). An unknown item counts zero.
    pub async fn counts(
        &self,
        actor: &Actor,
        item: Option<ContentId>,
    ) -> Result<CommentCounts, CoreError> {
        require_reader(actor, Capability::ModerateComments)?;
        let by_status = self.store.read().comments().counts(item).await?;
        let n = |s| by_status.get(&s).copied().unwrap_or(0);
        Ok(CommentCounts {
            pending: n(CommentStatus::Pending),
            approved: n(CommentStatus::Approved),
            spam: n(CommentStatus::Spam),
            trash: n(CommentStatus::Trash),
        })
    }

    /// The items of `comments` (at most [`MAX_ID_FILTER`](ferrite_store::MAX_ID_FILTER)
    /// distinct ones, as a page holds), read through `repos`: one `get_many`, then the
    /// paths of those with a public page now. An item without a public page keeps its
    /// title only for an `actor` who may read content ([`require_read`]`(Content)`): a
    /// `comments:read` token without `content:read` sees its id, type and status only.
    async fn items_of(
        &self,
        repos: &dyn Repos,
        actor: &Actor,
        comments: &[Comment],
    ) -> Result<HashMap<ContentId, CommentItem>, CoreError> {
        if comments.is_empty() {
            return Ok(HashMap::new());
        }
        let mut ids: Vec<ContentId> = comments.iter().map(|c| c.item_id).collect();
        ids.sort_unstable();
        ids.dedup();
        let items = repos.content().get_many(&ids).await?;
        let now = Timestamp::now();
        let reads_content = require_read(actor, Area::Content).is_ok();
        let shown: Vec<_> = items
            .iter()
            .filter(|i| has_public_page(i, now))
            .cloned()
            .collect();
        // One set of rules for the whole page (R10); the cache keys are not needed.
        let rules = self.permalinks.pin();
        let paths: HashMap<ContentId, String> =
            public_paths(repos, rules.as_ref(), shown, &mut DepKeys::default())
                .await?
                .into_iter()
                .map(|(item, path)| (item.id, path))
                .collect();
        Ok(items
            .into_iter()
            .map(|i| {
                let permalink = paths.get(&i.id).cloned();
                let title = if reads_content || has_public_page(&i, now) {
                    i.title
                } else {
                    String::new()
                };
                let item = CommentItem {
                    id: i.id,
                    permalink,
                    kind: i.kind,
                    title,
                    status: i.status,
                };
                (item.id, item)
            })
            .collect())
    }

    /// `comments` with their items ([`items_of`](Self::items_of)).
    async fn with_items(
        &self,
        actor: &Actor,
        comments: Vec<Comment>,
    ) -> Result<Vec<QueuedComment>, CoreError> {
        let items = self.items_of(self.store.read(), actor, &comments).await?;
        comments
            .into_iter()
            .map(|comment| queued(comment, &items))
            .collect()
    }

    /// Sets comment `id`'s status (`moderate_comments`, then `comments:write`) and
    /// invalidates its item's page, in one `Tx`; returns the comment with its item (read
    /// in the same `Tx`, so a failed read changes nothing). Missing is `NotFound`. Not audited (P14).
    pub async fn moderate(
        &self,
        actor: &Actor,
        id: CommentId,
        status: CommentStatus,
    ) -> Result<QueuedComment, CoreError> {
        require(actor, Capability::ModerateComments)?;
        let tx = self.store.begin().await?;
        let done = async {
            let cap = Capture::begin(
                tx.as_ref(),
                actor,
                Op::CommentModerate,
                &[ObjectRef::Comment(id)],
            )
            .await?;
            let previous = tx.comments().get(id).await?.map(|c| c.status);
            let comment = tx
                .comments()
                .set_status(id, status)
                .await?
                .ok_or(CoreError::NotFound)?;
            if let Some(previous) = previous.filter(|p| *p != comment.status) {
                let by = Some(actor.user);
                let kind = "comment.moderated";
                crate::webhooks::emit_comment(tx.as_ref(), kind, &comment, Some(previous), by)
                    .await?;
            }
            tx.events()
                .append(&events::invalidate([post_key(comment.item_id)]))
                .await?;
            // Read in the `Tx`: a failure here rolls the change back.
            let items = self
                .items_of(tx.as_ref(), actor, std::slice::from_ref(&comment))
                .await?;
            let out = queued(comment, &items)?;
            cap.finish(tx.as_ref()).await?;
            Ok(out)
        }
        .await;
        finish(tx, done).await
    }

    /// Deletes comment `id` and its replies (`moderate_comments`, then
    /// `comments:write`); only a `spam` or `trash` comment (else
    /// `InvalidField(status)`); invalidates its item's page in the same `Tx`. Missing is
    /// `NotFound`.
    pub async fn delete(&self, actor: &Actor, id: CommentId) -> Result<(), CoreError> {
        require(actor, Capability::ModerateComments)?;
        let tx = self.store.begin().await?;
        let done = async {
            let comment = tx.comments().get(id).await?.ok_or(CoreError::NotFound)?;
            if !matches!(comment.status, CommentStatus::Spam | CommentStatus::Trash) {
                return Err(CoreError::invalid_field(
                    "status",
                    "only spam or trashed comments can be deleted",
                ));
            }
            if !tx.comments().delete(id).await? {
                return Err(CoreError::NotFound);
            }
            let by = Some(actor.user);
            crate::webhooks::emit_comment(tx.as_ref(), "comment.deleted", &comment, None, by)
                .await?;
            tx.events()
                .append(&events::invalidate([post_key(comment.item_id)]))
                .await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }

    /// The public comment page (1-based, P34) that shows approved comment `comment` of
    /// `item` under the current `comments.per_page`: its root's position divided by the
    /// page size, plus one. `None` when it is not an approved comment of `item`. No
    /// visibility check: for the redirect after a submit.
    pub async fn page_of(
        &self,
        item: ContentId,
        comment: CommentId,
    ) -> Result<Option<u32>, CoreError> {
        let repos = self.store.read();
        let per_page = comment_settings(repos).await?.per_page;
        let position = repos.comments().root_position(item, comment).await?;
        Ok(position.map(|p| u32::try_from(p / u64::from(per_page) + 1).unwrap_or(u32::MAX)))
    }
}

/// `comment` with its item from `items`. The comments' foreign key keeps every item: one
/// missing (deleted between the two reads) is an internal error, never a made-up item.
fn queued(
    comment: Comment,
    items: &HashMap<ContentId, CommentItem>,
) -> Result<QueuedComment, CoreError> {
    let item = items.get(&comment.item_id).cloned().ok_or_else(|| {
        CoreError::Store(StoreError::Database(format!(
            "comment {} is on item {}, which does not exist",
            comment.id, comment.item_id
        )))
    })?;
    Ok(QueuedComment { comment, item })
}

/// Whether a reply to a comment whose parent is `up` is within [`MAX_COMMENT_DEPTH`]:
/// the one depth rule of submits and of [`CommentPublic::can_reply`]. Walks the stored
/// chain (any status; a missing ancestor ends it), looking ids up in `known`
/// (id → parent) before the store; at most [`MAX_COMMENT_DEPTH`] steps, so a cycle
/// ends too.
async fn reply_allowed(
    repos: &dyn Repos,
    mut up: Option<CommentId>,
    known: &HashMap<CommentId, Option<CommentId>>,
) -> Result<bool, CoreError> {
    let mut depth: u8 = 0;
    while let Some(id) = up {
        depth += 1;
        if depth >= MAX_COMMENT_DEPTH {
            return Ok(false);
        }
        up = match known.get(&id) {
            Some(parent) => *parent,
            None => repos.comments().get(id).await?.and_then(|c| c.parent_id),
        };
    }
    Ok(true)
}

/// Approved `rows` (one [`approved_page`](ferrite_store::CommentRepo::approved_page))
/// as the public thread, with [`can_reply`](CommentPublic::can_reply) from the stored
/// chains. Chains inside `rows` cost no store call; a reply whose ancestors are not
/// shown (trashed, spam, held) costs at most [`MAX_COMMENT_DEPTH`] lookups.
pub(crate) async fn public_thread(
    repos: &dyn Repos,
    rows: Vec<Comment>,
) -> Result<Vec<CommentPublic>, CoreError> {
    let known: HashMap<CommentId, Option<CommentId>> =
        rows.iter().map(|c| (c.id, c.parent_id)).collect();
    let mut replyable = HashSet::new();
    for c in &rows {
        if reply_allowed(repos, c.parent_id, &known).await? {
            replyable.insert(c.id);
        }
    }
    Ok(thread_of(rows, &replyable))
}

/// `rows` in thread order with depths (roots in the order given, each reply right after
/// its parent, siblings in the order given); `replyable` holds the ids a reply may go
/// to.
fn thread_of(rows: Vec<Comment>, replyable: &HashSet<CommentId>) -> Vec<CommentPublic> {
    let index: HashMap<CommentId, usize> =
        rows.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    let mut roots = Vec::new();
    for (i, c) in rows.iter().enumerate() {
        match c.parent_id.and_then(|p| index.get(&p)) {
            Some(&p) if p != i => children[p].push(i),
            _ => roots.push(i),
        }
    }
    let mut order: Vec<(usize, u8)> = Vec::with_capacity(rows.len());
    // Depth first; children pushed in reverse so the oldest comes out first.
    let mut stack: Vec<(usize, u8)> = roots.iter().rev().map(|&i| (i, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        order.push((i, depth));
        for &child in children[i].iter().rev() {
            stack.push((child, depth.saturating_add(1)));
        }
    }
    let mut slots: Vec<Option<Comment>> = rows.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|(i, depth)| {
            let c = slots[i].take()?;
            Some(CommentPublic {
                id: c.id,
                parent_id: if depth == 0 { None } else { c.parent_id },
                depth,
                html: render_body(&c.body),
                author_url: (!c.author_url.is_empty()).then_some(c.author_url),
                author_name: c.author_name,
                date: c.created_at,
                can_reply: replyable.contains(&c.id),
            })
        })
        .collect()
}

/// The name a signed-in commenter's comments carry: the display name without bidi
/// controls (one stored before R13 may hold them) or other control characters, cut to
/// [`MAX_COMMENT_NAME`] characters (the login if that leaves it blank).
fn account_name(user: &ferrite_domain::User) -> String {
    let clean: String = user
        .display_name
        .chars()
        .filter(|&c| !c.is_control() && !is_bidi_control(c))
        .collect();
    let name = clean.trim();
    let name = if name.is_empty() { &user.login } else { name };
    name.chars().take(MAX_COMMENT_NAME).collect()
}

/// An insert whose item or parent was deleted meanwhile: `NotFound`.
fn gone_on_foreign_key(e: StoreError) -> CoreError {
    match e {
        StoreError::ForeignKey { .. } => CoreError::NotFound,
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_page_urls() {
        assert_eq!(comment_page_url("/hello/", 0), "/hello/");
        assert_eq!(comment_page_url("/hello/", 1), "/hello/");
        assert_eq!(comment_page_url("/hello/", 3), "/hello/?cpage=3");
    }

    #[test]
    fn links_are_counted_once_each() {
        assert_eq!(count_links("no links"), 0);
        assert_eq!(count_links("http://a https://b www.c"), 3);
        assert_eq!(count_links("HTTPS://WWW.a.example"), 1);
        assert_eq!(count_links("http://www.a and www.b"), 2);
    }

    #[test]
    fn urls() {
        for ok in [
            "",
            "http://a.example",
            "HTTPS://a.example:8443/p?q=1#f",
            "https://xn--bcher-kva.example/",
            "https://bücher.example/",
            "http://[::1]:80/",
            "http://127.0.0.1/",
        ] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt://x",
            "data:text/html,x",
            "//a.example",
            "http:/a.example",
            "http://",
            "http://:80",
            "http://a.example:x/",
            "http://a.example:123456/",
            "http://u:p@a.example/",
            "http://a b.example/",
            "http://a.example/\u{0}",
            "http://a.example/<script>",
            "http://[nope]/",
            "http://[::1]x/",
            "http://[::1]:/",
            "http://a\\b.example/",
            "http://a!.example/",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn defaults_and_stored_settings() {
        let d = settings_from(None, None, None);
        assert_eq!(
            d,
            CommentSettings {
                enabled: true,
                moderation: Moderation::Known,
                per_page: COMMENTS_PER_PAGE_DEFAULT,
            }
        );
        let s = settings_from(
            Some(&Value::Bool(false)),
            Some(&Value::String("all".into())),
            Some(&Value::from(7)),
        );
        assert!(!s.enabled);
        assert_eq!(s.moderation, Moderation::All);
        assert_eq!(s.per_page, 7);
        assert_eq!(
            settings_from(None, Some(&Value::String("bogus".into())), None).moderation,
            Moderation::Known
        );
        for bad in [
            Value::from(0),
            Value::from(101),
            Value::from(-1),
            Value::from(2.5),
            Value::from("5"),
        ] {
            assert_eq!(
                settings_from(None, None, Some(&bad)).per_page,
                COMMENTS_PER_PAGE_DEFAULT,
                "{bad}"
            );
        }
    }

    #[test]
    fn thread_cycle_and_self_parent_do_not_loop() {
        let item = ContentId::new();
        let mk = |id: CommentId, parent: Option<CommentId>| Comment {
            id,
            item_id: item,
            parent_id: parent,
            user_id: None,
            author_name: "n".into(),
            author_email: "e@x".into(),
            author_url: String::new(),
            body: "b".into(),
            status: CommentStatus::Approved,
            ip_hash: None,
            created_at: Timestamp::now(),
        };
        let (a, b) = (CommentId::new(), CommentId::new());
        let selfish = CommentId::new();
        let t = thread_of(
            vec![mk(a, Some(b)), mk(b, Some(a)), mk(selfish, Some(selfish))],
            &HashSet::new(),
        );
        // The cycle has no root and is not shown; the self-parent is a root.
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].id, t[0].depth), (selfish, 0));
    }

    #[test]
    fn body_rules() {
        assert_eq!(check_body(" a\r\nb\rc ").unwrap(), "a\nb\nc");
        assert!(check_body("tab\tok").is_ok());
        assert!(check_body("\u{1b}[31m").is_err());
        assert!(check_body("\u{85}").is_err());
    }
}
