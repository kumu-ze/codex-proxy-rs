use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::PluginError;

const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;

/// API 主版本不兼容时拒绝加载；安装器只复制摘要清单明确列出的文件。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub api_version: u32,
    pub executable: String,
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// 经过结构、路径和内容摘要验证的本地包。
pub struct PluginPackage {
    root: PathBuf,
    manifest: PluginManifest,
}

impl PluginPackage {
    /// 加载管理员已放置的包目录。摘要校验不等于发布者认证。
    pub fn open(root: &Path) -> Result<Self, PluginError> {
        let root = root
            .canonicalize()
            .map_err(|_| PluginError::InvalidPackage)?;
        let path = root.join("plugin.json");
        let metadata = fs::symlink_metadata(&path).map_err(|_| PluginError::InvalidPackage)?;
        if !metadata.is_file() || metadata.len() > 64 * 1024 {
            return Err(PluginError::InvalidPackage);
        }
        let bytes = fs::read(path).map_err(|_| PluginError::Io)?;
        let manifest: PluginManifest =
            serde_json::from_slice(&bytes).map_err(|_| PluginError::InvalidPackage)?;
        if manifest.api_version != 1 {
            return Err(PluginError::Incompatible);
        }
        if manifest.id.is_empty()
            || manifest.id.len() > 64
            || !manifest
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || semver::Version::parse(&manifest.version).is_err()
            || manifest.files.is_empty()
            || manifest.files.len() > 256
            || !manifest.files.contains_key(&manifest.executable)
            || manifest
                .capabilities
                .iter()
                .any(|c| c != "request.openai" && c != "provider.openai")
            || manifest.capabilities.len() > 2
        {
            return Err(PluginError::InvalidPackage);
        }
        let mut total = 0u64;
        for (name, digest) in &manifest.files {
            let path = checked_file(&root, name)?;
            let size = fs::metadata(&path).map_err(|_| PluginError::Io)?.len();
            total = total.checked_add(size).ok_or(PluginError::InvalidPackage)?;
            if total > MAX_PACKAGE_BYTES
                || digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(PluginError::InvalidPackage);
            }
            let data = fs::read(path).map_err(|_| PluginError::Io)?;
            if hex::encode(Sha256::digest(data)) != *digest {
                return Err(PluginError::InvalidPackage);
            }
        }
        Ok(Self { root, manifest })
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub(super) fn executable(&self) -> PathBuf {
        self.root.join(&self.manifest.executable)
    }
    pub(super) fn root(&self) -> &Path {
        &self.root
    }
}

fn checked_file(root: &Path, name: &str) -> Result<PathBuf, PluginError> {
    if name.is_empty() || name.contains(['\\', ':']) || name == "plugin.json" {
        return Err(PluginError::InvalidPackage);
    }
    let mut current = root.to_path_buf();
    for component in Path::new(name).components() {
        let Component::Normal(part) = component else {
            return Err(PluginError::InvalidPackage);
        };
        current.push(part);
        if fs::symlink_metadata(&current)
            .map_err(|_| PluginError::InvalidPackage)?
            .file_type()
            .is_symlink()
        {
            return Err(PluginError::InvalidPackage);
        }
    }
    if !current.is_file() {
        return Err(PluginError::InvalidPackage);
    }
    Ok(current)
}
