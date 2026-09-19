//! 受管理员信任的外部插件进程；不是执行不可信代码的操作系统沙箱。

mod install;
mod manifest;
mod process;
mod registry;

pub use install::install_package;
pub use manifest::{PluginManifest, PluginPackage};
pub use process::PluginProcess;
pub use registry::{PluginConfig, PluginRegistry};

/// 错误不包含插件输出、环境变量或请求正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    #[error("invalid plugin package")]
    InvalidPackage,
    #[error("incompatible plugin API")]
    Incompatible,
    #[error("plugin I/O failed")]
    Io,
    #[error("plugin protocol failed")]
    Protocol,
    #[error("plugin deadline exceeded")]
    Timeout,
    #[error("plugin is stopped")]
    Stopped,
}
