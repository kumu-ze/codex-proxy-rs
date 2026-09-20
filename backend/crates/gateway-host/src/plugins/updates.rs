use futures::StreamExt as _;
use gateway_admin::ports::plugins::{PluginOperationError, PluginUpdateInfo};
use serde::Deserialize;
use std::time::Duration;

pub(super) fn github_repository(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_matches('/').trim_end_matches(".git");
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
    {
        return None;
    }
    Some(path.to_owned())
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

pub(super) async fn check(
    repository: &str,
    asset_pattern: &str,
    current: &str,
    proxy: Option<&str>,
) -> Result<PluginUpdateInfo, PluginOperationError> {
    let releases = release_index(repository, proxy).await?;
    let current_version =
        semver::Version::parse(current).map_err(|_| PluginOperationError::Invalid)?;
    let mut newest = None;
    for release in releases {
        if release.draft {
            continue;
        }
        let Ok(version) = semver::Version::parse(
            release
                .tag_name
                .strip_prefix('v')
                .unwrap_or(&release.tag_name),
        ) else {
            continue;
        };
        let wanted = asset_pattern.replace("{version}", &version.to_string());
        let Some(asset) = release.assets.iter().find(|asset| asset.name == wanted) else {
            continue;
        };
        // 只投影该仓库的固定 GitHub 页面与附件地址，不把 Release 中的任意 URL 交给浏览器。
        if !valid_link(&release.html_url, &format!("/{repository}/releases/tag/"))
            || !valid_link(
                &asset.browser_download_url,
                &format!("/{repository}/releases/download/"),
            )
        {
            continue;
        }
        if newest
            .as_ref()
            .is_some_and(|(previous, _, _, _, _)| previous >= &version)
        {
            continue;
        }
        let digest = asset
            .digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .filter(|digest| digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_owned);
        newest = Some((
            version,
            release.html_url,
            asset.browser_download_url.clone(),
            digest,
            release.prerelease,
        ));
    }
    let mut result = PluginUpdateInfo {
        current_version: current.to_owned(),
        latest_version: None,
        update_available: false,
        release_url: None,
        download_url: None,
        sha256: None,
        prerelease: false,
    };
    if let Some((version, release_url, download_url, digest, prerelease)) = newest {
        result.update_available = version > current_version;
        result.latest_version = Some(version.to_string());
        result.release_url = Some(release_url);
        result.download_url = Some(download_url);
        result.sha256 = digest;
        result.prerelease = prerelease;
    }
    Ok(result)
}

fn network_error(error: reqwest::Error) -> PluginOperationError {
    if error.is_timeout() {
        PluginOperationError::DownloadTimeout
    } else {
        PluginOperationError::Download
    }
}
fn valid_link(value: &str, prefix: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path().starts_with(prefix)
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

async fn release_index(
    repository: &str,
    proxy: Option<&str>,
) -> Result<Vec<Release>, PluginOperationError> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("CodexProxyRS-PluginUpdates/1")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30));
    if let Some(proxy) = proxy {
        builder =
            builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| PluginOperationError::Proxy)?);
    }
    let client = builder
        .build()
        .map_err(|_| PluginOperationError::Download)?;
    let response = client
        .get(format!(
            "https://api.github.com/repos/{repository}/releases?per_page=20"
        ))
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(network_error)?;
    if !response.status().is_success() {
        return Err(PluginOperationError::DownloadHttp(
            response.status().as_u16(),
        ));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(network_error)?;
        if body.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(PluginOperationError::UpdateSource);
        }
        body.extend_from_slice(&chunk);
    }
    let releases: Vec<Release> =
        serde_json::from_slice(&body).map_err(|_| PluginOperationError::UpdateSource)?;
    Ok(releases)
}
