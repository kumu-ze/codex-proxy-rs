use super::{PluginConfig, PluginError};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

const MAX_BYTES: usize = 128 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub config: PluginConfig,
    pub enabled: bool,
}
pub(super) struct Catalog {
    pub root: PathBuf,
}
impl Catalog {
    pub fn open(root: &Path) -> Result<Self, PluginError> {
        fs::create_dir_all(root).map_err(|_| PluginError::Io)?;
        Ok(Self {
            root: root.canonicalize().map_err(|_| PluginError::Io)?,
        })
    }
    pub fn load(&self, seeds: Vec<PluginConfig>) -> Result<Vec<Record>, PluginError> {
        let path = self.root.join("registry.json");
        match fs::read(path) {
            Ok(bytes) if bytes.len() <= 256 * 1024 => {
                serde_json::from_slice(&bytes).map_err(|_| PluginError::InvalidPackage)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(seeds
                .into_iter()
                .map(|config| Record {
                    config,
                    enabled: true,
                })
                .collect()),
            _ => Err(PluginError::Io),
        }
    }
    pub fn save(&self, records: &[Record]) -> Result<(), PluginError> {
        let path = self.root.join("registry.json");
        let temporary = self
            .root
            .join(format!(".registry-{}", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| PluginError::Io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(fs::Permissions::from_mode(0o600))
                    .map_err(|_| PluginError::Io)?;
            }
            file.write_all(&serde_json::to_vec(records).map_err(|_| PluginError::Io)?)
                .map_err(|_| PluginError::Io)?;
            file.sync_all().map_err(|_| PluginError::Io)?;
            fs::rename(&temporary, path).map_err(|_| PluginError::Io)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

pub(super) struct StagedPackage {
    pub path: PathBuf,
}
impl Drop for StagedPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// 仅管理员提供下载地址；不携带宿主认证，不记录含签名参数的 URL。
pub(super) async fn download(
    catalog: &Catalog,
    url: &str,
    sha256: Option<&str>,
) -> Result<StagedPackage, PluginError> {
    if url.len() > 8192
        || sha256.is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(PluginError::InvalidPackage);
    }
    let mut url = reqwest::Url::parse(url).map_err(|_| PluginError::InvalidPackage)?;
    let bytes = tokio::time::timeout(Duration::from_secs(90), async {
        for _ in 0..6 {
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(PluginError::InvalidPackage);
            }
            let host = url.host_str().ok_or(PluginError::InvalidPackage)?;
            let port = url
                .port_or_known_default()
                .ok_or(PluginError::InvalidPackage)?;
            let addresses: Vec<_> = tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
                .await
                .map_err(|_| PluginError::Io)?
                .collect();
            // 地址解析后固定连接目标，防止重定向或 DNS 变化访问回环回调与云元数据。
            if addresses.is_empty()
                || addresses.iter().any(|a| match a.ip() {
                    std::net::IpAddr::V4(ip) => {
                        ip.is_loopback()
                            || ip.is_link_local()
                            || ip.is_unspecified()
                            || ip.is_multicast()
                            || ip.is_broadcast()
                    }
                    std::net::IpAddr::V6(ip) => {
                        ip.is_loopback()
                            || ip.is_unspecified()
                            || ip.is_multicast()
                            || ip.is_unicast_link_local()
                            || ip.to_ipv4_mapped().is_some()
                    }
                })
            {
                return Err(PluginError::InvalidPackage);
            }
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .resolve_to_addrs(host, &addresses)
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(90))
                .build()
                .map_err(|_| PluginError::Io)?;
            let response = client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| PluginError::Io)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(PluginError::InvalidPackage)?;
                let next = url
                    .join(location)
                    .map_err(|_| PluginError::InvalidPackage)?;
                if url.scheme() == "https" && next.scheme() != "https" {
                    return Err(PluginError::InvalidPackage);
                }
                url = next;
                continue;
            }
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|size| size > MAX_BYTES as u64)
            {
                return Err(PluginError::InvalidPackage);
            }
            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| PluginError::Io)?;
                if body.len() + chunk.len() > MAX_BYTES {
                    return Err(PluginError::InvalidPackage);
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(body);
        }
        Err(PluginError::InvalidPackage)
    })
    .await
    .map_err(|_| PluginError::Timeout)??;
    if sha256.is_some_and(|s| !hex::encode(Sha256::digest(&bytes)).eq_ignore_ascii_case(s)) {
        return Err(PluginError::InvalidPackage);
    }
    let path = catalog
        .root
        .join(format!(".download-{}", uuid::Uuid::new_v4()));
    tokio::task::spawn_blocking(move || extract(&bytes, path))
        .await
        .map_err(|_| PluginError::Io)?
}

fn extract(bytes: &[u8], path: PathBuf) -> Result<StagedPackage, PluginError> {
    fs::create_dir(&path).map_err(|_| PluginError::Io)?;
    let staged = StagedPackage { path };
    let decoder = flate2::read::GzDecoder::new(bytes);
    // 对解压后的整个流也设上限，包含 tar 头和填充，避免压缩炸弹。
    let mut archive = tar::Archive::new(decoder.take((MAX_BYTES + 2 * 1024 * 1024) as u64));
    let mut total = 0u64;
    let mut count = 0;
    for entry in archive.entries().map_err(|_| PluginError::InvalidPackage)? {
        let mut entry = entry.map_err(|_| PluginError::InvalidPackage)?;
        count += 1;
        let name = entry
            .path()
            .map_err(|_| PluginError::InvalidPackage)?
            .into_owned();
        if count > 512
            || name.to_str().is_none_or(|s| s.contains(['\\', ':']))
            || name
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(PluginError::InvalidPackage);
        }
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(PluginError::InvalidPackage);
        }
        total = total
            .checked_add(entry.size())
            .ok_or(PluginError::InvalidPackage)?;
        if total > MAX_BYTES as u64 {
            return Err(PluginError::InvalidPackage);
        }
        let target = staged.path.join(&name);
        if kind.is_dir() {
            fs::create_dir_all(&target).map_err(|_| PluginError::Io)?;
            continue;
        }
        fs::create_dir_all(target.parent().ok_or(PluginError::InvalidPackage)?)
            .map_err(|_| PluginError::Io)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(|_| PluginError::InvalidPackage)?;
        std::io::copy(&mut entry, &mut file).map_err(|_| PluginError::InvalidPackage)?;
    }
    super::PluginPackage::open(&staged.path)?;
    Ok(staged)
}
