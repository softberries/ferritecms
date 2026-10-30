//! Release lookup and download (M4-2 P25/P31): GitHub Releases of
//! `softberries/ferritecms`, read through the REST API.

use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The GitHub API root. Under `test-seams` a hidden `--base-url` replaces it.
pub(super) const DEFAULT_API: &str = "https://api.github.com";
const REPO: &str = "softberries/ferritecms";

/// The largest `SHA256SUMS` / `.minisig` accepted.
const SMALL_LIMIT: u64 = 1 << 20;
/// The largest release asset accepted.
const ASSET_LIMIT: u64 = 1 << 30;

/// A release as the GitHub API describes it (the fields used).
#[derive(Debug, Deserialize)]
pub(super) struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

impl Release {
    /// The version in the tag (`vX.Y.Z`, P25).
    pub fn version(&self) -> Result<semver::Version> {
        let tag = self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name);
        semver::Version::parse(tag)
            .with_context(|| format!("the release tag {:?} is not vX.Y.Z", self.tag_name))
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// The asset name for `version` and a target triple (P25).
pub(super) fn asset_name(version: &semver::Version, target: &str) -> String {
    format!("ferrite-{version}-{target}.tar.gz")
}

pub(super) struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    /// A client for `base` (the GitHub API root). `https_only` refuses plain-HTTP URLs,
    /// redirects included; only the test seam turns it off.
    pub fn new(base: &str, https_only: bool) -> Result<Self> {
        // reqwest has no provider of its own (M4-2 P28: no aws-lc) and panics when the
        // process has none; an already installed one is kept.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("ferrite/", env!("CARGO_PKG_VERSION")))
            .https_only(https_only)
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| anyhow!("cannot build the HTTP client: {e}"))?;
        Ok(Client {
            http,
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    /// The newest stable release, or the release tagged `v<version>`.
    pub async fn lookup(&self, version: Option<&semver::Version>) -> Result<Release> {
        let url = match version {
            None => format!("{}/repos/{REPO}/releases/latest", self.base),
            Some(v) => format!("{}/repos/{REPO}/releases/tags/v{v}", self.base),
        };
        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| anyhow!("cannot reach {url}: {e}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            match version {
                None => bail!("no release found at {url}"),
                Some(v) => bail!("there is no release v{v}"),
            }
        }
        let body = read_limited(resp, &url, SMALL_LIMIT).await?;
        let release: Release = serde_json::from_slice(&body)
            .with_context(|| format!("unexpected release metadata from {url}"))?;
        if release.draft {
            bail!("release {} is a draft", release.tag_name);
        }
        Ok(release)
    }

    /// Downloads a small file (`SHA256SUMS`, its signature) into memory.
    pub async fn fetch_small(&self, asset: &Asset) -> Result<Vec<u8>> {
        let resp = self.get(asset).await?;
        read_limited(resp, &asset.browser_download_url, SMALL_LIMIT).await
    }

    /// Streams an asset into `out`, returning its sha256.
    pub async fn download(&self, asset: &Asset, out: &mut std::fs::File) -> Result<[u8; 32]> {
        let url = &asset.browser_download_url;
        let mut resp = self.get(asset).await?;
        check_length(&resp, url, ASSET_LIMIT)?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| anyhow!("download of {url} failed: {e}"))?
        {
            total += chunk.len() as u64;
            if total > ASSET_LIMIT {
                bail!("{url} is larger than {ASSET_LIMIT} bytes");
            }
            hasher.update(&chunk);
            out.write_all(&chunk)
                .with_context(|| format!("cannot write the download of {}", asset.name))?;
        }
        out.flush()?;
        Ok(hasher.finalize().into())
    }

    async fn get(&self, asset: &Asset) -> Result<reqwest::Response> {
        let url = &asset.browser_download_url;
        self.http
            .get(url)
            .header("Accept", "application/octet-stream")
            .send()
            .await
            .map_err(|e| anyhow!("cannot download {url}: {e}"))
    }
}

fn check_length(resp: &reqwest::Response, url: &str, limit: u64) -> Result<()> {
    if !resp.status().is_success() {
        bail!("{url} answered {}", resp.status());
    }
    if resp.content_length().is_some_and(|n| n > limit) {
        bail!("{url} is larger than {limit} bytes");
    }
    Ok(())
}

async fn read_limited(mut resp: reqwest::Response, url: &str, limit: u64) -> Result<Vec<u8>> {
    check_length(&resp, url, limit)?;
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| anyhow!("reading {url} failed: {e}"))?
    {
        if body.len() as u64 + chunk.len() as u64 > limit {
            bail!("{url} is larger than {limit} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The release carries seven assets plus `SHA256SUMS` and its signature (M4-3a P47): the
    /// six target tarballs and `ferrite-plugins-<version>.tar.gz`. Self-update picks its
    /// asset by the exact name of its target, so the plugins asset is never taken, and its
    /// line in `SHA256SUMS` does not disturb the lookup of the binary's.
    #[test]
    fn target_assets_are_picked_beside_the_plugins_asset() {
        const TARGETS: [&str; 6] = [
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
        ];
        let version = semver::Version::new(0, 1, 0);
        let plugins = format!("ferrite-plugins-{version}.tar.gz");
        let mut names: Vec<String> = TARGETS.iter().map(|t| asset_name(&version, t)).collect();
        names.push(plugins.clone());
        let mut sums = String::new();
        for (i, name) in names.iter().enumerate() {
            sums.push_str(&format!("{:064x}  {name}\n", i + 1));
        }
        names.push("SHA256SUMS".to_owned());
        names.push("SHA256SUMS.minisig".to_owned());
        let release = Release {
            tag_name: format!("v{version}"),
            draft: false,
            prerelease: false,
            assets: names
                .iter()
                .map(|name| Asset {
                    name: name.clone(),
                    browser_download_url: format!("https://example.test/{name}"),
                })
                .collect(),
        };

        for (i, target) in TARGETS.iter().enumerate() {
            let want = asset_name(&version, target);
            assert_ne!(want, plugins);
            assert_eq!(release.asset(&want).unwrap().name, want);
            assert_eq!(
                super::super::verify::listed_sha256(sums.as_bytes(), &want).unwrap(),
                format!("{:064x}", i + 1)
            );
        }
        assert_eq!(release.asset(&plugins).unwrap().name, plugins);
    }
}
