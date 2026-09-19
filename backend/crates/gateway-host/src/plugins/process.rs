use std::{path::Path, process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use super::{PluginError, PluginPackage};

const MAX_FRAME: usize = 1024 * 1024;

/// 每个进程只有一个待完成 RPC；调用者取消后，未消费的响应不能被下一请求误认。
pub struct PluginProcess {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    next_id: u64,
    pending: bool,
    stopped: bool,
}

impl PluginProcess {
    /// 子进程不继承宿主凭据环境。标准错误暂不进入普通日志，避免插件泄漏敏感数据。
    pub async fn start(
        package: &PluginPackage,
        data_directory: &Path,
        deadline: Duration,
    ) -> Result<Self, PluginError> {
        let data_directory = data_directory.canonicalize().map_err(|_| PluginError::Io)?;
        let mut child = Command::new(package.executable())
            .current_dir(package.root())
            .env_clear()
            .env("RS_PLUGIN_DATA_DIR", data_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| PluginError::Io)?;
        let input = child.stdin.take().ok_or(PluginError::Io)?;
        let output = child.stdout.take().ok_or(PluginError::Io)?;
        let mut process = Self {
            child,
            input,
            output,
            next_id: 1,
            pending: false,
            stopped: false,
        };
        let result = process
            .call(
                "initialize",
                json!({"apiVersion": 1, "pluginId": package.manifest().id}),
                deadline,
            )
            .await?;
        if result != json!({"apiVersion": 1, "pluginId": package.manifest().id}) {
            process.stop().await?;
            return Err(PluginError::Incompatible);
        }
        Ok(process)
    }

    /// 长度前缀帧：四字节大端长度，随后 UTF-8 JSON；超时后进程不可复用。
    pub async fn call(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, PluginError> {
        if self.stopped {
            return Err(PluginError::Stopped);
        }
        if self.pending {
            self.stop().await?;
            return Err(PluginError::Stopped);
        }
        if method.is_empty() || method.len() > 128 || deadline.is_zero() {
            return Err(PluginError::Protocol);
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(PluginError::Protocol)?;
        let body = serde_json::to_vec(
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        )
        .map_err(|_| PluginError::Protocol)?;
        if body.len() > MAX_FRAME {
            return Err(PluginError::Protocol);
        }
        self.pending = true;
        let result = tokio::time::timeout(deadline, self.exchange(id, &body))
            .await
            .unwrap_or(Err(PluginError::Timeout));
        if result.is_err() {
            let _ = self.stop().await;
        } else {
            self.pending = false;
        }
        result
    }

    async fn exchange(&mut self, id: u64, body: &[u8]) -> Result<Value, PluginError> {
        self.input
            .write_u32(body.len() as u32)
            .await
            .map_err(|_| PluginError::Io)?;
        self.input
            .write_all(body)
            .await
            .map_err(|_| PluginError::Io)?;
        self.input.flush().await.map_err(|_| PluginError::Io)?;
        let size = self.output.read_u32().await.map_err(|_| PluginError::Io)? as usize;
        if size == 0 || size > MAX_FRAME {
            return Err(PluginError::Protocol);
        }
        let mut buffer = vec![0; size];
        self.output
            .read_exact(&mut buffer)
            .await
            .map_err(|_| PluginError::Io)?;
        let mut reply: Value =
            serde_json::from_slice(&buffer).map_err(|_| PluginError::Protocol)?;
        if reply.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || reply.get("id").and_then(Value::as_u64) != Some(id)
            || reply.get("error").is_some()
            || reply.get("result").is_none()
        {
            return Err(PluginError::Protocol);
        }
        Ok(reply["result"].take())
    }

    pub async fn stop(&mut self) -> Result<(), PluginError> {
        self.stopped = true;
        self.child.start_kill().map_err(|_| PluginError::Io)?;
        tokio::time::timeout(Duration::from_secs(1), self.child.wait())
            .await
            .map_err(|_| PluginError::Timeout)?
            .map_err(|_| PluginError::Io)?;
        Ok(())
    }

    /// 在取得通信锁后查询；取消留下的半帧与已退出进程都不能再接收调用。
    pub fn is_available(&mut self) -> bool {
        !self.stopped && !self.pending && matches!(self.child.try_wait(), Ok(None))
    }
}
