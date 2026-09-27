//! Stable/Alpha upstream release discovery and verified compressed-package preparation.
//! Actor-owned staging validates executables; activation remains a separate workflow.
#[path = "core_stage.rs"]
mod stage;
#[path = "core_release_transport.rs"]
mod transport;
use crate::remote::tls::{self, RootMode};
use crate::resources::TARGET;
use anyhow::{Context as _, Result, ensure};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
pub use stage::StagedCore;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{io::AsyncWriteExt as _, sync::watch};
pub(crate) use transport::Route;
const MAX_METADATA: usize = 1024 * 1024;
const MAX_PACKAGE: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST: u64 = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoreRelease {
    pub version: String,
    pub target: String,
    pub asset: String,
    pub bytes: u64,
    pub sha256: String,
    pub download_url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PreparedCore {
    pub id: String,
    pub release: CoreRelease,
}
pub(crate) struct ResolvedRelease {
    pub(crate) release: CoreRelease,
    pub(crate) route: Route,
}
#[derive(Deserialize)]
struct ReleaseResponse {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    state: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    release: CoreRelease,
}

fn stable_version(version: &str) -> bool {
    if version.len() > 64 {
        return false;
    }
    let Some(version) = version.strip_prefix('v') else {
        return false;
    };
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}
#[derive(Clone, Copy)]
pub(crate) enum ReleaseChannel {
    Stable,
    Alpha,
}
pub(crate) fn alpha_version(version: &str) -> bool {
    version.strip_prefix("alpha-").is_some_and(|hash| {
        (7..=40).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub(crate) fn release_version(version: &str) -> bool {
    stable_version(version) || alpha_version(version)
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}
fn asset_name(version: &str) -> Result<String> {
    ensure!(release_version(version), "unsupported core version");
    // Retain upstream's bundled amd64-v2 variant. Other targets need their own
    // archive/platform runtime validation before being advertised as upgrades.
    ensure!(
        matches!(TARGET, "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl"),
        "core preparation currently requires Linux x86_64"
    );
    Ok(format!("mihomo-linux-amd64-v2-{version}.gz"))
}
#[derive(Clone)]
struct Repository {
    api: url::Url,
    packages: url::Url,
}
impl Repository {
    fn official() -> Self {
        Self {
            api: "https://api.github.com/repos/MetaCubeX/mihomo/releases/"
                .parse()
                .unwrap(),
            packages: "https://github.com/MetaCubeX/mihomo/releases/download/"
                .parse()
                .unwrap(),
        }
    }
    fn package_url(&self, version: &str) -> Result<String> {
        Ok(self
            .packages
            .join(&format!(
                "{}/{}",
                if alpha_version(version) {
                    "Prerelease-Alpha"
                } else {
                    version
                },
                asset_name(version)?
            ))?
            .to_string())
    }
    #[cfg(test)]
    fn client(&self) -> Result<reqwest::Client> {
        self.client_for(&Route::Direct, RootMode::Platform)
    }
    fn client_for(&self, route: &Route, roots: RootMode) -> Result<reqwest::Client> {
        let api = self.api.clone();
        let packages = self.packages.clone();
        let builder = reqwest::Client::builder()
            .tls_backend_rustls()
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .https_only(self.api.scheme() == "https")
            .user_agent(concat!("mihomo-server/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                let url = attempt.url();
                let known_origin = url.origin() == api.origin() || url.origin() == packages.origin();
                let github_asset = url.scheme() == "https"
                    && url.port_or_known_default() == Some(443)
                    && matches!(
                        url.host_str(),
                        Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com")
                    );
                if attempt.previous().len() > 5
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || !(known_origin || github_asset)
                {
                    attempt.error("core release redirect is not allowed")
                } else {
                    attempt.follow()
                }
            }));
        tls::configure(route.configure(builder)?, roots, false)?
            .build()
            .context("build verified core release client")
    }
    #[cfg(test)]
    async fn resolve(&self, version: Option<&str>, routes: Vec<Route>) -> Result<ResolvedRelease> {
        self.resolve_channel(version, routes, ReleaseChannel::Stable).await
    }
    async fn resolve_channel(
        &self,
        version: Option<&str>,
        routes: Vec<Route>,
        channel: ReleaseChannel,
    ) -> Result<ResolvedRelease> {
        check_request(version, channel)?;
        let mut last = None;
        for route in routes {
            let result = tokio::time::timeout(
                Duration::from_secs(20),
                route.run(async {
                    let first = async {
                        let client = self.client_for(&route, RootMode::Platform)?;
                        self.discover_channel(&client, version, channel).await
                    }
                    .await;
                    match first {
                        Ok(release) => Ok(release),
                        Err(error) if tls::should_retry(&error) => {
                            let client = self.client_for(&route, RootMode::Static)?;
                            self.discover_channel(&client, version, channel)
                                .await
                                .context("core metadata static roots fallback failed")
                        }
                        Err(error) => Err(error),
                    }
                }),
            )
            .await
            .context("core release metadata timed out")
            .and_then(|result| result);
            match result {
                Ok(release) => return Ok(ResolvedRelease { release, route }),
                Err(error) => last = Some(error.context(format!("core metadata {} route failed", route.name()))),
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("no core download route available")))
    }
    fn validate(&self, release: &CoreRelease) -> Result<()> {
        ensure!(
            release.target == TARGET && release.asset == asset_name(&release.version)?,
            "core release target/asset mismatch"
        );
        ensure!(
            release.bytes > 2 && release.bytes <= MAX_PACKAGE,
            "core package exceeds size bounds"
        );
        ensure!(valid_hash(&release.sha256), "missing or invalid core package SHA-256");
        ensure!(
            release.download_url == self.package_url(&release.version)?,
            "core package URL does not match pinned release"
        );
        Ok(())
    }
    #[cfg(test)]
    async fn discover(&self, client: &reqwest::Client, version: Option<&str>) -> Result<CoreRelease> {
        self.discover_channel(client, version, ReleaseChannel::Stable).await
    }
    async fn discover_channel(
        &self,
        client: &reqwest::Client,
        version: Option<&str>,
        channel: ReleaseChannel,
    ) -> Result<CoreRelease> {
        check_request(version, channel)?;
        let endpoint = match (channel, version) {
            (ReleaseChannel::Alpha, _) => self.api.join("tags/Prerelease-Alpha")?,
            (ReleaseChannel::Stable, Some(version)) => self.api.join(&format!("tags/{version}"))?,
            (ReleaseChannel::Stable, None) => self.api.join("latest")?,
        };
        let mut response = client
            .get(endpoint)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| e.without_url())
            .context("fetch core release metadata")?;
        ensure!(
            response.status().is_success(),
            "core release metadata request failed with status {}",
            response.status()
        );
        ensure!(
            response.content_length().is_none_or(|n| n <= MAX_METADATA as u64),
            "core release metadata exceeds 1 MiB"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| e.without_url())
            .context("read core release metadata")?
        {
            ensure!(
                chunk.len() <= MAX_METADATA - bytes.len(),
                "core release metadata exceeds 1 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        // Do not echo remote response text or parser excerpts.
        let metadata: ReleaseResponse =
            serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid core release metadata"))?;
        ensure!(!metadata.draft, "draft core release is unsupported");
        let (resolved_version, asset) = match channel {
            ReleaseChannel::Stable => {
                ensure!(
                    !metadata.prerelease && stable_version(&metadata.tag_name),
                    "only published stable core releases are supported"
                );
                let name = asset_name(&metadata.tag_name)?;
                let mut assets = metadata.assets.into_iter().filter(|a| a.name == name);
                let asset = assets.next().context("matching core release asset missing")?;
                ensure!(assets.next().is_none(), "duplicate core release assets");
                (metadata.tag_name, asset)
            }
            ReleaseChannel::Alpha => {
                ensure!(
                    metadata.prerelease && metadata.tag_name == "Prerelease-Alpha",
                    "invalid Alpha core release tag/status"
                );
                let mut assets = metadata
                    .assets
                    .into_iter()
                    .filter(|a| a.name.starts_with("mihomo-linux-amd64-v2-alpha-") && a.name.ends_with(".gz"));
                let asset = assets.next().context("matching Alpha core release asset missing")?;
                ensure!(assets.next().is_none(), "duplicate Alpha core release assets");
                let resolved = asset
                    .name
                    .strip_prefix("mihomo-linux-amd64-v2-")
                    .and_then(|v| v.strip_suffix(".gz"))
                    .context("invalid Alpha core asset")?
                    .to_string();
                ensure!(alpha_version(&resolved), "invalid Alpha core version");
                (resolved, asset)
            }
        };
        ensure!(
            version.is_none_or(|v| v == resolved_version),
            "resolved core version differs from requested tag"
        );
        ensure!(asset.state == "uploaded", "core asset upload is incomplete");
        let hash = asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .context("core release has no SHA-256 digest")?;
        let release = CoreRelease {
            version: resolved_version,
            target: TARGET.into(),
            asset: asset.name,
            bytes: asset.size,
            sha256: hash.to_ascii_lowercase(),
            download_url: asset.browser_download_url,
        };
        self.validate(&release)?;
        Ok(release)
    }
}
pub(crate) async fn discover_via(version: Option<&str>, routes: Vec<Route>) -> Result<ResolvedRelease> {
    discover_channel_via(version, routes, ReleaseChannel::Stable).await
}

fn check_request(version: Option<&str>, channel: ReleaseChannel) -> Result<()> {
    if let Some(version) = version {
        ensure!(
            match channel {
                ReleaseChannel::Stable => stable_version(version),
                ReleaseChannel::Alpha => alpha_version(version),
            },
            "unsupported requested core version/channel"
        );
    }
    asset_name(version.unwrap_or("v0.0.0"))?;
    Ok(())
}
pub(crate) async fn discover_channel_via(
    version: Option<&str>,
    routes: Vec<Route>,
    channel: ReleaseChannel,
) -> Result<ResolvedRelease> {
    Repository::official().resolve_channel(version, routes, channel).await
}

pub(crate) struct CoreDownloads {
    root: PathBuf,
    repository: Repository,
}
impl CoreDownloads {
    pub(crate) fn new(core_directory: &Path) -> Result<Self> {
        let root = core_directory.join(".upgrade-staging");
        private_directory(&root)?;
        // These are exclusively owned temporary directories; never follow links
        // or touch unknown names, cached candidates or the live executable.
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_str().is_some_and(pending_name) && fs::symlink_metadata(entry.path())?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(Self {
            root: root.canonicalize()?,
            repository: Repository::official(),
        })
    }
    #[cfg(test)]
    pub(crate) async fn prepare(
        &self,
        version: Option<&str>,
        shutdown: &watch::Receiver<bool>,
    ) -> Result<PreparedCore> {
        self.prepare_via(version, vec![Route::Direct], shutdown).await
    }
    #[cfg(test)]
    pub(crate) async fn prepare_via(
        &self,
        version: Option<&str>,
        routes: Vec<Route>,
        shutdown: &watch::Receiver<bool>,
    ) -> Result<PreparedCore> {
        let resolved = self.repository.resolve(version, routes).await?;
        self.prepare_selected(resolved, shutdown).await
    }
    /// Keep the metadata/hash pinned across the pre-download no-op check.
    #[cfg(test)]
    pub(crate) async fn prepare_resolved(
        &self,
        release: CoreRelease,
        shutdown: &watch::Receiver<bool>,
    ) -> Result<PreparedCore> {
        self.prepare_selected(
            ResolvedRelease {
                release,
                route: Route::Direct,
            },
            shutdown,
        )
        .await
    }
    pub(crate) async fn prepare_selected(
        &self,
        resolved: ResolvedRelease,
        shutdown: &watch::Receiver<bool>,
    ) -> Result<PreparedCore> {
        let ResolvedRelease { release, route } = resolved;
        route.check()?;
        self.repository.validate(&release)?;
        ensure!(!*shutdown.borrow(), "core preparation cancelled during shutdown");
        let id = format!("{}-{}", release.version, release.sha256);
        let final_path = self.root.join(&id);
        if fs::symlink_metadata(&final_path).is_ok() {
            let cached = self.inspect(&id)?;
            ensure!(cached.release == release, "prepared core release metadata changed");
            route.check()?;
            return Ok(cached);
        }
        let pending = Pending::new(&self.root)?;
        tokio::time::timeout(
            Duration::from_secs(300),
            route.run(async {
                let first = async {
                    let client = self.repository.client_for(&route, RootMode::Platform)?;
                    self.download(&client, &release, &pending.0).await
                }
                .await;
                match first {
                    Ok(()) => Ok(()),
                    Err(error) if tls::should_retry(&error) => {
                        let package = pending.0.join("package.gz");
                        if package.try_exists()? {
                            // Only this exclusively owned partial file may be retired for TLS retry.
                            let m = fs::symlink_metadata(&package)?;
                            ensure!(m.is_file() && m.len() <= MAX_PACKAGE, "unsafe partial core package");
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt as _;
                                ensure!(
                                    m.permissions().mode() & 0o077 == 0,
                                    "unsafe partial core package permissions"
                                );
                            }
                            fs::remove_file(package)?;
                        }
                        let client = self.repository.client_for(&route, RootMode::Static)?;
                        self.download(&client, &release, &pending.0)
                            .await
                            .context("core package static roots fallback failed")
                    }
                    Err(error) => Err(error),
                }
            }),
        )
        .await
        .context("core package download timed out")??;
        ensure!(!*shutdown.borrow(), "core preparation cancelled during shutdown");
        route.check()?;
        let manifest = Manifest {
            schema_version: 1,
            release: release.clone(),
        };
        let mut file = private_file(&pending.0.join("release.json"))?;
        file.write_all(&serde_json::to_vec(&manifest)?)?;
        file.sync_all()?;
        sync_directory(&pending.0)?;
        // No await between cancellation check and atomic publication.
        ensure!(!*shutdown.borrow(), "core preparation cancelled during shutdown");
        route.check()?;
        fs::rename(&pending.0, &final_path)?;
        sync_directory(&self.root)?;
        Ok(PreparedCore { id, release })
    }
    async fn download(&self, client: &reqwest::Client, release: &CoreRelease, directory: &Path) -> Result<()> {
        let mut response = client
            .get(&release.download_url)
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .map_err(|e| e.without_url())
            .context("fetch core package")?;
        ensure!(
            response.status().is_success(),
            "core package request failed with status {}",
            response.status()
        );
        ensure!(
            response
                .headers()
                .get("content-encoding")
                .is_none_or(|v| v == "identity"),
            "encoded core package response is unsupported"
        );
        ensure!(
            response.content_length().is_none_or(|n| n == release.bytes),
            "core package length differs from release metadata"
        );
        let output = private_file(&directory.join("package.gz"))?;
        let mut output = tokio::fs::File::from_std(output);
        let mut digest = Context::new(&SHA256);
        let mut size = 0;
        let mut magic = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| e.without_url())
            .context("read core package")?
        {
            ensure!(
                chunk.len() as u64 <= release.bytes - size,
                "core package exceeds declared length"
            );
            magic.extend(chunk.iter().take(2 - magic.len()).copied());
            size += chunk.len() as u64;
            digest.update(&chunk);
            output.write_all(&chunk).await?;
        }
        ensure!(size == release.bytes, "core package is incomplete");
        ensure!(
            hex(digest.finish().as_ref()) == release.sha256,
            "core package SHA-256 mismatch"
        );
        ensure!(magic == [0x1f, 0x8b], "core package is not gzip");
        output.flush().await?;
        output.sync_all().await?;
        Ok(())
    }
    pub(crate) fn inspect(&self, id: &str) -> Result<PreparedCore> {
        let (version, hash) = id.rsplit_once('-').context("invalid prepared core ID")?;
        ensure!(
            release_version(version) && valid_hash(hash) && hash == hash.to_ascii_lowercase(),
            "invalid prepared core ID"
        );
        let directory = self.root.join(id);
        check_directory(&directory)?;
        let manifest = directory.join("release.json");
        check_file(&manifest, MAX_MANIFEST)?;
        let manifest: Manifest = serde_json::from_slice(&fs::read(manifest)?)
            .map_err(|_| anyhow::anyhow!("invalid prepared core manifest"))?;
        ensure!(manifest.schema_version == 1, "unsupported prepared core manifest");
        self.repository.validate(&manifest.release)?;
        ensure!(
            manifest.release.version == version && manifest.release.sha256 == hash,
            "prepared core ID/manifest mismatch"
        );
        let package = directory.join("package.gz");
        check_file(&package, MAX_PACKAGE)?;
        let mut file = File::open(package)?;
        let mut digest = Context::new(&SHA256);
        let mut size = 0;
        let mut buffer = [0_u8; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            ensure!(
                count as u64 <= manifest.release.bytes - size,
                "prepared core package exceeds declared length"
            );
            size += count as u64;
            digest.update(&buffer[..count]);
        }
        ensure!(
            size == manifest.release.bytes && hex(digest.finish().as_ref()) == hash,
            "prepared core package integrity check failed"
        );
        Ok(PreparedCore {
            id: id.into(),
            release: manifest.release,
        })
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn pending_name(name: &str) -> bool {
    name.strip_prefix(".pending-")
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
struct Pending(PathBuf);
impl Pending {
    fn new(root: &Path) -> Result<Self> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("generate core staging ID"))?;
        let path = root.join(format!(".pending-{}", hex(&random)));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        if let Err(e) = builder.create(path)
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(e.into());
        }
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    check_directory(path)
}
fn check_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_dir(), "core staging directory must be a real directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "core staging directory must be private"
        );
    }
    Ok(())
}
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn check_file(path: &Path, limit: u64) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= limit,
        "invalid core staging file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "core staging file must be private"
        );
    }
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "core_release_tests.rs"]
mod tests;
