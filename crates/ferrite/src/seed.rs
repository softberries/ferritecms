//! `ferrite seed demo`: demo content for a new site.
//!
//! Two profiles (`--profile`, M4-3b P61): `blog` (the default, described below) and
//! `business`, which writes pages Home (the static front page, `reading.front_page`,
//! with a featured image), Services, Consulting and Support (children of Home with
//! `menu_order` 1–3 and card images), About and Contact, three posts in category News,
//! menus `primary` (Home, About, News → `/post/`, Contact) and `footer`, and
//! `site.title` "Larkspur Studio". It activates no theme; it prints the
//! `ferrite theme activate business` line to run next, and makes its image variants
//! with the embedded `business` theme's sizes. Both profiles share the rules below.
//!
//! The `blog` profile writes, through the core services where they exist: an `admin`
//! user (no password, `must_reset`, so it logs in only after
//! `ferrite user reset-password`), categories News
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

mod business;
mod demo;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, bail};
use bytes::Bytes;
use clap::{Args, Subcommand, ValueEnum};
use ferrite_core::{Actor, ContentDraft, seed::DemoBatch};
use ferrite_domain::{
    Artifact, ArtifactKind, ContentId, ContentQuery, Media, Menu, MenuItem, MenuItemId,
    MenuItemKind, NewUser, Role, TermId, Timestamp, UserId, UserStatus,
};
use ferrite_http::media::variant_specs;
use ferrite_media::{MediaStore, VariantSpec, process_image};
use ferrite_store::Store;
use ferrite_theme::Theme;
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
    /// What kind of site to seed.
    #[arg(long, value_enum, default_value_t = Profile::Blog)]
    pub profile: Profile,
    /// Test seam: fail after the posts, to show that nothing stays written.
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub fail_after_posts: bool,
}

/// The demo content set (`--profile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Profile {
    /// Posts, pages, terms and menus of a blog (the default).
    Blog,
    /// A small-business site with a static front page, for the `business` theme.
    Business,
}

pub async fn run(cmd: SeedCommand) -> anyhow::Result<()> {
    let SeedCommand::Demo(args) = cmd;
    let config = load_config(&args.data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let seeded = async {
        startup_check(
            store.as_ref(),
            &binary_version()?,
            &config.database,
            &config.data_dir,
        )
        .await?;
        let media = open_media(&config)?;
        let theme = load_theme(
            store.as_ref(),
            &media,
            &config.data_dir,
            config.effective_cluster_mode(),
        )
        .await?
        .theme;
        // The sizes a `media.variants` job would make on a node with this theme; the
        // business profile's images are made for the theme it is seeded for.
        let specs = match args.profile {
            Profile::Blog => variant_specs(&theme),
            Profile::Business => variant_specs(
                &Theme::load_embedded("business").context("the embedded business theme")?,
            ),
        };
        seed_demo(
            store.clone(),
            &media,
            &specs,
            args.profile,
            args.force,
            #[cfg(feature = "test-seams")]
            args.fail_after_posts,
        )
        .await
    }
    .await;
    store.close().await;
    let s = seeded?;
    match args.profile {
        Profile::Blog => println!(
            "Seeded demo content: {} posts, {} pages, {} terms, 2 menus, 1 image (admin user: {ADMIN_LOGIN}).",
            s.posts, s.pages, s.terms
        ),
        Profile::Business => {
            println!(
                "Seeded business demo content: {} posts, {} pages, {} term, 2 menus, {} images (admin user: {ADMIN_LOGIN}).",
                s.posts, s.pages, s.terms, s.images
            );
            println!(
                "next: ferrite theme activate business --data {}",
                args.data.display()
            );
        }
    }
    Ok(())
}

/// What one run wrote.
struct Summary {
    posts: usize,
    pages: usize,
    terms: usize,
    images: usize,
}

async fn seed_demo(
    store: Arc<dyn Store>,
    media: &MediaStore,
    specs: &[VariantSpec],
    profile: Profile,
    force: bool,
    #[cfg(feature = "test-seams")] fail_after_posts: bool,
) -> anyhow::Result<Summary> {
    let existing = store
        .read()
        .content()
        .count(&ContentQuery::default())
        .await?;
    if existing > 0 && !force {
        bail!(
            "the site already has {existing} content item(s); refusing to seed. \
             Run `ferrite seed demo --force` to add another demo batch"
        );
    }
    let rules = permalink_rules(store.as_ref()).await?;
    // The blobs go first, outside the transaction: they are content-addressed, and the
    // files of a run that fails are left for the artifacts GC.
    let mut images = Vec::new();
    match profile {
        Profile::Blog => images.push(
            store_image(
                media,
                specs,
                demo::gradient_png()?,
                demo::IMAGE_ALT,
                demo::IMAGE_NAME,
            )
            .await?,
        ),
        Profile::Business => {
            for i in &business::IMAGES {
                let png = demo::gradient_png_of(i.width, i.height, i.from, i.to)?;
                images.push(store_image(media, specs, png, i.alt, i.name).await?);
            }
        }
    }

    let batch = DemoBatch::begin(store.clone(), Arc::new(rules)).await?;
    let written = match profile {
        Profile::Blog => {
            write_rows(
                &batch,
                images.remove(0),
                #[cfg(feature = "test-seams")]
                fail_after_posts,
            )
            .await
        }
        Profile::Business => {
            write_business_rows(
                &batch,
                images,
                #[cfg(feature = "test-seams")]
                fail_after_posts,
            )
            .await
        }
    };
    match written {
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
    #[cfg(feature = "test-seams")] fail_after_posts: bool,
) -> anyhow::Result<Summary> {
    let actor = admin_actor(batch).await?;

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

    let image = media_item(batch, &actor, demo::IMAGE_TITLE, "sample-gradient", image).await?;

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
    if fail_after_posts {
        bail!("test seam: the demo batch fails after the posts");
    }

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
        images: 1,
    })
}

/// The admin every profile writes as.
async fn admin_actor(batch: &DemoBatch) -> anyhow::Result<Actor> {
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
    Ok(Actor::new(admin.id, admin.role))
}

/// A media item for `image` with `title` and `slug`; returns its id.
async fn media_item(
    batch: &DemoBatch,
    actor: &Actor,
    title: &str,
    slug: &str,
    image: StoredImage,
) -> anyhow::Result<ContentId> {
    let item = batch
        .media_item(
            actor,
            ContentDraft {
                kind: "media".into(),
                title: title.into(),
                slug: Some(slug.into()),
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
        .with_context(|| format!("image {title:?}"))?;
    Ok(item.id)
}

/// Every row of the `business` profile, through `batch`. `images` are the stored
/// [`business::IMAGES`], in order.
async fn write_business_rows(
    batch: &DemoBatch,
    images: Vec<StoredImage>,
    #[cfg(feature = "test-seams")] fail_after_posts: bool,
) -> anyhow::Result<Summary> {
    let actor = admin_actor(batch).await?;
    let (tax, slug, name) = business::NEWS;
    let news = batch
        .term(tax, slug, name)
        .await
        .with_context(|| format!("term {tax}/{slug}"))?;

    let mut ids = Vec::new();
    for (spec, image) in business::IMAGES.iter().zip(images) {
        ids.push(media_item(batch, &actor, spec.title, spec.slug, image).await?);
    }
    let (hero, cards) = ids.split_first().context("the hero image")?;

    let now = Timestamp::now().as_micros();
    let mut posts = 0;
    for (i, (title, slug, lead)) in business::POSTS.iter().enumerate() {
        let draft = ContentDraft {
            kind: "post".into(),
            title: (*title).into(),
            slug: Some((*slug).into()),
            parent_id: None,
            excerpt: Some((*lead).into()),
            body: business::post_body(lead),
            fields: json!({}),
            menu_order: 0,
            terms: vec![news.id],
            seo: None,
        };
        // Newest first, about two weeks apart.
        let age = i64::try_from(i)? * 14 * DAY_MICROS;
        let published_at = Timestamp::from_micros(now - DAY_MICROS / 24 - age)?;
        batch
            .publish(&actor, draft, Some(published_at))
            .await
            .with_context(|| format!("post {title:?}"))?;
        posts += 1;
    }
    #[cfg(feature = "test-seams")]
    if fail_after_posts {
        bail!("test seam: the demo batch fails after the posts");
    }

    let home = publish_page(batch, &actor, &business::HOME, None, 0, Some(*hero)).await?;
    let mut children = Vec::new();
    for ((p, order), card) in business::CHILDREN.iter().zip(1..).zip(cards) {
        children.push(publish_page(batch, &actor, p, Some(home.id), order, Some(*card)).await?);
    }
    let about = publish_page(batch, &actor, &business::ABOUT, None, 1, None).await?;
    let contact = publish_page(batch, &actor, &business::CONTACT, None, 2, None).await?;

    batch
        .menu(&Menu {
            name: "primary".into(),
            label: "Primary navigation".into(),
            items: vec![
                url_item(0, "Home", "/"),
                item(1, "About", MenuItemKind::Item, about.id.as_uuid()),
                url_item(2, "News", "/post/"),
                item(3, "Contact", MenuItemKind::Item, contact.id.as_uuid()),
            ],
        })
        .await
        .context("primary menu")?;
    let services = children.first().context("the services page")?;
    batch
        .menu(&Menu {
            name: "footer".into(),
            label: "Footer".into(),
            items: vec![
                item(0, "Services", MenuItemKind::Item, services.id.as_uuid()),
                item(1, "About", MenuItemKind::Item, about.id.as_uuid()),
                item(2, "Contact", MenuItemKind::Item, contact.id.as_uuid()),
                url_item(3, "RSS feed", "/feed/"),
            ],
        })
        .await
        .context("footer menu")?;

    batch
        .option(&actor, "site.title", &json!(business::SITE_TITLE), true)
        .await?;
    batch
        .option(
            &actor,
            "site.description",
            &json!(business::SITE_DESCRIPTION),
            true,
        )
        .await?;
    batch
        .option(
            &actor,
            "reading.front_page",
            &json!(home.id.to_string()),
            true,
        )
        .await?;
    Ok(Summary {
        posts,
        pages: 3 + children.len(),
        terms: 1,
        images: business::IMAGES.len(),
    })
}

/// Publishes the business page `p`; returns its id.
async fn publish_page(
    batch: &DemoBatch,
    actor: &Actor,
    p: &business::DemoPage,
    parent: Option<ContentId>,
    menu_order: i32,
    featured: Option<ContentId>,
) -> anyhow::Result<ferrite_domain::ContentItem> {
    batch
        .publish(actor, business::page(p, parent, menu_order, featured), None)
        .await
        .with_context(|| format!("page {:?}", p.title))
}

/// The sample image as stored: its artifact rows and a maker of its `media` row (it
/// needs the media item's id), to be written with the media item.
struct StoredImage {
    artifacts: Vec<Artifact>,
    row: Box<dyn FnOnce(ContentId) -> Media + Send>,
}

/// Stores the blobs of the generated image `png` (original and variants) and returns
/// its artifacts and a maker of its `media` row with `alt` and file `name`.
async fn store_image(
    media: &MediaStore,
    specs: &[VariantSpec],
    png: Vec<u8>,
    alt: &'static str,
    name: &'static str,
) -> anyhow::Result<StoredImage> {
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
    let hash = original.hash.clone();
    let mime = original.mime.clone();
    let bytes = i64::try_from(original.bytes)?;
    let width = Some(i32::try_from(processed.width)?);
    let height = Some(i32::try_from(processed.height)?);
    let row = move |item_id| Media {
        item_id,
        artifact_hash: hash,
        mime,
        bytes,
        width,
        height,
        alt: alt.into(),
        original_name: name.into(),
        variants: Value::Object(variants),
        sanitized: false,
    };
    Ok(StoredImage {
        artifacts,
        row: Box::new(row),
    })
}

fn artifact(r: &ferrite_media::ArtifactRef, kind: ArtifactKind) -> anyhow::Result<Artifact> {
    Ok(Artifact {
        hash: r.hash.clone(),
        kind,
        bytes: i64::try_from(r.bytes)?,
        mime: r.mime.clone(),
        created_at: Timestamp::now(),
        orphaned_at: None,
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
