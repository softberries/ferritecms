//! WXR comments → comments (pass 4, P18).
//!
//! The comment HTML becomes plain text ([`plain_text`]). Status: `1` → approved, `0` →
//! pending, `spam` → spam, `trash` → trash (WordPress does not export spam, R4, but the
//! mapping stands). Pingbacks and trackbacks are skipped and counted. Threading is kept;
//! a parent that was skipped, or one on another item, makes the child top-level (the
//! latter noted). A comment whose WP `user_id` is a
//! mapped account gets that user (and so `by_account`). Comments of items that were not
//! imported are skipped.

use ferrite_core::import::{ImportBatch, ImportComment};
use ferrite_domain::{CommentId, CommentStatus, ContentId, ImportMapKind, Timestamp, UserId};
use futures::TryStreamExt;
use std::collections::HashMap;

use scraper::Node;

use super::content::parent_first;
use super::types::type_name;
use super::wp_time;
use crate::ImportError;
use crate::convert::Notes;
use crate::convert::html::parse_guarded;
use crate::ir::Ir;
use crate::model::Comment;
use crate::run::{Report, Session};

/// The report kind of comments.
const KIND: &str = "comments";

/// WP `comment_approved` → status; `None` for a value WordPress does not use.
pub(crate) fn status(approved: &str) -> Option<CommentStatus> {
    Some(match approved.trim() {
        "1" => CommentStatus::Approved,
        "0" => CommentStatus::Pending,
        "spam" => CommentStatus::Spam,
        "trash" => CommentStatus::Trash,
        _ => return None,
    })
}

/// Comment HTML as plain text, and whether it was too deep to parse: tags stripped (the
/// text of `script` and `style` dropped), entities decoded, `<p>` (and the other blocks)
/// and line breaks in the source as blank lines, `<br>` as a newline; spaces collapsed,
/// the result trimmed. Markup the tree builder would take quadratic time on (the
/// conversion's depth guard) is not parsed: its tags are dropped as text, entities left
/// as they are.
pub(crate) fn plain_text_guarded(html: &str) -> (String, bool) {
    let html = html.replace("\r\n", "\n").replace('\r', "\n");
    let mut notes = Notes::default();
    let Some(doc) = parse_guarded(&html, &mut notes) else {
        return (strip_tags(&html), true);
    };
    let mut out = String::new();
    for node in doc.tree.root().descendants() {
        match node.value() {
            Node::Text(text) => {
                let in_code = node
                    .parent()
                    .and_then(|p| p.value().as_element().map(|e| e.name().to_owned()))
                    .is_some_and(|n| n == "script" || n == "style");
                if !in_code {
                    push_text(&mut out, text);
                }
            }
            Node::Element(e) => match e.name() {
                "br" => newlines(&mut out, 1),
                "p" | "div" | "blockquote" | "li" | "ul" | "ol" | "pre" | "h1" | "h2" | "h3"
                | "h4" | "h5" | "h6" => newlines(&mut out, 2),
                _ => {}
            },
            _ => {}
        }
    }
    (out.trim().to_owned(), false)
}

/// The text between the tags of `html`, in one linear pass.
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match (in_tag, c) {
            (false, '<') => {
                push_text(&mut out, &text);
                text.clear();
                in_tag = true;
            }
            (true, '>') => in_tag = false,
            (false, c) => text.push(c),
            (true, _) => {}
        }
    }
    push_text(&mut out, &text);
    out.trim().to_owned()
}

/// Appends text: newlines as paragraph breaks, other whitespace as one space, control
/// characters dropped.
fn push_text(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\n' => newlines(out, 2),
            c if c.is_whitespace() => {
                if !out.is_empty() && !out.ends_with([' ', '\n']) {
                    out.push(' ');
                }
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

/// Ends `out` with at least `n` newlines (none at the start), trailing spaces removed.
fn newlines(out: &mut String, n: usize) {
    out.truncate(out.trim_end_matches(' ').len());
    if out.is_empty() {
        return;
    }
    let have = out.len() - out.trim_end_matches('\n').len();
    for _ in have..n {
        out.push('\n');
    }
}

/// One comment with the WP id of its item.
struct Pending {
    item: i64,
    comment: Comment,
}

/// Imports the comments of every imported item, parents first, `batch_items` per batch.
pub(crate) async fn import(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    // Only (comment, parent, item) ids stay in memory; the comments themselves are read
    // from the IR per chunk.
    let mut refs: Vec<(i64, i64, i64)> = Vec::new();
    let mut items = session.ir().items(&[]);
    while let Some(item) = items.try_next().await? {
        if type_name(&item.post_type).is_none() {
            continue;
        }
        refs.extend(
            item.comments
                .iter()
                .map(|c| (c.wp_id, c.parent_wp_id, item.wp_id)),
        );
    }
    drop(items);
    let order = comment_order(&refs);
    // The WP item of each comment, to tell a parent on another item.
    let items_of: HashMap<i64, i64> = refs.iter().map(|(c, _, i)| (*c, *i)).collect();
    drop(refs);
    let mut pings = 0u64;
    for chunk in order.chunks(session.batch_items()) {
        let pending = load(session.ir(), chunk).await?;
        let mut batch = session.batch().await?;
        let mut part = Report::default();
        let written = write(&mut batch, &pending, &items_of, &mut part, &mut pings).await;
        session.finish_batch(batch, written).await?;
        report.absorb(part);
    }
    if pings > 0 {
        report.note(format!("{pings} pingbacks and trackbacks were skipped"));
    }
    Ok(())
}

/// The comments of `refs` (comment, parent, item WP ids) as (comment, item), each parent
/// before its children.
fn comment_order(refs: &[(i64, i64, i64)]) -> Vec<(i64, i64)> {
    let nodes: Vec<(i64, i64)> = refs.iter().map(|(c, p, _)| (*c, *p)).collect();
    let item_of: HashMap<i64, i64> = refs.iter().map(|(c, _, i)| (*c, *i)).collect();
    parent_first(&nodes)
        .into_iter()
        .filter_map(|c| item_of.get(&c).map(|i| (c, *i)))
        .collect()
}

/// The comments of `chunk` (comment, item), read from their items in the IR, in order.
async fn load(ir: &Ir, chunk: &[(i64, i64)]) -> Result<Vec<Pending>, ImportError> {
    let mut found: HashMap<i64, Comment> = HashMap::new();
    let mut read: Vec<i64> = Vec::new();
    for (_, item) in chunk {
        if read.contains(item) {
            continue;
        }
        read.push(*item);
        let wanted: Vec<i64> = chunk
            .iter()
            .filter(|(_, i)| i == item)
            .map(|(c, _)| *c)
            .collect();
        if let Some(it) = ir.item(*item).await? {
            found.extend(
                it.comments
                    .into_iter()
                    .filter(|c| wanted.contains(&c.wp_id))
                    .map(|c| (c.wp_id, c)),
            );
        }
    }
    Ok(chunk
        .iter()
        .filter_map(|(c, item)| {
            found.remove(c).map(|comment| Pending {
                item: *item,
                comment,
            })
        })
        .collect())
}

async fn write(
    b: &mut ImportBatch<'_>,
    chunk: &[Pending],
    items_of: &HashMap<i64, i64>,
    report: &mut Report,
    pings: &mut u64,
) -> Result<(), ImportError> {
    for p in chunk {
        let c = &p.comment;
        if matches!(c.comment_type.as_str(), "pingback" | "trackback") {
            report.counts_of(KIND).skipped += 1;
            *pings += 1;
            continue;
        }
        let Some(status) = status(&c.approved) else {
            report.counts_of(KIND).skipped += 1;
            report.note(format!(
                "comment {} skipped: status {:?}",
                c.wp_id, c.approved
            ));
            continue;
        };
        let Some(item) = b.lookup(ImportMapKind::Content, p.item).await? else {
            report.counts_of(KIND).skipped += 1;
            continue;
        };
        if b.lookup(ImportMapKind::Comment, c.wp_id).await?.is_some() {
            report.counts_of(KIND).unchanged += 1;
            continue;
        }
        let parent = match c.parent_wp_id {
            0 => None,
            wp if items_of.get(&wp).is_some_and(|item| *item != p.item) => {
                report.note(format!(
                    "comment {}: its parent {wp} is on another item; made top-level",
                    c.wp_id
                ));
                None
            }
            wp => b
                .lookup(ImportMapKind::Comment, wp)
                .await?
                .map(|m| CommentId::from_uuid(m.id)),
        };
        let user = match c.user_id {
            0 => None,
            wp => b
                .lookup(ImportMapKind::User, wp)
                .await?
                .map(|m| UserId::from_uuid(m.id)),
        };
        let (body, too_deep) = plain_text_guarded(&c.content);
        let outcome = b
            .comment(ImportComment {
                wp_id: c.wp_id,
                item: ContentId::from_uuid(item.id),
                parent,
                user,
                author_name: c.author.trim().to_owned(),
                author_email: c.author_email.trim().to_owned(),
                author_url: c.author_url.trim().to_owned(),
                body,
                status,
                created_at: wp_time(&c.date_gmt, &c.date_local).unwrap_or_else(Timestamp::now),
            })
            .await?;
        report.counts_of(KIND).created += 1;
        if too_deep {
            report.note(format!(
                "comment {}: its markup is nested too deep to parse; kept as text",
                c.wp_id
            ));
        }
        if !outcome.altered.is_empty() {
            report.note(format!(
                "comment {}: {} changed to fit the comment rules",
                c.wp_id,
                outcome.altered.join(", ")
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(html: &str) -> String {
        plain_text_guarded(html).0
    }

    #[test]
    fn statuses_map_as_p18_says() {
        assert_eq!(status("1"), Some(CommentStatus::Approved));
        assert_eq!(status("0"), Some(CommentStatus::Pending));
        assert_eq!(status("spam"), Some(CommentStatus::Spam));
        assert_eq!(status("trash"), Some(CommentStatus::Trash));
        assert_eq!(status("post-trashed"), None);
    }

    #[test]
    fn html_becomes_plain_text() {
        assert_eq!(
            plain_text(
                "Lovely <a href=\"https://spam.example\">photos</a>!\nSecond paragraph <b>bold</b>."
            ),
            "Lovely photos!\n\nSecond paragraph bold."
        );
        assert_eq!(
            plain_text("<p>One &amp; two</p><p>Three<br>four<script>x()</script></p>"),
            "One & two\n\nThree\nfour"
        );
        assert_eq!(plain_text("  a   b\r\n\r\n\r\nc  "), "a b\n\nc");
    }

    /// B1: a hostile body (40 000 nested divs) is not parsed (the tree builder would take
    /// quadratic time): its tags are dropped as text, fast, and it is reported.
    #[test]
    fn hostile_nesting_is_text_and_reported() {
        let html = format!("{}deep{}", "<div>".repeat(40_000), "</div>".repeat(40_000));
        let started = std::time::Instant::now();
        let (text, too_deep) = plain_text_guarded(&html);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(too_deep);
        assert_eq!(text, "deep");
        let (text, too_deep) = plain_text_guarded("<p>fine &amp; dandy</p>");
        assert!(!too_deep);
        assert_eq!(text, "fine & dandy");
    }

    /// C4: comments are ordered from (comment, parent, item) ids alone, parents first;
    /// a parent on another item does not hold its child back.
    #[test]
    fn order_uses_ids_only() {
        let refs = [(3, 2, 10), (2, 1, 10), (1, 0, 10), (4, 0, 11), (5, 1, 11)];
        assert_eq!(
            comment_order(&refs),
            [(1, 10), (2, 10), (3, 10), (4, 11), (5, 11)]
        );
    }
}
