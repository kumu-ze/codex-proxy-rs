use serde_json::{Value, json};
use std::io::{self, Read as _, Write as _};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut prefix = [0u8; 4];
        if let Err(error) = input.read_exact(&mut prefix) {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(());
            }
            return Err(error.into());
        }
        let size = u32::from_be_bytes(prefix) as usize;
        if size == 0 || size > 1024 * 1024 {
            return Err("invalid frame".into());
        }
        let mut bytes = vec![0; size];
        input.read_exact(&mut bytes)?;
        let request: Value = serde_json::from_slice(&bytes)?;
        // 原生进程只提供页面；地址和 Key 由宿主页面管理，不经过插件 RPC。
        let result = match request["method"].as_str() {
            Some("initialize") => Some(request["params"].clone()),
            Some("admin.ui") => Some(json!({"html": include_str!("page.html")})),
            _ => None,
        };
        let reply = match result {
            Some(result) => json!({"jsonrpc":"2.0", "id":request["id"], "result":result}),
            None => {
                json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-32601,"message":"unsupported method"}})
            }
        };
        let bytes = serde_json::to_vec(&reply)?;
        output.write_all(&(bytes.len() as u32).to_be_bytes())?;
        output.write_all(&bytes)?;
        output.flush()?;
    }
}
