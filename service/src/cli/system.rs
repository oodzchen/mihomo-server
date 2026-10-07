//! Service lifecycle (delegated to the installed per-user helper) and
//! program updates (delegated to the release installer).
use anyhow::{Context as _, Result, bail};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Repository the release installer is fetched from.
pub const REPOSITORY: &str = match option_env!("MIHOMO_SERVER_REPOSITORY") {
    Some(repository) => repository,
    None => "oodzchen/mihomo-server",
};

/// Bundle root of this program: `<root>/bin/mihomo-server`.
fn bundle_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bin = executable.parent()?;
    (bin.file_name()? == "bin").then(|| bin.parent().map(Path::to_path_buf))?
}

/// The release this program was installed from (`/opt/mihomo-server/releases/<tag>`).
pub fn installed_release() -> Option<String> {
    let root = bundle_root()?;
    (root.parent()?.file_name()? == "releases").then(|| root.file_name()?.to_str().map(str::to_owned))?
}

fn helper() -> PathBuf {
    if let Some(helper) = std::env::var_os("MIHOMO_SERVER_HELPER") {
        return helper.into();
    }
    bundle_root()
        .map(|root| root.join("mihomo-server-user"))
        .filter(|helper| helper.is_file())
        .unwrap_or_else(|| "mihomo-server-user".into())
}

/// Replace this process with `mihomo-server-user ARGS`.
pub fn exec_helper(arguments: &[OsString]) -> Result<i32> {
    let helper = helper();
    let mut command = Command::new(&helper);
    command.args(arguments);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let error = command.exec();
        bail!(
            "cannot run {}: {error}; service commands need the shared installation (see the installer)",
            helper.display()
        )
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .with_context(|| format!("cannot run {}", helper.display()))?;
        Ok(status.code().unwrap_or(1))
    }
}

fn http_client(redirects: bool) -> Result<reqwest::Client> {
    Ok(client_builder(redirects)?.build()?)
}

fn client_builder(redirects: bool) -> Result<reqwest::ClientBuilder> {
    let builder = reqwest::Client::builder()
        .tls_backend_rustls()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .user_agent(format!("mihomo-server/{}", crate::VERSION))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(120))
        .redirect(if redirects {
            reqwest::redirect::Policy::limited(5)
        } else {
            reqwest::redirect::Policy::none()
        });
    crate::remote::tls::configure(builder, crate::remote::tls::RootMode::Static, false)
}

/// Latest release tag, read from GitHub's `releases/latest` redirect (no API quota).
pub async fn latest_release() -> Result<String> {
    latest_release_from(&http_client(false)?).await
}

/// [`latest_release`] through a download route (the Web check uses the managed proxy).
pub(crate) async fn latest_release_via(route: &crate::core_release::Route) -> Result<String> {
    let client = route.configure(client_builder(false)?)?.build()?;
    route.run(latest_release_from(&client)).await
}

async fn latest_release_from(client: &reqwest::Client) -> Result<String> {
    let response = client
        .get(format!("https://github.com/{REPOSITORY}/releases/latest"))
        .send()
        .await
        .context("cannot query the latest release")?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .context("no published release found")?;
    tag_from_location(location).context("unexpected latest release location")
}

pub fn tag_from_location(location: &str) -> Option<String> {
    let (_, tag) = location.trim_end_matches('/').rsplit_once("/releases/tag/")?;
    (!tag.is_empty() && !tag.contains('/')).then(|| tag.to_owned())
}

/// A private copy of the installer script: a local path or URL from
/// `MIHOMO_SERVER_INSTALLER`, else the latest published installer.
async fn installer(prefer_local: bool) -> Result<(PathBuf, tempdir::Guard)> {
    let source = std::env::var("MIHOMO_SERVER_INSTALLER").ok();
    if prefer_local
        && source.is_none()
        && let Some(local) = bundle_root()
            .map(|root| root.join("install.sh"))
            .filter(|path| path.is_file())
    {
        return Ok((local, tempdir::Guard::default()));
    }
    let source =
        source.unwrap_or_else(|| format!("https://github.com/{REPOSITORY}/releases/latest/download/install.sh"));
    if !source.starts_with("https://") && !source.starts_with("http://") {
        return Ok((source.into(), tempdir::Guard::default()));
    }
    eprintln!("==> fetching installer {source}");
    let response = http_client(true)?.get(&source).send().await?.error_for_status()?;
    let script = response.bytes().await?;
    anyhow::ensure!(
        script.starts_with(b"#!"),
        "downloaded installer is not a script; check {source}"
    );
    let guard = tempdir::Guard::create()?;
    let path = guard.path().join("install.sh");
    std::fs::write(&path, &script)?;
    Ok((path, guard))
}

async fn run_installer(prefer_local: bool, arguments: &[&str]) -> Result<i32> {
    let (script, _guard) = installer(prefer_local).await?;
    let status = Command::new("bash")
        .arg(&script)
        .args(arguments)
        .status()
        .context("cannot run bash")?;
    Ok(status.code().unwrap_or(1))
}

pub async fn update(force: bool) -> Result<i32> {
    anyhow::ensure!(
        !management_client::installation::nix_managed(),
        "{}",
        management_client::installation::NIX_UPDATE_HINT
    );
    let installed = installed_release();
    if std::env::var_os("MIHOMO_SERVER_INSTALLER").is_none() {
        match latest_release().await {
            Ok(latest) if !force && installed.as_deref() == Some(latest.as_str()) => {
                println!("mihomo-server {latest} is already the latest release (use --force to reinstall)");
                return Ok(0);
            }
            Ok(latest) => println!(
                "Updating mihomo-server {} -> {latest}",
                installed.as_deref().unwrap_or("(not installed)")
            ),
            Err(error) => eprintln!("warning: {error:#}; running the latest installer anyway"),
        }
    }
    run_installer(false, &[]).await
}

pub async fn uninstall(purge: bool) -> Result<i32> {
    anyhow::ensure!(
        !management_client::installation::nix_managed(),
        "{}",
        management_client::installation::NIX_UPDATE_HINT
    );
    let mut arguments = vec!["--uninstall"];
    if purge {
        arguments.push("--purge");
    }
    run_installer(true, &arguments).await
}

/// A private temporary directory removed on drop.
mod tempdir {
    use std::path::{Path, PathBuf};

    #[derive(Default)]
    pub struct Guard(Option<PathBuf>);

    impl Guard {
        pub fn create() -> anyhow::Result<Self> {
            let mut entropy = [0u8; 8];
            getrandom::fill(&mut entropy).map_err(|error| anyhow::anyhow!("random: {error}"))?;
            let suffix: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("mihomo-server-{suffix}"));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(&path)?;
            Ok(Self(Some(path)))
        }

        pub fn path(&self) -> &Path {
            self.0.as_deref().expect("temporary directory")
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(path) = &self.0 {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tag_is_read_from_the_redirect() {
        assert_eq!(
            tag_from_location("https://github.com/o/r/releases/tag/v0.2.0").as_deref(),
            Some("v0.2.0")
        );
        assert_eq!(tag_from_location("https://github.com/o/r/releases"), None);
        assert_eq!(tag_from_location("https://github.com/o/r/releases/tag/"), None);
    }
}
