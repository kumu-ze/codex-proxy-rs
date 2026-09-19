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
