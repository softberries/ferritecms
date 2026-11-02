//! MCP prompts (design 12 §5.5): message templates a user picks in their client. They
//! only produce messages, never read or change the site; the tools they name do that,
//! as the caller, when the model calls them.

use rmcp::{
    handler::server::wrapper::Parameters,
    model::{PromptMessage, Role},
    prompt, prompt_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use super::FerriteMcp;

/// The arguments of `write_post`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WritePostArgs {
    /// What the post is about.
    pub topic: String,
    /// The tone, e.g. "friendly", "formal" (default: the site's usual tone).
    #[serde(default)]
    pub tone: Option<String>,
}

/// The arguments of `review_seo`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviewSeoArgs {
    /// The item to review: its id, title or URL.
    pub item: String,
}

fn user(text: String) -> Vec<PromptMessage> {
    vec![PromptMessage::new_text(Role::User, text)]
}

#[prompt_router(vis = "pub(crate)")]
impl FerriteMcp {
    /// Draft a blog post on a topic, saved as a draft for the user to review.
    #[prompt(name = "write_post")]
    pub async fn write_post(
        &self,
        Parameters(args): Parameters<WritePostArgs>,
    ) -> Vec<PromptMessage> {
        let tone = args
            .tone
            .filter(|t| !t.trim().is_empty())
            .map_or_else(String::new, |t| format!(" Write in a {} tone.", t.trim()));
        user(format!(
            "Write a blog post about: {topic}.{tone}\n\n\
             1. Call site_overview and read the resource ferrite://guide/writing.\n\
             2. Use site_search to find related posts worth linking to.\n\
             3. Write the post in Markdown with a clear title, short paragraphs and `##` \
             headings, and an SEO description of at most 160 characters.\n\
             4. Save it with content_create as a **draft** of type `post` (title, \
             body_markdown, seo). Do not publish it.\n\
             5. Show me the draft with content_preview_link and wait for my review.",
            topic = args.topic.trim(),
        ))
    }

    /// Review one item's SEO: title, description and headings, with suggestions.
    #[prompt(name = "review_seo")]
    pub async fn review_seo(
        &self,
        Parameters(args): Parameters<ReviewSeoArgs>,
    ) -> Vec<PromptMessage> {
        user(format!(
            "Review the SEO of this item: {item}.\n\n\
             Find it (content_get by id, or site_search by title or URL), then check its \
             title, `seo.title` (the whole page title, ideally under 60 characters), \
             `seo.description` (under 160 characters, the page's summary), the heading \
             structure of the body (one topic per `##`, no skipped levels), image alt texts \
             and the excerpt. List concrete suggestions with the text you propose. Change \
             nothing until I approve; then apply the approved ones with content_update.",
            item = args.item.trim(),
        ))
    }

    /// Summarise the site's health: status, dead jobs and queued ones with a last error,
    /// and plugin failures.
    #[prompt(name = "site_health")]
    pub async fn site_health(&self) -> Vec<PromptMessage> {
        user(
            "Check the health of this site and summarise it for me.\n\n\
             Call system_status (versions, database schema, nodes, job counts), jobs_list \
             for dead jobs, and queued ones with a last error, and plugins_list for plugins \
             with recent failures. \
             Report what is fine, what needs attention and what I can do about it (the admin \
             screen to use). Do not retry jobs or change plugins; just report."
                .to_owned(),
        )
    }

    /// Find images without alt text and propose some.
    #[prompt(name = "tidy_media")]
    pub async fn tidy_media(&self) -> Vec<PromptMessage> {
        user(
            "Find the images in the media library that have no alt text.\n\n\
             Page through media_list (kind image), and for each image without alt text \
             propose a short, descriptive alt text from its title, file name and where it \
             is used. Show me the list first; after I confirm, set the approved ones with \
             media_update."
                .to_owned(),
        )
    }
}
