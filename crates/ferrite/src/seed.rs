//! `ferrite seed demo`: demo content for a new site.
//!
//! Writes, through the core services where they exist: an `admin` user (no password,
//! `must_reset`, so it logs in only after `ferrite user reset-password`), categories News
//! and Guides, tags rust and cms, 12 published posts with block bodies dated over the
//! past months, three pages (About, About/Team, Contact), a `primary` and a `footer`
//! menu, one sample image (a PNG generated here, stored with its variants: the core
//! sizes plus the active theme's) used as the featured image of some posts, and the
//! `site.title` / `site.description` options.
//!
//! Idempotency: if the site has any content item, `seed demo` refuses (exit 1) unless
//! `--force` is given. With `--force` it adds another batch: the user, terms and image
//! file are reused when they exist, posts and pages get fresh slugs (`-2`, `-3`, … as
//! the content service allocates them), and both menus and the two options are
//! replaced to point at the new batch.
//!
//! **Atomic.** The image files are stored first (content-addressed, so a repeat is a
//! no-op), then every row is written through one [`DemoBatch`], a single transaction:
//! the user, terms, the image's media item, artifacts and `media` row, posts, pages,
//! menus and options, with the events the services emit. A run that fails part-way
//! writes no row; a plain re-run then succeeds. The image files of such a run stay
//! behind for the artifacts GC.
//!
//! The image's variants are made here, in-process, with the sizes a `media.variants`
//! job would use (`variant_specs`), not by a job: the seed runs without a node. Its
//! `media` row and `artifacts` rows are written in the same transaction as the media
//! item, which also emits `post:<media id>` (ruling R11).

mod demo;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, bail};
use bytes::Bytes;
use clap::{Args, Subcommand};
use ferrite_core::{Actor, ContentDraft, seed::DemoBatch};
use ferrite_domain::{
    Artifact, ArtifactKind, ContentId, ContentQuery, Media, Menu, MenuItem, MenuItemId,
    MenuItemKind, NewUser, Role, TermId, Timestamp, UserId, UserStatus,
};
use ferrite_http::media::variant_specs;
use ferrite_media::{MediaStore, VariantSpec, process_image};
use ferrite_store::Store;
use serde_json::{Value, json};

use crate::{
    binary_version, load_config,
    open::{OpenMode, open_store, startup_check},
    site::{load_theme, open_media, permalink_rules},
};

const ADMIN_LOGIN: &str = "admin";
const DAY_MICROS: i64 = 86_400_000_000;

#[derive(Debug, Subcommand)]
pub enum SeedCommand {
    /// Add demo content: posts, pages, terms, menus and a sample image.
    Demo(DemoArgs),
}

#[derive(Debug, Args)]
pub struct DemoArgs {
    /// The site's data folder (contains ferrite.toml).
    #[arg(long)]
    pub data: PathBuf,
    /// Add another demo batch even if the site already has content.
    #[arg(long)]
    pub force: bool,
    /// Test seam: fail after the posts, to show that nothing stays written.
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub fail_after_posts: bool,
}

pub async fn run(cmd: SeedCommand) -> anyhow::Result<()> {
    let SeedCommand::Demo(args) = cmd;
    let config = load_config(&args.data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let seeded = async {
        startup_check(store.as_ref(), &binary_version()?, &config.database).await?;
        let media = open_media(&config)?;
        let theme = load_theme(
            store.as_ref(),
            &media,
            &config.data_dir,
            config.effective_cluster_mode(),
        )
        .await?
        .theme;
        // The sizes a `media.variants` job would make on a node with this theme.
        let specs = variant_specs(&theme);
        seed_demo(
            store.clone(),
            &media,
            &specs,
            Options {
                force: args.force,
                #[cfg(feature = "test-seams")]
                fail_after_posts: args.fail_after_posts,
            },
        )
        .await
    }
    .await;
    store.close().await;
    let s = seeded?;
    println!(
        "Seeded demo content: {} posts, {} pages, {} terms, 2 menus, 1 image (admin user: {ADMIN_LOGIN}).",
        s.posts, s.pages, s.terms
    );
    Ok(())
}

/// What one run wrote.
struct Summary {
    posts: usize,
    pages: usize,
    terms: usize,
}

/// What one run does.
struct Options {
    force: bool,
    /// Test seam: the batch fails after the posts.
    #[cfg(feature = "test-seams")]
    fail_after_posts: bool,
}

async fn seed_demo(
    store: Arc<dyn Store>,
    media: &MediaStore,
    specs: &[VariantSpec],
    opts: Options,
) -> anyhow::Result<Summary> {
    let existing = store
        .read()
        .content()
        .count(&ContentQuery::default())
        .await?;
    if existing > 0 && !opts.force {
        bail!(
            "the site already has {existing} content item(s); refusing to seed. \
             Run `ferrite seed demo --force` to add another demo batch"
        );
    }
    let rules = permalink_rules(store.as_ref()).await?;
    // The blobs go first, outside the transaction: they are content-addressed, and the
    // files of a run that fails are left for the artifacts GC.
    let image = store_image(media, specs).await?;

    let batch = DemoBatch::begin(store.clone(), Arc::new(rules)).await?;
    match write_rows(&batch, image, &opts).await {
        Ok(summary) => {
            batch.commit().await?;
            Ok(summary)
        }
        Err(e) => {
            if let Err(rollback) = batch.rollback().await {
                tracing::warn!(error = %rollback, "rollback of the demo batch failed");
            }
            Err(e)
        }
    }
}

/// Every row of the demo, through `batch`.
async fn write_rows(
    batch: &DemoBatch,
    image: StoredImage,
    opts: &Options,
) -> anyhow::Result<Summary> {
    let admin = batch
        .user(&NewUser {
            id: UserId::new(),
            login: ADMIN_LOGIN.into(),
            email: "admin@example.com".into(),
            display_name: "Site Admin".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: true,
        })
        .await?;
    let actor = Actor::new(admin.id, admin.role);

    let mut terms = Vec::new();
    for (tax, slug, name) in demo::TERMS {
        terms.push(
            batch
                .term(tax, slug, name)
                .await
                .with_context(|| format!("term {tax}/{slug}"))?,
        );
    }
    let term_id = |slug: &str| -> anyhow::Result<TermId> {
        terms
            .iter()
            .find(|t| t.slug == slug)
            .map(|t| t.id)
            .with_context(|| format!("demo term {slug} missing"))
    };

    let sample = batch
        .media_item(
            &actor,
            ContentDraft {
                kind: "media".into(),
                title: demo::IMAGE_TITLE.into(),
                slug: Some("sample-gradient".into()),
                parent_id: None,
                excerpt: Some(String::new()),
                body: json!({ "type": "doc", "content": [] }),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
            &image.artifacts,
            image.row,
        )
        .await
        .context("the sample image")?;
    let image = sample.id;

    let now = Timestamp::now().as_micros();
    let mut posts = 0;
    for (i, p) in demo::POSTS.iter().enumerate() {
        let mut ids = vec![term_id(p.category)?];
        for tag in p.tags {
            ids.push(term_id(tag)?);
        }
        let fields = if p.featured {
            json!({ "featured_image": image.to_string() })
        } else {
            json!({})
        };
        let draft = ContentDraft {
            kind: "post".into(),
            title: p.title.into(),
            slug: Some(p.slug.into()),
            parent_id: None,
            excerpt: None,
            body: demo::post_body(p),
            fields,
            menu_order: 0,
            terms: ids,
            seo: None,
        };
        // Newest first, about nine days apart, at varied times of day.
        let age = i64::try_from(i)? * 9 * DAY_MICROS + i64::try_from(i % 4)? * 3_600_000_000;
        let published_at = Timestamp::from_micros(now - DAY_MICROS / 24 - age)?;
        batch
            .publish(&actor, draft, Some(published_at))
            .await
            .with_context(|| format!("post {:?}", p.title))?;
        posts += 1;
    }
    #[cfg(feature = "test-seams")]
    if opts.fail_after_posts {
        bail!("test seam: the demo batch fails after the posts");
    }
    #[cfg(not(feature = "test-seams"))]
    let _ = opts;

    let about = batch
        .publish(&actor, demo::page("About", "about", None, 0), None)
        .await
        .context("page About")?;
    let team = batch
        .publish(&actor, demo::page("Team", "team", Some(about.id), 0), None)
        .await
        .context("page Team")?;
    let contact = batch
        .publish(&actor, demo::page("Contact", "contact", None, 1), None)
        .await
        .context("page Contact")?;

    let news = term_id("news")?;
    batch
        .menu(&Menu {
            name: "primary".into(),
            label: "Primary navigation".into(),
            items: vec![
                url_item(0, "Home", "/"),
                item(1, "About", MenuItemKind::Item, about.id.as_uuid()),
                item(2, "News", MenuItemKind::Term, news.as_uuid()),
                item(3, "Contact", MenuItemKind::Item, contact.id.as_uuid()),
            ],
        })
        .await
        .context("primary menu")?;
    batch
        .menu(&Menu {
            name: "footer".into(),
            label: "Footer".into(),
            items: vec![
                item(0, "Team", MenuItemKind::Item, team.id.as_uuid()),
                item(1, "Contact", MenuItemKind::Item, contact.id.as_uuid()),
                url_item(2, "RSS feed", "/feed/"),
            ],
        })
        .await
        .context("footer menu")?;

    batch
        .option(&actor, "site.title", &json!(demo::SITE_TITLE), true)
        .await?;
    batch
        .option(
            &actor,
            "site.description",
            &json!(demo::SITE_DESCRIPTION),
            true,
        )
        .await?;
    Ok(Summary {
        posts,
        pages: 3,
        terms: terms.len(),
    })
}

/// The sample image as stored: its artifact rows and `media` row, to be written with the
/// media item.
struct StoredImage {
    artifacts: Vec<Artifact>,
    row: Media,
}

/// Generates the sample PNG and stores its blobs (original and variants).
async fn store_image(media: &MediaStore, specs: &[VariantSpec]) -> anyhow::Result<StoredImage> {
    let png = demo::gradient_png()?;
    let specs = specs.to_vec();
    let source = png.clone();
    let processed = tokio::task::spawn_blocking(move || process_image(&source, &specs))
        .await
        .context("image task")?
        .context("cannot process the sample image")?;
    let original = media
        .put_artifact(Bytes::from(png))
        .await
        .context("cannot store the sample image")?;
    let mut artifacts = vec![artifact(&original, ArtifactKind::Media)?];
    let mut variants = serde_json::Map::new();
    for v in &processed.variants {
        let stored = media
            .put_artifact(v.data.clone())
            .await
            .context("cannot store an image variant")?;
        variants.insert(
            v.name.clone(),
            json!({ "hash": stored.hash.as_str(), "w": v.width, "h": v.height, "mime": v.mime }),
        );
        artifacts.push(artifact(&stored, ArtifactKind::Variant)?);
    }
    let row = Media {
        // Set by the batch once the item exists.
        item_id: ContentId::new(),
        artifact_hash: original.hash.clone(),
        mime: original.mime.clone(),
        bytes: i64::try_from(original.bytes)?,
        width: Some(i32::try_from(processed.width)?),
        height: Some(i32::try_from(processed.height)?),
        alt: demo::IMAGE_ALT.into(),
        original_name: demo::IMAGE_NAME.into(),
        variants: Value::Object(variants),
        sanitized: false,
    };
    Ok(StoredImage { artifacts, row })
}

fn artifact(r: &ferrite_media::ArtifactRef, kind: ArtifactKind) -> anyhow::Result<Artifact> {
    Ok(Artifact {
        hash: r.hash.clone(),
        kind,
        bytes: i64::try_from(r.bytes)?,
        mime: r.mime.clone(),
        created_at: Timestamp::now(),
    })
}

fn item(position: i32, label: &str, kind: MenuItemKind, target: uuid::Uuid) -> MenuItem {
    MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position,
        kind,
        target_id: Some(target),
        url: None,
        label: label.into(),
    }
}

fn url_item(position: i32, label: &str, url: &str) -> MenuItem {
    MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position,
        kind: MenuItemKind::Url,
        target_id: None,
        url: Some(url.into()),
        label: label.into(),
    }
}
