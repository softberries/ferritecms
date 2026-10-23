//! `ferrite seed demo`: demo content for a new site.
//!
//! Writes, through the core services where they exist: an `admin` user (no password,
//! `must_reset`; login arrives in M2), categories News and Guides, tags rust and cms,
//! 12 published posts with block bodies dated over the past months, three pages
//! (About, About/Team, Contact), a `primary` and a `footer` menu, one sample image
//! (a PNG generated here, stored with its variants: the core sizes plus the active
//! theme's) used as the featured image of some posts, and the `site.title` /
//! `site.description` options.
//!
//! Idempotency: if the site has any content item, `seed demo` refuses (exit 1) unless
//! `--force` is given. With `--force` it adds another batch: the user, terms and image
//! file are reused when they exist, posts and pages get fresh slugs (`-2`, `-3`, … as
//! the content service allocates them), and both menus and the two options are
//! replaced to point at the new batch.
//!
//! **Not atomic.** Each step is its own transaction (one per service call), so a run
//! that fails part-way (store error, media backend down) leaves what it already
//! wrote: possibly the user, terms, the image (as a draft media item if its media row
//! failed, or stored files without a row) and some posts. A plain re-run then refuses
//! because content exists; `--force` completes a demo by adding a **whole new batch**
//! next to the partial one (reusing user, terms and stored image files; new `-N`
//! slugs). It does not resume or clean up the partial batch: delete it by hand, or
//! start from a fresh data folder, if it matters.
//!
//! There is no media service yet (M2), so the image's `media` row and its `artifacts`
//! rows are written here, in one transaction that also emits `post:<media id>`
//! (ruling R11); the media item itself is created and published through
//! [`ContentService`].

mod demo;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, bail};
use bytes::Bytes;
use clap::{Args, Subcommand};
use ferrite_core::{
    Actor, ContentDraft, ContentService, MenuService, OptionService, TaxonomyService, events,
};
use ferrite_domain::{
    Artifact, ArtifactKind, ContentId, ContentItem, ContentQuery, Media, Menu, MenuItem,
    MenuItemId, MenuItemKind, NewUser, Role, Term, TermId, Timestamp, User, UserId, UserStatus,
};
use ferrite_media::{MediaStore, VariantSpec, default_variants, process_image};
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
        let mut specs = default_variants();
        for (name, size) in &theme.manifest().images {
            // A height without crop is a bounding box, which the processor rejects.
            if (size.height.is_some() && !size.crop) || specs.iter().any(|s| &s.name == name) {
                continue;
            }
            specs.push(VariantSpec {
                name: name.clone(),
                width: size.width,
                height: size.height,
                crop: size.crop,
            });
        }
        seed_demo(store.clone(), &media, &specs, args.force).await
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

async fn seed_demo(
    store: Arc<dyn Store>,
    media: &MediaStore,
    specs: &[VariantSpec],
    force: bool,
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
    let admin = admin_user(store.as_ref()).await?;
    let actor = Actor {
        user: admin.id,
        role: admin.role,
    };
    let rules = permalink_rules(store.as_ref()).await?;
    let content = ContentService::new(store.clone(), Arc::new(rules));

    let taxonomy = TaxonomyService::new(store.clone());
    let mut terms = Vec::new();
    for (tax, slug, name) in demo::TERMS {
        terms.push(term(store.as_ref(), &taxonomy, &actor, tax, slug, name).await?);
    }
    let term_id = |slug: &str| -> anyhow::Result<TermId> {
        terms
            .iter()
            .find(|t| t.slug == slug)
            .map(|t| t.id)
            .with_context(|| format!("demo term {slug} missing"))
    };

    let image = sample_image(store.as_ref(), media, &content, &actor, specs).await?;

    let now = Timestamp::now().as_micros();
    let mut posts = Vec::new();
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
        };
        // Newest first, about nine days apart, at varied times of day.
        let age = i64::try_from(i)? * 9 * DAY_MICROS + i64::try_from(i % 4)? * 3_600_000_000;
        let published_at = Timestamp::from_micros(now - DAY_MICROS / 24 - age)?;
        posts.push(publish(store.as_ref(), &content, &actor, draft, Some(published_at)).await?);
    }

    let about = publish(
        store.as_ref(),
        &content,
        &actor,
        demo::page("About", "about", None, 0),
        None,
    )
    .await?;
    let team = publish(
        store.as_ref(),
        &content,
        &actor,
        demo::page("Team", "team", Some(about.id), 0),
        None,
    )
    .await?;
    let contact = publish(
        store.as_ref(),
        &content,
        &actor,
        demo::page("Contact", "contact", None, 1),
        None,
    )
    .await?;

    let menus = MenuService::new(store.clone());
    let news = term_id("news")?;
    menus
        .put(
            &actor,
            &Menu {
                name: "primary".into(),
                label: "Primary navigation".into(),
                items: vec![
                    url_item(0, "Home", "/"),
                    item(1, "About", MenuItemKind::Item, about.id.as_uuid()),
                    item(2, "News", MenuItemKind::Term, news.as_uuid()),
                    item(3, "Contact", MenuItemKind::Item, contact.id.as_uuid()),
                ],
            },
        )
        .await
        .context("primary menu")?;
    menus
        .put(
            &actor,
            &Menu {
                name: "footer".into(),
                label: "Footer".into(),
                items: vec![
                    item(0, "Team", MenuItemKind::Item, team.id.as_uuid()),
                    item(1, "Contact", MenuItemKind::Item, contact.id.as_uuid()),
                    url_item(2, "RSS feed", "/feed/"),
                ],
            },
        )
        .await
        .context("footer menu")?;

    let options = OptionService::load(store.clone()).await?;
    options
        .set(&actor, "site.title", &json!(demo::SITE_TITLE), true)
        .await?;
    options
        .set(
            &actor,
            "site.description",
            &json!(demo::SITE_DESCRIPTION),
            true,
        )
        .await?;
    Ok(Summary {
        posts: posts.len(),
        pages: 3,
        terms: terms.len(),
    })
}

/// The `admin` user, created (no password, must reset) if missing.
async fn admin_user(store: &dyn Store) -> anyhow::Result<User> {
    if let Some(u) = store.read().users().get_by_login(ADMIN_LOGIN).await? {
        return Ok(u);
    }
    // There is no user service yet (M2); a new user is referenced by nothing cached,
    // so no event is needed.
    let tx = store.begin().await?;
    let user = tx
        .users()
        .insert(&NewUser {
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
    tx.commit().await?;
    Ok(user)
}

/// The term `slug` of `tax`, created if missing.
async fn term(
    store: &dyn Store,
    svc: &TaxonomyService,
    actor: &Actor,
    tax: &str,
    slug: &str,
    name: &str,
) -> anyhow::Result<Term> {
    if let Some(t) = store.read().terms().get_by_slug(tax, slug).await? {
        return Ok(t);
    }
    svc.create_term(actor, tax, name, Some(slug.into()), None, "")
        .await
        .with_context(|| format!("term {tax}/{slug}"))
}

/// Creates `draft`, optionally backdates its first publish time, and publishes it.
async fn publish(
    store: &dyn Store,
    svc: &ContentService,
    actor: &Actor,
    draft: ContentDraft,
    published_at: Option<Timestamp>,
) -> anyhow::Result<ContentItem> {
    let title = draft.title.clone();
    let mut item = svc
        .create(actor, draft)
        .await
        .with_context(|| format!("create {title:?}"))?;
    if let Some(at) = published_at {
        // `publish` keeps an existing `published_at`; the service has no setter for it.
        let tx = store.begin().await?;
        let done = async {
            let mut current = tx.content().get(item.id).await?.context("item vanished")?;
            current.published_at = Some(at);
            let saved = tx.content().update(&current).await?;
            tx.events()
                .append(&events::invalidate([events::post_key(saved.id)]))
                .await?;
            anyhow::Ok(saved)
        }
        .await;
        item = match done {
            Ok(saved) => {
                tx.commit().await?;
                saved
            }
            Err(e) => {
                tx.rollback().await?;
                return Err(e);
            }
        };
    }
    svc.publish(actor, item.id, item.version)
        .await
        .with_context(|| format!("publish {title:?}"))
}

/// Generates the sample PNG, stores it and its variants, and publishes a media item
/// for it. Returns the media item's id.
async fn sample_image(
    store: &dyn Store,
    media: &MediaStore,
    svc: &ContentService,
    actor: &Actor,
    specs: &[VariantSpec],
) -> anyhow::Result<ContentId> {
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
    let item = svc
        .create(
            actor,
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
            },
        )
        .await
        .context("create the media item")?;
    let row = Media {
        item_id: item.id,
        artifact_hash: original.hash.clone(),
        mime: original.mime.clone(),
        bytes: i64::try_from(original.bytes)?,
        width: Some(i32::try_from(processed.width)?),
        height: Some(i32::try_from(processed.height)?),
        alt: demo::IMAGE_ALT.into(),
        original_name: demo::IMAGE_NAME.into(),
        variants: Value::Object(variants),
    };
    // The media row and its artifacts in one transaction with `post:<media id>` (R11).
    let tx = store.begin().await?;
    let done = async {
        for a in &artifacts {
            tx.artifacts().insert_if_absent(a).await?;
        }
        tx.media().insert(&row).await?;
        tx.events()
            .append(&events::invalidate([events::post_key(item.id)]))
            .await?;
        anyhow::Ok(())
    }
    .await;
    match done {
        Ok(()) => tx.commit().await?,
        Err(e) => {
            tx.rollback().await?;
            return Err(e.context("cannot write the media row"));
        }
    }
    let published = svc
        .publish(actor, item.id, item.version)
        .await
        .context("publish the media item")?;
    Ok(published.id)
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
