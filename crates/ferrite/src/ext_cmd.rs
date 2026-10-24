//! `ferrite theme …` and `ferrite plugin …`: operator extension management (P26).
//!
//! A system path over [`ExtensionService`] and the install pipeline ([`Installer`]): no
//! actor, no audit. Like `user`, every subcommand loads the site's config, opens the
//! database (SQLite takes the data-folder lock, so these commands fail while `serve`
//! runs) and the media store, and runs the same startup migration check as `serve`.
//!
//! `plugin install` prints the permission report (high-risk lines start with `!`, and
//! an update lists what it adds) and then asks `install? [y/N]` on a terminal; without a
//! terminal `--yes` is required. What is approved is exactly the report that was
//! printed: the install passes that report's fingerprint to the pipeline. The data
//! folder lock is held while the question is open.
//!
//! Output never includes setting values; plugin settings are not touched here.

use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, bail};
use bytes::Bytes;
use clap::{Args, Subcommand};
use ferrite_core::{CoreError, ExtensionService};
use ferrite_domain::InstalledTheme;
use ferrite_http::extensions::{Inspected, Installer, MAX_PLUGIN_BYTES};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{HostConfig, PermissionItem, PermissionReport, Risk};
use ferrite_store::Store;
use ferrite_theme::Theme;

use crate::{
    binary_version,
    extensions::{install_theme_dir, record_plugin},
    load_config,
    open::{OpenMode, open_store, startup_check},
    serve::host_config,
    site::open_media,
};

/// The component's file extension.
const WASM_EXT: &str = "wasm";
/// The plugin manifest's file name.
const MANIFEST_FILE: &str = "ferrite-plugin.toml";

#[derive(Debug, Args)]
pub struct DataArg {
    /// The site's data folder (contains ferrite.toml).
    #[arg(long)]
    pub data: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum ThemeCommand {
    /// Install a theme folder as an artifact (a theme that does not compile is refused).
    Install {
        /// The theme folder (holds `theme.toml` and `templates/`).
        #[arg(long)]
        dir: PathBuf,
        /// Make it the active theme.
        #[arg(long)]
        activate: bool,
        #[command(flatten)]
        data: DataArg,
    },
    /// Make an installed (or embedded) theme the active one; refused if it does not load.
    Activate {
        name: String,
        #[command(flatten)]
        data: DataArg,
    },
    /// List themes: `<name>  <version>  <installed|embedded>  [active]`.
    List(DataArg),
}

#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// Check a plugin folder and print its permission report; nothing is stored.
    ///
    /// Exit code 1 when the manifest or the component is refused.
    Check {
        /// The plugin folder: `ferrite-plugin.toml` and one `*.wasm`.
        #[arg(long)]
        dir: PathBuf,
        #[command(flatten)]
        data: DataArg,
    },
    /// Install (or update) a plugin from a folder after showing its permission report.
    Install {
        /// The plugin folder: `ferrite-plugin.toml` and one `*.wasm`.
        #[arg(long)]
        dir: PathBuf,
        /// Enable it after installing.
        #[arg(long)]
        enable: bool,
        /// Approve the printed report without asking (required when stdin is not a
        /// terminal).
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        data: DataArg,
    },
    /// List plugins: `<name>  <version>  <enabled|disabled>  [<reason>]`.
    List(DataArg),
    /// Enable a plugin.
    Enable {
        name: String,
        #[command(flatten)]
        data: DataArg,
    },
    /// Disable a plugin.
    Disable {
        name: String,
        #[command(flatten)]
        data: DataArg,
    },
    /// Remove a plugin; its KV and settings are kept for 30 days unless --purge.
    Remove {
        name: String,
        /// Delete the plugin's KV and settings now.
        #[arg(long)]
        purge: bool,
        #[command(flatten)]
        data: DataArg,
    },
}

/// What a running command works on.
struct Site {
    store: Arc<dyn Store>,
    media: Arc<MediaStore>,
    config: ferrite_config::Config,
}

pub async fn run_theme(cmd: ThemeCommand) -> anyhow::Result<()> {
    match cmd {
        ThemeCommand::Install {
            dir,
            activate,
            data,
        } => {
            with_site(&data.data, async |site| {
                let row = install_theme_dir(&site.store, &site.media, &dir, activate).await?;
                println!(
                    "installed theme {} {}{}",
                    row.name,
                    row.version,
                    if row.active { " (active)" } else { "" }
                );
                Ok(())
            })
            .await
        }
        ThemeCommand::Activate { name, data } => {
            with_site(&data.data, async |site| activate_theme(&site, &name).await).await
        }
        ThemeCommand::List(data) => {
            with_site(&data.data, async |site| list_themes(&site).await).await
        }
    }
}

pub async fn run_plugin(cmd: PluginCommand) -> anyhow::Result<()> {
    match cmd {
        PluginCommand::Check { dir, data } => {
            // The package is read before the database lock is taken.
            let package = read_package(&dir)?;
            with_site(&data.data, async |site| {
                let installer = installer(&site)?;
                let inspected = inspect(&installer, &package).await?;
                print_report(&inspected);
                println!("ok: nothing was stored");
                Ok(())
            })
            .await
        }
        PluginCommand::Install {
            dir,
            enable,
            yes,
            data,
        } => {
            let package = read_package(&dir)?;
            with_site(&data.data, async |site| {
                let installer = installer(&site)?;
                let inspected = inspect(&installer, &package).await?;
                print_report(&inspected);
                if !yes && !confirm()? {
                    bail!("aborted; nothing was installed");
                }
                let row = record_plugin(&site.store, &installer, inspected, enable).await?;
                println!(
                    "installed plugin {} {} ({})",
                    row.name,
                    row.version,
                    if row.enabled { "enabled" } else { "disabled" }
                );
                Ok(())
            })
            .await
        }
        PluginCommand::List(data) => with_site(&data.data, async |site| list_plugins(&site).await).await,
        PluginCommand::Enable { name, data } => {
            with_site(&data.data, async |site| {
                set_enabled(&site, &name, true).await
            })
            .await
        }
        PluginCommand::Disable { name, data } => {
            with_site(&data.data, async |site| {
                set_enabled(&site, &name, false).await
            })
            .await
        }
        PluginCommand::Remove { name, purge, data } => {
            with_site(&data.data, async |site| {
                let row = ExtensionService::new(site.store.clone())
                    .remove_plugin(&name, purge)
                    .await
                    .map_err(|e| not_found(e, "plugin", &name))?;
                if purge {
                    println!("removed plugin {} and purged its data", row.name);
                } else {
                    println!(
                        "removed plugin {}; its KV and settings are kept for 30 days (--purge deletes them now)",
                        row.name
                    );
                }
                Ok(())
            })
            .await
        }
    }
}

/// Loads the config, opens the store and media and runs the startup check, then `f`;
/// the store is closed afterwards either way.
async fn with_site(
    data: &Path,
    f: impl AsyncFnOnce(Site) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let config = load_config(data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let done = async {
        startup_check(store.as_ref(), &binary_version()?, &config.database).await?;
        let media = Arc::new(open_media(&config)?);
        f(Site {
            store: store.clone(),
            media,
            config,
        })
        .await
    }
    .await;
    store.close().await;
    done
}

fn installer(site: &Site) -> anyhow::Result<Installer> {
    Installer::new(
        site.store.clone(),
        site.media.clone(),
        host_config(&site.config.plugins),
    )
    .context("cannot start a plugin host")
}

// ---- themes --------------------------------------------------------------------------

async fn activate_theme(site: &Site, name: &str) -> anyhow::Result<()> {
    // Only themes are staged here: the scratch plugin host's limits do not matter.
    let installer = Installer::new(
        site.store.clone(),
        site.media.clone(),
        HostConfig::default(),
    )
    .context("cannot start the installer")?;
    let service = ExtensionService::new(site.store.clone());
    match site.store.read().themes().get(name).await? {
        Some(row) => {
            // As the API: the row changes only once its files load, and only if it still
            // names the bundle just loaded.
            installer.theme_from_artifact(&row).await.with_context(|| {
                format!("theme {name:?} does not load; the active theme is unchanged")
            })?;
            service
                .activate_theme(name, &row.artifact_hash)
                .await
                .with_context(|| format!("cannot activate theme {name:?}"))?;
        }
        None if Theme::embedded_names().iter().any(|n| n == name) => {
            let (artifact, row) = installer
                .stage_embedded(name)
                .await
                .with_context(|| format!("embedded theme {name:?} does not load"))?;
            service
                .install_theme(&artifact, &row, true)
                .await
                .with_context(|| format!("cannot activate theme {name:?}"))?;
        }
        None => bail!("theme not found: {name:?}"),
    }
    println!("activated theme {name}");
    Ok(())
}

async fn list_themes(site: &Site) -> anyhow::Result<()> {
    let rows = site.store.read().themes().list().await?;
    let any_active = rows.iter().any(|r| r.active);
    for row in &rows {
        print_theme(row);
    }
    for name in Theme::embedded_names() {
        if rows.iter().any(|r| r.name == name) {
            continue;
        }
        let theme = Theme::load_embedded(&name)
            .with_context(|| format!("embedded theme {name:?} does not load"))?;
        // With no active row the node serves the embedded default.
        let active = !any_active && name == ferrite_theme::DEFAULT_THEME;
        println!(
            "{}  {}  embedded{}",
            name,
            theme.manifest().version,
            if active { "  active" } else { "" }
        );
    }
    Ok(())
}

fn print_theme(row: &InstalledTheme) {
    println!(
        "{}  {}  installed{}",
        row.name,
        row.version,
        if row.active { "  active" } else { "" }
    );
}

// ---- plugins -------------------------------------------------------------------------

/// A plugin folder's two files.
struct Package {
    manifest: String,
    wasm: Vec<u8>,
}

/// Reads `ferrite-plugin.toml` and the one `*.wasm` of `dir` (the size is checked before
/// the component is read).
fn read_package(dir: &Path) -> anyhow::Result<Package> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let manifest = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let mut components = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let path = entry
            .with_context(|| format!("cannot read {}", dir.display()))?
            .path();
        if path.extension().and_then(|e| e.to_str()) == Some(WASM_EXT) && path.is_file() {
            components.push(path);
        }
    }
    let wasm_path = match components.as_slice() {
        [one] => one,
        [] => bail!("{} holds no *.{WASM_EXT} component", dir.display()),
        _ => bail!(
            "{} holds more than one *.{WASM_EXT} component; keep exactly one",
            dir.display()
        ),
    };
    let len = std::fs::metadata(wasm_path)?.len();
    if len > MAX_PLUGIN_BYTES {
        bail!(
            "{} is larger than {MAX_PLUGIN_BYTES} bytes",
            wasm_path.display()
        );
    }
    let wasm =
        std::fs::read(wasm_path).with_context(|| format!("cannot read {}", wasm_path.display()))?;
    Ok(Package { manifest, wasm })
}

async fn inspect(installer: &Installer, package: &Package) -> anyhow::Result<Inspected> {
    installer
        .inspect_plugin(&package.manifest, Bytes::copy_from_slice(&package.wasm))
        .await
        .context("the plugin is refused")
}

/// Prints the report: one line per capability (`!` marks high risk), what an update
/// adds to the installed version's grant, the limits and the fingerprint approved.
fn print_report(inspected: &Inspected) {
    let report = inspected.report();
    let kind = if inspected.is_update() {
        "update"
    } else {
        "new"
    };
    println!("plugin {} {} ({kind})", report.plugin, report.version);
    println!("component: {}", report.artifact);
    println!("permissions:");
    if report.items.is_empty() {
        println!("  (none)");
    }
    for item in &report.items {
        println!("{}", item_line(item));
    }
    if inspected.is_update() {
        if report.added.is_empty() {
            println!("update asks for nothing beyond the installed version's grant");
        } else {
            println!("this update WIDENS the installed grant, adding:");
            for item in &report.added {
                println!("{}", item_line(item));
            }
        }
    }
    print_limits(report);
    println!("fingerprint: {}", report.fingerprint);
}

fn item_line(item: &PermissionItem) -> String {
    let mark = match item.risk {
        Risk::High => '!',
        Risk::Medium => '~',
        Risk::Low => ' ',
    };
    let detail = if item.detail.is_empty() {
        String::new()
    } else {
        format!(" [{}]", item.detail.join(", "))
    };
    format!("{mark} {}: {}{detail}", item.key, item.text)
}

fn print_limits(report: &PermissionReport) {
    if report.limits.is_empty() {
        return;
    }
    println!("limits:");
    for l in &report.limits {
        println!(
            "  {}: requested {} (default {}, ceiling {}){}",
            l.name,
            l.requested,
            l.default,
            l.ceiling,
            if l.above_default {
                " above default"
            } else {
                ""
            }
        );
    }
}

/// Asks `install? [y/N]` on a terminal; without one `--yes` is required.
fn confirm() -> anyhow::Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("`plugin install` needs --yes when stdin is not a terminal; nothing was installed");
    }
    eprint!("install? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    stdin.read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

async fn list_plugins(site: &Site) -> anyhow::Result<()> {
    for row in site.store.read().plugins().list().await? {
        let state = if row.enabled { "enabled" } else { "disabled" };
        match &row.disabled_reason {
            Some(reason) => println!("{}  {}  {}  {}", row.name, row.version, state, reason),
            None => println!("{}  {}  {}", row.name, row.version, state),
        }
    }
    Ok(())
}

async fn set_enabled(site: &Site, name: &str, enabled: bool) -> anyhow::Result<()> {
    ExtensionService::new(site.store.clone())
        .set_plugin_enabled(name, enabled)
        .await
        .map_err(|e| not_found(e, "plugin", name))?;
    println!(
        "{} plugin {name}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

/// `NotFound` names the extension; other errors keep the core message.
fn not_found(e: CoreError, what: &str, name: &str) -> anyhow::Error {
    match e {
        CoreError::NotFound => anyhow::anyhow!("{what} not found: {name:?}"),
        other => other.into(),
    }
}
