//! The records a WXR file holds, as read, before any conversion.
//!
//! Strings are kept exactly as WordPress wrote them (dates as `YYYY-MM-DD HH:MM:SS`, statuses
//! as WP names); the conversion interprets them.

use serde::{Deserialize, Serialize};

/// The `<channel>` header.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub title: String,
    pub link: String,
    pub description: String,
    pub base_site_url: String,
    pub base_blog_url: String,
    pub wxr_version: String,
    pub language: String,
}

/// A `<wp:author>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Author {
    pub wp_id: i64,
    pub login: String,
    pub email: String,
    pub display_name: String,
    pub first_name: String,
    pub last_name: String,
}

/// A `<wp:category>` (taxonomy `category`), `<wp:tag>` (`post_tag`) or `<wp:term>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Term {
    pub wp_id: i64,
    pub taxonomy: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub parent_slug: Option<String>,
}

/// An `<item>`: a post of any type, with its terms, meta and comments.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub wp_id: i64,
    pub post_type: String,
    pub status: String,
    pub title: String,
    pub link: String,
    pub guid: String,
    pub creator_login: String,
    pub content: String,
    pub excerpt: String,
    pub post_name: String,
    pub parent_wp_id: i64,
    pub menu_order: i32,
    pub date_local: String,
    pub date_gmt: String,
    pub modified_gmt: String,
    /// `wp:post_modified`, read as UTC when `modified_gmt` is zero (P7).
    #[serde(default)]
    pub modified_local: String,
    pub comment_status: String,
    pub is_sticky: bool,
    pub attachment_url: Option<String>,
    pub terms: Vec<ItemTerm>,
    pub meta: Vec<(String, String)>,
    pub comments: Vec<Comment>,
}

/// An item's `<category domain="…" nicename="…">name</category>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemTerm {
    pub taxonomy: String,
    pub slug: String,
    pub name: String,
}

/// A `<wp:comment>`. `approved` is WP's raw value (`1`, `0`, `spam`, `trash`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub wp_id: i64,
    pub author: String,
    pub author_email: String,
    pub author_url: String,
    pub date_gmt: String,
    pub date_local: String,
    pub content: String,
    pub approved: String,
    pub comment_type: String,
    pub parent_wp_id: i64,
    pub user_id: i64,
}

/// One record of the stream, in file order; the channel always comes first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Record {
    Channel(Channel),
    Author(Author),
    Term(Term),
    Item(Box<Item>),
}
