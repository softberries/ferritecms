//! `ferrite self-update` (09 §1, M4-2 P25/P26/P31): download the release for this target,
//! verify it (a minisign signature over `SHA256SUMS`, then the asset's sha256), run the new
//! binary's `--version` (and `migrate status`), then install it over the running binary,
//! keeping the old one as `ferrite.prev`. It never restarts anything.

mod release;
pub mod rollback;
pub mod swap;
mod verify;

use std::ffi::OsStr;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;

use crate::{binary_target, binary_version};
use release::{Client, Release};

/// How long the new binary may take to answer `--version` / `migrate status`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// The bound on the whole download of a release (the per-read timeout is separate).
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(600);
/// The largest binary extracted from a release asset.
const MAX_BINARY: u64 = 512 << 20;

#[derive(Debug, Args)]
pub struct SelfUpdateArgs {
    /// Install this release (`X.Y.Z` or `vX.Y.Z`) instead of the newest stable one. It
    /// must be newer than this binary: use `ferrite rollback` to go back. A prerelease
    /// (`X.Y.Z-rc.N`) is installed only when named here exactly.
    #[arg(long, value_name = "X.Y.Z")]
    pub version: Option<String>,
    /// Only print whether a newer release exists (exit 2 when it does, 0 when not).
    #[arg(long)]
    pub check: bool,
    /// Do not ask for confirmation (required when stdin is not a terminal).
    #[arg(long)]
    pub yes: bool,
    /// The site's data folder: also run the new binary's `migrate status` for it.
    #[arg(long)]
    pub data: Option<PathBuf>,
    /// Test seam: the GitHub API root to use instead of https://api.github.com (plain
    /// HTTP allowed).
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub base_url: Option<String>,
    /// Test seam: the path treated as the running binary.
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub test_current_exe: Option<PathBuf>,
    /// Test seam: the bound on the whole download, in milliseconds (default 10 minutes).
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub test_download_deadline_ms: Option<u64>,
}

pub async fn run(args: SelfUpdateArgs) -> Result<ExitCode> {
    let current_version = binary_version()?;
    let target = binary_target();
    let requested = args
        .version
        .as_deref()
        .map(|v| {
            semver::Version::parse(v.strip_prefix('v').unwrap_or(v))
                .with_context(|| format!("--version {v:?} is not X.Y.Z"))
        })
        .transpose()?;
    if let Some(v) = &requested
        && *v <= current_version
    {
        bail!(
            "--version {v} is not newer than this binary ({current_version}); \
             self-update only moves forward: use `ferrite rollback` to go back"
        );
    }
    // Refuse before any download when this build cannot verify a release (P26).
    let key = if args.check {
        None
    } else {
        Some(verify::release_key()?.ok_or_else(|| {
            anyhow!(
                "this build has no release key (crates/ferrite/release.pub is a \
                 placeholder), so it cannot verify a release; install a release build"
            )
        })?)
    };
    let current = current_exe(&args)?;

    let (base, https_only) = api_base(&args);
    let client = Client::new(&base, https_only)?;
    let release = client.lookup(requested.as_ref()).await?;
    let version = release.version()?;
    match &requested {
        Some(v) if *v != version => {
            bail!(
                "asked for v{v}, but the release found is {}",
                release.tag_name
            )
        }
        // The API's `prerelease` flag is unsigned; the version is bound to the signed
        // `SHA256SUMS` (the asset name) and to the binary's `--version`, so a prerelease
        // version is refused here whatever the flag says (M4-2 R10).
        None if release.prerelease || !version.pre.is_empty() => {
            bail!(
                "the newest release {} is a prerelease; the stable channel never installs \
                 one (name it exactly with --version to install it)",
                release.tag_name
            )
        }
        _ => {}
    }
    if version <= current_version {
        println!("ferrite {current_version} is up to date (newest release: {version})");
        return Ok(ExitCode::SUCCESS);
    }
    let asset_name = release::asset_name(&version, target);
    if args.check {
        println!("a newer release is available: {version} (this binary: {current_version})");
        if release.asset(&asset_name).is_none() {
            println!("note: release {version} has no asset for this target ({target})");
        }
        return Ok(ExitCode::from(2));
    }
    let Some(key) = key else {
        bail!("internal: no release key")
    };

    let dir = current
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent folder", current.display()))?;
    // The work folder sits beside the binary, so the new binary is renamed within one
    // filesystem; creating it is also the writability check.
    let work = tempfile::Builder::new()
        .prefix(".ferrite-update-")
        .tempdir_in(dir)
        .map_err(|e| {
            anyhow!(
                "cannot write to {} ({e}): run self-update as the binary's owner, outside \
                 the systemd sandbox",
                dir.display()
            )
        })?;

    let new = fetch_and_verify(
        &client,
        &release,
        &asset_name,
        &key,
        work.path(),
        download_deadline(&args),
    )
    .await?;

    let (code, out) = probe(&new, &[OsStr::new("--version")]).await?;
    let reported = out.trim();
    let reported_version = reported
        .split_whitespace()
        .last()
        .and_then(|v| semver::Version::parse(v).ok());
    if code != Some(0) || reported_version.as_ref() != Some(&version) {
        bail!(
            "the downloaded binary does not run as ferrite {version} (exit {code:?}, \
             output {reported:?}); nothing was changed"
        );
    }
    println!("new binary: {reported}");

    if let Some(data) = &args.data {
        let mut probe_args = ["migrate", "status", "--data"].map(OsStr::new).to_vec();
        probe_args.push(data.as_os_str());
        let (code, out) = probe(&new, &probe_args).await?;
        println!("migrate status of the new binary for {}:", data.display());
        print!("{out}");
        match code {
            Some(0) | Some(2) => {}
            Some(3) => bail!(
                "the new binary cannot run this site's database schema (migrate status \
                 exit 3); nothing was changed"
            ),
            other => eprintln!(
                "warning: could not read the migration status with the new binary (exit \
                 {other:?}; for example, SQLite while `serve` runs): see its output above"
            ),
        }
    }

    if !args.yes && !confirm(&current, &current_version, &version)? {
        bail!("cancelled; nothing was changed");
    }
    swap::install_new(&current, &new).map_err(|e| {
        anyhow!("{e:#}; run self-update as the binary's owner, outside the systemd sandbox")
    })?;
    drop(work);
    println!(
        "updated {} from {current_version} to {version}; the previous binary is kept at {}",
        current.display(),
        swap::prev_path(&current).display()
    );
    println!(
        "restart to run it: `systemctl restart ferrite` under systemd, otherwise stop and \
         start `ferrite serve`"
    );
    Ok(ExitCode::SUCCESS)
}

/// The running binary (symlinks resolved), or the test seam's path.
fn current_exe(args: &SelfUpdateArgs) -> Result<PathBuf> {
    #[cfg(feature = "test-seams")]
    if let Some(path) = &args.test_current_exe {
        return std::fs::canonicalize(path)
            .with_context(|| format!("cannot resolve {}", path.display()));
    }
    let _ = args;
    let exe = std::env::current_exe().context("cannot find the running binary")?;
    std::fs::canonicalize(&exe).with_context(|| format!("cannot resolve {}", exe.display()))
}

/// The bound on downloading a release (10 minutes; a test seam may shorten it).
fn download_deadline(args: &SelfUpdateArgs) -> Duration {
    #[cfg(feature = "test-seams")]
    if let Some(ms) = args.test_download_deadline_ms {
        return Duration::from_millis(ms);
    }
    let _ = args;
    DOWNLOAD_DEADLINE
}

/// The GitHub API root and whether only HTTPS is allowed.
fn api_base(args: &SelfUpdateArgs) -> (String, bool) {
    #[cfg(feature = "test-seams")]
    if let Some(base) = &args.base_url {
        return (base.clone(), false);
    }
    let _ = args;
    (release::DEFAULT_API.to_owned(), true)
}

/// Downloads `SHA256SUMS`, its signature and the asset into `work`, verifies them (P26:
/// the signature first, then the sha256) and unpacks the binary. Returns its path.
async fn fetch_and_verify(
    client: &Client,
    release: &Release,
    asset_name: &str,
    key: &minisign_verify::PublicKey,
    work: &Path,
    deadline: Duration,
) -> Result<PathBuf> {
    let tag = &release.tag_name;
    let asset = release.asset(asset_name).ok_or_else(|| {
        anyhow!(
            "release {tag} has no asset for this target ({}): expected {asset_name}",
            binary_target()
        )
    })?;
    let sums_asset = release
        .asset("SHA256SUMS")
        .ok_or_else(|| anyhow!("release {tag} has no SHA256SUMS"))?;
    let sig_asset = release
        .asset("SHA256SUMS.minisig")
        .ok_or_else(|| anyhow!("release {tag} has no SHA256SUMS.minisig"))?;
    let archive = work.join(asset_name);
    let downloads = async {
        let sums = client.fetch_small(sums_asset).await?;
        let sig = client.fetch_small(sig_asset).await?;
        verify::check_signature(key, &sums, &sig)?;
        let expected = verify::listed_sha256(&sums, asset_name)?;
        let mut file = std::fs::File::create(&archive)
            .with_context(|| format!("cannot create {}", archive.display()))?;
        let actual = verify::hex(&client.download(asset, &mut file).await?);
        anyhow::Ok((expected, actual))
    };
    // `read_timeout` bounds each read; this bounds the whole download.
    let (expected, actual) = tokio::time::timeout(deadline, downloads)
        .await
        .map_err(|_| {
            anyhow!(
                "the download of release {tag} took longer than {}s; nothing was changed",
                deadline.as_secs_f32()
            )
        })??;
    if actual != expected {
        bail!(
            "the sha256 of {asset_name} ({actual}) does not match SHA256SUMS ({expected}); \
             nothing was changed"
        );
    }
    let new = work.join("ferrite");
    let (archive_path, new_path) = (archive.clone(), new.clone());
    tokio::task::spawn_blocking(move || unpack(&archive_path, &new_path))
        .await
        .context("unpacking panicked")??;
    Ok(new)
}

/// Writes the `ferrite` file of the `.tar.gz` at `archive` (at its top level or one folder
/// down, P25) to `out` with mode 0755. Nothing else is extracted.
fn unpack(archive: &Path, out: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut found = false;
    for entry in tar
        .entries()
        .context("the release asset is not a .tar.gz")?
    {
        let mut entry = entry.context("the release asset is not a valid .tar.gz")?;
        let path = entry.path()?.into_owned();
        if !is_binary_entry(&path) {
            continue;
        }
        if entry.header().entry_type() != tar::EntryType::Regular {
            bail!("the release asset's ferrite is not a regular file");
        }
        if found {
            bail!("the release asset holds more than one ferrite binary");
        }
        found = true;
        if entry.header().size()? > MAX_BINARY {
            bail!("the release asset's ferrite is larger than {MAX_BINARY} bytes");
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o755);
        let mut dst = opts
            .open(out)
            .with_context(|| format!("cannot create {}", out.display()))?;
        let mut buf = [0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = entry
                .read(&mut buf)
                .context("the release asset is not a valid .tar.gz")?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_BINARY {
                bail!("the release asset's ferrite is larger than {MAX_BINARY} bytes");
            }
            dst.write_all(&buf[..n])?;
        }
        dst.sync_all()?;
    }
    if !found {
        bail!("the release asset holds no ferrite binary");
    }
    Ok(())
}

/// Whether a tar entry path is the release's binary: a file named `ferrite` at the top
/// level or one folder down, `./` components ignored (`tar -C dir .` writes them), and
/// nothing but plain names otherwise (no `..`, no root).
fn is_binary_entry(path: &Path) -> bool {
    use std::path::Component;
    let parts: Vec<Component<'_>> = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    (1..=2).contains(&parts.len())
        && parts.iter().all(|c| matches!(c, Component::Normal(_)))
        && parts.last() == Some(&Component::Normal(OsStr::new("ferrite")))
}

/// Runs `binary` (the new one, or for `rollback` the kept one) with `args` (stdin closed,
/// bounded by [`PROBE_TIMEOUT`]); returns its exit code and its stdout followed by its
/// stderr.
async fn probe(binary: &Path, args: &[&OsStr]) -> Result<(Option<i32>, String)> {
    let child = tokio::process::Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(PROBE_TIMEOUT, child)
        .await
        .map_err(|_| {
            let shown: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
            anyhow!(
                "{} did not answer `{}` in time",
                binary.display(),
                shown.join(" ")
            )
        })?
        .with_context(|| format!("cannot run {}", binary.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), text))
}

fn confirm(current: &Path, from: &semver::Version, to: &semver::Version) -> Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("self-update needs --yes when stdin is not a terminal; nothing was changed");
    }
    eprint!(
        "Replace {} ({from}) with {to}, keeping the old binary as {}? [y/N] ",
        current.display(),
        swap::prev_path(current).display()
    );
    std::io::stderr().flush()?;
    let mut line = String::new();
    stdin.read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_entry_paths() {
        for ok in [
            "ferrite",
            "./ferrite",
            "dir/ferrite",
            "./dir/ferrite",
            "dir/./ferrite",
        ] {
            assert!(is_binary_entry(Path::new(ok)), "{ok}");
        }
        for bad in [
            "a/b/ferrite",
            "../ferrite",
            "/ferrite",
            "dir/../ferrite",
            "ferrite/README.md",
            "ferrite.prev",
            ".",
        ] {
            assert!(!is_binary_entry(Path::new(bad)), "{bad}");
        }
    }
}
