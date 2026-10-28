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
use scraper::{Html, Node};

use super::content::parent_first;
use super::types::type_name;
use super::wp_time;
use crate::ImportError;
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

/// Comment HTML as plain text: tags stripped (the text of `script` and `style` dropped),
/// entities decoded, `<p>` (and the other blocks) and line breaks in the source as blank
/// lines, `<br>` as a newline; spaces collapsed, the result trimmed.
pub fn plain_text(html: &str) -> String {
    let doc = Html::parse_fragment(&html.replace("\r\n", "\n").replace('\r', "\n"));
    let mut out = String::new();
    for node in doc.tree.root().descendants() {
        match node.value() {
            Node::Text(text) => {
                let in_code = node
                    .parent()
                    .and_then(|p| p.value().as_element().map(|e| e.name().to_owned()))
                    .is_some_and(|n| n == "script" || n == "style");
                if in_code {
                    continue;
                }
                for c in text.chars() {
                    match c {
                        '\n' => newlines(&mut out, 2),
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
            Node::Element(e) => match e.name() {
                "br" => newlines(&mut out, 1),
                "p" | "div" | "blockquote" | "li" | "ul" | "ol" | "pre" | "h1" | "h2" | "h3"
                | "h4" | "h5" | "h6" => newlines(&mut out, 2),
                _ => {}
            },
            _ => {}
        }
    }
    out.trim().to_owned()
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
    let mut all = Vec::new();
    let mut items = session.ir().items(&[]);
    while let Some(item) = items.try_next().await? {
        if type_name(&item.post_type).is_none() {
            continue;
        }
        for comment in item.comments {
            all.push(Pending {
                item: item.wp_id,
                comment,
            });
        }
    }
    drop(items);
    let nodes: Vec<(i64, i64)> = all
        .iter()
        .map(|p| (p.comment.wp_id, p.comment.parent_wp_id))
        .collect();
    let order = parent_first(&nodes);
    let mut by_id: std::collections::HashMap<i64, Pending> =
        all.into_iter().map(|p| (p.comment.wp_id, p)).collect();
    let ordered: Vec<Pending> = order.iter().filter_map(|id| by_id.remove(id)).collect();
    // The WP item of each comment, to tell a parent on another item.
    let items_of: std::collections::HashMap<i64, i64> =
        ordered.iter().map(|p| (p.comment.wp_id, p.item)).collect();
    let mut pings = 0u64;
    for chunk in ordered.chunks(session.batch_items()) {
        let mut batch = session.batch().await?;
        let mut part = Report::default();
        let written = write(&mut batch, chunk, &items_of, &mut part, &mut pings).await;
        session.finish_batch(batch, written).await?;
        let counts = report.counts_of(KIND);
        let c = part.counts_of(KIND);
        counts.created += c.created;
        counts.unchanged += c.unchanged;
        counts.skipped += c.skipped;
        report.notes.extend(part.notes);
    }
    if pings > 0 {
        report.note(format!("{pings} pingbacks and trackbacks were skipped"));
    }
    Ok(())
}

async fn write(
    b: &mut ImportBatch<'_>,
    chunk: &[Pending],
    items_of: &std::collections::HashMap<i64, i64>,
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
        b.comment(ImportComment {
            wp_id: c.wp_id,
            item: ContentId::from_uuid(item.id),
            parent,
            user,
            author_name: c.author.trim().to_owned(),
            author_email: c.author_email.trim().to_owned(),
            author_url: c.author_url.trim().to_owned(),
            body: plain_text(&c.content),
            status,
            created_at: wp_time(&c.date_gmt, &c.date_local).unwrap_or_else(Timestamp::now),
        })
        .await?;
        report.counts_of(KIND).created += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
