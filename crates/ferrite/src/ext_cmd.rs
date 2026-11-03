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
//!
//! **Trust (M6-2, design 14 §9).** A plugin folder may hold `ferrite-plugin.minisig` and
//! `ferrite-plugin.pub`; the report then starts with the publisher lines. `install NAME`,
//! `search` and `update` use the configured registry and always fetch it first. `keys`
//! lists, trusts and revokes publisher keys; `unbind` frees a name and keeps its data
//! (both operator only: the API has neither). `digest` prints the signing payload of a
//! folder and needs no data folder. Exit codes: 0 done, 1 error (refusals included), 2
//! only for `update --check` when updates are available.

use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use std::process::ExitCode;

use anyhow::{Context, bail};
use bytes::Bytes;
use clap::{Args, Subcommand};
use ferrite_core::{CoreError, ExtensionService, OwnerChange, SignerSource};
use ferrite_domain::InstalledTheme;
use ferrite_domain::plugin_trust::{
    InstallSource, KeyId, KeySource, MAX_PUBLISHER_KEY_BYTES, MAX_SIGNATURE_BYTES,
    PUBLISHER_KEY_FILE, PublisherKey, Revocation, SIGNATURE_FILE,
};
use ferrite_http::extensions::{
    BundleSignature, Inspected, Installer, MAX_PLUGIN_BYTES, update_for,
};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{HostConfig, PermissionItem, PermissionReport, Risk};
use ferrite_store::Store;
use ferrite_theme::{Theme, ThemeSource};

use crate::{
    binary_version,
    extensions::{install_theme_dir, record_plugin},
    load_config,
    open::{OpenMode, open_store, startup_check},
    serve::host_config,
    site::{open_media, sha256_hex},
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
    /// Pack a theme folder into the bundle `POST /api/v1/themes` takes (needs no data
    /// folder): the canonical ustar, hidden files left out. A theme that does not load is
    /// refused and nothing is written. Prints
    /// `packed <n> files, <bytes> bytes, sha256 <hex>`.
    Pack {
        /// The theme folder (holds `theme.toml` and `templates/`).
        #[arg(long)]
        dir: PathBuf,
        /// The bundle file to write (replaced if it exists).
        #[arg(long)]
        out: PathBuf,
    },
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
    /// Install (or update) a plugin from a folder, or `NAME[@VERSION]` from the
    /// configured registry (fetched first), after showing its permission report.
    Install {
        /// A registry plugin, `NAME` or `NAME@VERSION` (default: the newest this binary
        /// runs).
        #[arg(conflicts_with = "dir", required_unless_present = "dir")]
        name: Option<String>,
        /// The plugin folder: `ferrite-plugin.toml`, one `*.wasm`, and optionally
        /// `ferrite-plugin.minisig` and `ferrite-plugin.pub`.
        #[arg(long)]
        dir: Option<PathBuf>,
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
    /// Search the configured registry (fetched first): `<name>  <latest>  <publisher>
    /// <description>`. Exit 1 without a registry or when it cannot be fetched.
    Search {
        /// Matches name, description and publisher name; none lists every plugin.
        query: Option<String>,
        #[command(flatten)]
        data: DataArg,
    },
    /// Install the updates the registry offers for installed plugins (all, or `NAME…`).
    ///
    /// `--check` prints `<name>  <installed> -> <available>` and exits 2 when there are
    /// updates, 0 when there are none.
    Update {
        names: Vec<String>,
        /// Only list the updates.
        #[arg(long)]
        check: bool,
        /// Approve each printed report without asking.
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        data: DataArg,
    },
    /// Print the signing payload of a plugin folder (what `minisign -S -m` signs); needs
    /// no data folder.
    Digest {
        /// The plugin folder: `ferrite-plugin.toml` and one `*.wasm`.
        #[arg(long)]
        dir: PathBuf,
        /// Write it to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Publisher keys: list, trust (operator) and revoke.
    #[command(subcommand)]
    Keys(KeysCommand),
    /// Delete the owner binding of a plugin name and keep its data, so the next install
    /// binds it again (operator only; for a publisher that lost its key).
    Unbind {
        name: String,
        #[command(flatten)]
        data: DataArg,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// List the site's keys: `<key_id>  <source>  <label>  [revoked[, compromised]: <reason>]`.
    List(DataArg),
    /// Trust a minisign public key file as an operator key (clears a local revocation).
    Trust {
        /// A minisign public key file (or the bare base64 line).
        file: PathBuf,
        /// What the key is (shown in lists).
        #[arg(long)]
        label: String,
        #[command(flatten)]
        data: DataArg,
    },
    /// Revoke a key: installs and updates signed by it are refused; with
    /// `--compromised` its installed plugins are disabled.
    Revoke {
        /// 16 hex digits, as minisign prints it.
        key_id: String,
        #[arg(long)]
        reason: String,
        /// The key was stolen or leaked: disable its plugins.
        #[arg(long)]
        compromised: bool,
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
        ThemeCommand::Pack { dir, out } => pack_theme(&dir, &out),
    }
}

/// `ferrite theme pack`: no config, database or media store.
fn pack_theme(dir: &Path, out: &Path) -> anyhow::Result<()> {
    let (bundle, files) = ferrite_theme::pack_counted(&ThemeSource::Dir(dir.to_path_buf()))
        .with_context(|| format!("cannot pack {}", dir.display()))?;
    let bundle = Bytes::from(bundle);
    // What an upload does with it: unpacked and every template compiled.
    Theme::load(&ThemeSource::Bundle(bundle.clone()))
        .with_context(|| format!("{} does not load as a theme", dir.display()))?;
    std::fs::write(out, &bundle).with_context(|| format!("cannot write {}", out.display()))?;
    println!(
        "packed {files} files, {} bytes, sha256 {}",
        bundle.len(),
        sha256_hex(&bundle)
    );
    Ok(())
}

pub async fn run_plugin(cmd: PluginCommand) -> anyhow::Result<ExitCode> {
    let done = match cmd {
        PluginCommand::Check { dir, data } => {
            // The package is read before the database lock is taken.
            let package = read_package(&dir)?;
            with_site(&data.data, async |site| {
                let installer = installer(&site)?;
                let inspected = inspect(&installer, &package).await?;
                print_report(&inspected);
                refusal(&inspected)?;
                println!("ok: nothing was stored");
                Ok(())
            })
            .await
        }
        PluginCommand::Install {
            name,
            dir,
            enable,
            yes,
            data,
        } => {
            let package = dir.as_deref().map(read_package).transpose()?;
            with_site(&data.data, async |site| {
                let installer = installer(&site)?;
                let inspected = match (&package, &name) {
                    (Some(package), _) => inspect(&installer, package).await?,
                    (None, Some(spec)) => {
                        let (name, version) = match spec.split_once('@') {
                            Some((n, v)) => (n, Some(v)),
                            None => (spec.as_str(), None),
                        };
                        refresh(&installer).await?;
                        installer
                            .inspect_registry(name, version)
                            .await
                            .with_context(|| format!("cannot install {spec} from the registry"))?
                    }
                    (None, None) => bail!("give a registry plugin NAME or --dir"),
                };
                approve_and_record(&site, &installer, inspected, enable, yes).await
            })
            .await
        }
        PluginCommand::Search { query, data } => {
            with_site(&data.data, async |site| {
                let installer = installer(&site)?;
                refresh(&installer).await?;
                let view = installer.registry_view(query.as_deref()).await?;
                for p in view.plugins {
                    println!(
                        "{}  {}  {}  {}",
                        p.plugin.name,
                        p.latest.as_deref().unwrap_or("-"),
                        p.publisher_name,
                        one_line(&p.plugin.description)
                    );
                }
                Ok(())
            })
            .await
        }
        PluginCommand::Update {
            names,
            check,
            yes,
            data,
        } => {
            let mut updates_found = false;
            let done = with_site(&data.data, async |site| {
                let installer = installer(&site)?;
                refresh(&installer).await?;
                let updates = available_updates(&site, &installer, &names).await?;
                updates_found = !updates.is_empty();
                if check || updates.is_empty() {
                    for (name, installed, available) in &updates {
                        println!("{name}  {installed} -> {available}");
                    }
                    if updates.is_empty() {
                        println!("no updates");
                    }
                    return Ok(());
                }
                for (name, _, available) in updates {
                    let inspected = installer
                        .inspect_registry(&name, Some(&available))
                        .await
                        .with_context(|| format!("cannot update {name} to {available}"))?;
                    approve_and_record(&site, &installer, inspected, false, yes).await?;
                }
                Ok(())
            })
            .await;
            done?;
            return Ok(if check && updates_found {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            });
        }
        PluginCommand::Digest { dir, out } => {
            let package = read_package(&dir)?;
            let payload = ferrite_plugin_host::trust::signing_payload(
                package.manifest.as_bytes(),
                &package.wasm,
            );
            match out {
                Some(out) => std::fs::write(&out, payload)
                    .with_context(|| format!("cannot write {}", out.display()))?,
                None => print!("{payload}"),
            }
            Ok(())
        }
        PluginCommand::Keys(cmd) => run_keys(cmd).await,
        PluginCommand::Unbind { name, data } => {
            with_site(&data.data, async |site| {
                if ExtensionService::new(site.store.clone()).unbind(&name).await? {
                    eprintln!(
                        "warning: {name}'s data is kept but no longer bound to a publisher: the \
                         next install of {name}, signed or not, gets it"
                    );
                    println!("unbound plugin {name}");
                } else {
                    println!("plugin {name} had no binding");
                }
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
    };
    done.map(|()| ExitCode::SUCCESS)
}

async fn run_keys(cmd: KeysCommand) -> anyhow::Result<()> {
    match cmd {
        KeysCommand::List(data) => {
            with_site(&data.data, async |site| {
                for k in ExtensionService::new(site.store.clone())
                    .list_keys()
                    .await?
                {
                    let revoked = match &k.revoked {
                        Some(r) if r.compromised => {
                            format!("  [revoked, compromised: {}]", one_line(&r.reason))
                        }
                        Some(r) => format!("  [revoked: {}]", one_line(&r.reason)),
                        None => String::new(),
                    };
                    println!(
                        "{}  {}  {}{revoked}",
                        k.key.key_id,
                        k.source.as_str(),
                        one_line(&k.label)
                    );
                }
                Ok(())
            })
            .await
        }
        KeysCommand::Trust { file, label, data } => {
            let len = std::fs::metadata(&file)
                .with_context(|| format!("cannot read {}", file.display()))?
                .len();
            if len > MAX_PUBLISHER_KEY_BYTES as u64 {
                bail!(
                    "{} is not a minisign public key (too large)",
                    file.display()
                );
            }
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let key = PublisherKey::parse(&text)
                .with_context(|| format!("{} is not a minisign public key", file.display()))?
                .with_context(|| format!("{} holds no key", file.display()))?;
            with_site(&data.data, async |site| {
                ExtensionService::new(site.store.clone())
                    .trust_key(&key, &label)
                    .await?;
                println!("trusted key {} ({label})", key.key_id);
                Ok(())
            })
            .await
        }
        KeysCommand::Revoke {
            key_id,
            reason,
            compromised,
            data,
        } => {
            let id = KeyId::parse(&key_id).context("a key id is 16 hex digits")?;
            if reason.trim().is_empty() || reason.chars().count() > 500 {
                bail!("--reason needs 1 to 500 characters");
            }
            with_site(&data.data, async |site| {
                let service = ExtensionService::new(site.store.clone());
                let installer = installer(&site)?;
                let found = match service.get_key(&id).await? {
                    Some(k) => Some((k.key, k.label)),
                    None => installer.registry_context().await?.and_then(|r| {
                        r.index.publishers.iter().find_map(|p| {
                            p.keys
                                .iter()
                                .chain(p.revoked_keys.iter().map(|r| &r.key))
                                .find(|k| k.key_id == id)
                                .map(|k| (k.clone(), p.name.clone()))
                        })
                    }),
                };
                let Some((key, label)) = found else {
                    bail!("key {id} is not known to this site or its registry");
                };
                let disabled = service
                    .revoke_key(
                        &key,
                        &Revocation {
                            at: ferrite_domain::Timestamp::now(),
                            reason: reason.clone(),
                            compromised,
                        },
                        &label,
                        KeySource::Registry,
                    )
                    .await?;
                println!("revoked key {id}");
                for name in disabled {
                    println!("disabled plugin {name}");
                }
                Ok(())
            })
            .await
        }
    }
}

/// Fetches the configured registry now (operator commands always do); a missing
/// registry or a failed fetch is an error.
async fn refresh(installer: &Installer) -> anyhow::Result<()> {
    let r = installer
        .refresh_registry()
        .await
        .context("cannot check the plugin registry")?;
    for name in r.disabled {
        eprintln!("warning: plugin {name} was disabled: the registry revoked its signing key");
    }
    Ok(())
}

/// `(name, installed, available)` for each installed plugin (of `names`, when given)
/// the registry offers an update for.
async fn available_updates(
    site: &Site,
    installer: &Installer,
    names: &[String],
) -> anyhow::Result<Vec<(String, String, String)>> {
    let Some(ctx) = installer.registry_context().await? else {
        return Ok(Vec::new());
    };
    let service = ExtensionService::new(site.store.clone());
    let mut out = Vec::new();
    for row in site.store.read().plugins().list().await? {
        if !names.is_empty() && !names.contains(&row.name) {
            continue;
        }
        let owner = service.owner(&row.name).await?;
        if let Some((version, _)) = update_for(&ctx.index, &ctx.registry_key, &row, owner.as_ref())
        {
            out.push((row.name.clone(), row.version.clone(), version));
        }
    }
    for name in names {
        if !out.iter().any(|(n, _, _)| n == name)
            && site.store.read().plugins().get(name).await?.is_none()
        {
            bail!("plugin not installed: {name:?}");
        }
    }
    Ok(out)
}

/// Prints the report, asks (or takes `--yes`) and records the plugin.
async fn approve_and_record(
    site: &Site,
    installer: &Installer,
    inspected: Inspected,
    enable: bool,
    yes: bool,
) -> anyhow::Result<()> {
    print_report(&inspected);
    refusal(&inspected)?;
    if !yes && !confirm()? {
        bail!("aborted; nothing was installed");
    }
    let row = record_plugin(&site.store, installer, inspected, enable).await?;
    println!(
        "installed plugin {} {} ({})",
        row.name,
        row.version,
        if row.enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

/// The trust refusal of an inspection, as the command's error.
fn refusal(inspected: &Inspected) -> anyhow::Result<()> {
    match inspected.decision() {
        Ok(_) => Ok(()),
        Err(r) => bail!("refused ({}): {}", r.field, r.message),
    }
}

/// Text from a plugin, a registry or a key file on one line: control characters
/// (newlines, escapes) are replaced, so it cannot forge output lines.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
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
        startup_check(
            store.as_ref(),
            &binary_version()?,
            &config.database,
            &config.data_dir,
        )
        .await?;
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
    Ok(Installer::new(
        site.store.clone(),
        site.media.clone(),
        host_config(&site.config.plugins),
    )
    .context("cannot start a plugin host")?
    .with_trust(crate::registry::trust_settings(&site.config.plugins.trust)?))
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

/// A plugin folder's files.
struct Package {
    manifest: String,
    wasm: Vec<u8>,
    /// `ferrite-plugin.minisig` and `ferrite-plugin.pub`, when present.
    signature: Option<BundleSignature>,
}

/// An optional small file of `dir`, refused above `max` bytes before it is read.
fn read_small(dir: &Path, name: &str, max: usize) -> anyhow::Result<Option<Vec<u8>>> {
    let path = dir.join(name);
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        Ok(m) if m.len() > max as u64 => bail!("{} is larger than {max} bytes", path.display()),
        Ok(_) => {
            Ok(Some(std::fs::read(&path).with_context(|| {
                format!("cannot read {}", path.display())
            })?))
        }
    }
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
    let minisig = read_small(dir, SIGNATURE_FILE, MAX_SIGNATURE_BYTES)?;
    let key = read_small(dir, PUBLISHER_KEY_FILE, MAX_PUBLISHER_KEY_BYTES)?;
    let signature = match (minisig, key) {
        (None, None) => None,
        (None, Some(_)) => bail!(
            "{} holds {PUBLISHER_KEY_FILE} but no {SIGNATURE_FILE}",
            dir.display()
        ),
        (Some(sig), key) => Some(
            BundleSignature::from_bytes(&sig, key.as_deref())
                .map_err(|e| anyhow::anyhow!("{SIGNATURE_FILE}: {e}"))?,
        ),
    };
    Ok(Package {
        manifest,
        wasm,
        signature,
    })
}

async fn inspect(installer: &Installer, package: &Package) -> anyhow::Result<Inspected> {
    installer
        .inspect_bundle(
            &package.manifest,
            Bytes::copy_from_slice(&package.wasm),
            package.signature.clone(),
            InstallSource::Folder,
        )
        .await
        .context("the plugin is refused")
}

/// The publisher and data lines of a report (design 14 §9).
fn print_trust(inspected: &Inspected) {
    let trust = inspected.trust();
    let claim = one_line(&trust.publisher_claim);
    let Ok(d) = inspected.decision() else {
        return;
    };
    match &d.signer {
        None => println!("! unsigned: nothing confirms who made it (claims {claim})"),
        Some(s) => match &s.source {
            SignerSource::Registry { publisher_id, name } => println!(
                "publisher: verified {} ({}, registry key {})",
                one_line(publisher_id),
                one_line(name),
                s.key_id
            ),
            SignerSource::New => println!(
                "! new publisher key {} (claims {claim}): approving trusts it for this plugin's updates",
                s.key_id
            ),
            SignerSource::Approved | SignerSource::Operator => {
                println!("publisher: key {} (claims {claim})", s.key_id)
            }
        },
    }
    let next = d.next_owner.as_ref().map(ToString::to_string);
    match d.owner_change {
        OwnerChange::Claim => println!(
            "! data: the plugin's existing data will be bound to {}",
            next.unwrap_or_default()
        ),
        OwnerChange::Bind => println!("data: bound to {}", next.unwrap_or_default()),
        OwnerChange::UnsignedUpdate => {
            println!("! data: unsigned; it gets the data of the plugin installed under this name")
        }
        OwnerChange::BindUnsigned | OwnerChange::Same => {}
    }
    println!("signatures policy: {}", trust.policy.as_str());
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
    print_trust(inspected);
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
