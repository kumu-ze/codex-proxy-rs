use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{PluginError, PluginPackage};

/// 仅复制 manifest 列出的文件；发布到新目录后再由管理员配置启用。
/// 已存在版本拒绝覆盖，更新与回退通过切换目录完成。
pub fn install_package(source: &Path, destination: &Path) -> Result<PluginPackage, PluginError> {
    let package = PluginPackage::open(source)?;
    fs::create_dir_all(destination).map_err(|_| PluginError::Io)?;
    let destination = destination.canonicalize().map_err(|_| PluginError::Io)?;
    let final_path = destination.join(format!(
        "{}-{}",
        package.manifest().id,
        package.manifest().version
    ));
    if final_path.exists() {
        return Err(PluginError::InvalidPackage);
    }
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PluginError::Io)?
        .as_nanos();
    let temporary = destination.join(format!(".install-{}-{suffix}", std::process::id()));
    fs::create_dir(&temporary).map_err(|_| PluginError::Io)?;
    let result = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))
                .map_err(|_| PluginError::Io)?;
        }
        for name in package.manifest().files.keys() {
            let target = temporary.join(name);
            fs::create_dir_all(target.parent().ok_or(PluginError::InvalidPackage)?)
                .map_err(|_| PluginError::Io)?;
            fs::copy(package.root().join(name), &target).map_err(|_| PluginError::Io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mode = if name == &package.manifest().executable {
                    0o700
                } else {
                    0o600
                };
                fs::set_permissions(&target, fs::Permissions::from_mode(mode))
                    .map_err(|_| PluginError::Io)?;
            }
        }
        fs::write(
            temporary.join("plugin.json"),
            serde_json::to_vec(package.manifest()).map_err(|_| PluginError::InvalidPackage)?,
        )
        .map_err(|_| PluginError::Io)?;
        PluginPackage::open(&temporary)?;
        // 目标版本必须是新目录；安装目录只允许管理员写入。
        if final_path.exists() {
            return Err(PluginError::InvalidPackage);
        }
        fs::rename(&temporary, &final_path).map_err(|_| PluginError::Io)?;
        PluginPackage::open(&final_path)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

/// 仅用于未登记 ID 的恢复：不覆盖任何旧文件，也不复用带额外文件的目录。
pub(super) fn restore_or_install_package(
    source: &Path,
    destination: &Path,
) -> Result<(PluginPackage, bool), PluginError> {
    let requested = PluginPackage::open(source)?;
    let target = destination.join(format!(
        "{}-{}",
        requested.manifest().id,
        requested.manifest().version
    ));
    match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return install_package(source, destination).map(|package| (package, true));
        }
        Err(_) => return Err(PluginError::Io),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(PluginError::ExistingPackage);
        }
        Ok(_) => {}
    }
    let existing = PluginPackage::open(&target).map_err(|_| PluginError::ExistingPackage)?;
    if existing.manifest() != requested.manifest() {
        return Err(PluginError::ExistingPackage);
    }
    let expected: std::collections::BTreeSet<std::path::PathBuf> = requested
        .manifest()
        .files
        .keys()
        .map(|name| Path::new(name).components().collect())
        .chain(std::iter::once(std::path::PathBuf::from("plugin.json")))
        .collect();
    let mut directories = vec![existing.root().to_path_buf()];
    let mut visited = 0;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).map_err(|_| PluginError::Io)? {
            let entry = entry.map_err(|_| PluginError::Io)?;
            visited += 1;
            if visited > 4096 {
                return Err(PluginError::ExistingPackage);
            }
            let kind = entry.file_type().map_err(|_| PluginError::Io)?;
            if kind.is_dir() {
                directories.push(entry.path());
            } else if !kind.is_file()
                || !expected.contains(
                    entry
                        .path()
                        .strip_prefix(existing.root())
                        .map_err(|_| PluginError::ExistingPackage)?,
                )
            {
                return Err(PluginError::ExistingPackage);
            }
        }
    }
    Ok((existing, false))
}
