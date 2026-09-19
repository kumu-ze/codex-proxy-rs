//! Provider 选择账号后、发送请求前的受限扩展合同；不暴露传输层和凭据。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionRequest {
    pub provider: String,
    pub account_id: String,
    pub model: String,
    pub credential_scope: String,
}

/// Provider 决定允许哪些字段；扩展不能任意覆盖请求头、认证或模型。
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionDecision {
    #[serde(default)]
    pub deny: bool,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("request extension unavailable")]
pub struct ExtensionUnavailable;

#[async_trait]
pub trait RequestExtension: Send + Sync {
    async fn before_send(
        &self,
        request: ExtensionRequest,
    ) -> Result<ExtensionDecision, ExtensionUnavailable>;
}
