use std::io::{self, Read as _, Write as _};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut prefix = [0u8; 4];
        if let Err(error) = input.read_exact(&mut prefix) {
            if error.kind() == io::ErrorKind::UnexpectedEof { return Ok(()) }
            return Err(error.into());
        }
        let length = u32::from_be_bytes(prefix) as usize;
        if length > 1024 * 1024 { return Err("oversized frame".into()); }
        let mut data = vec![0; length];
        input.read_exact(&mut data)?;
        let request: Value = serde_json::from_slice(&data)?;
        let result = match request["method"].as_str() {
            Some("initialize") => request["params"].clone(),
            Some("admin.ui") => json!({"html":include_str!("page.html")}),
            Some("admin.echo") => json!({"message":"插件通信成功", "input":request["params"]}),
            _ => Value::Null,
        };
        let body = serde_json::to_vec(&json!({"jsonrpc":"2.0", "id":request["id"], "result":result}))?;
        output.write_all(&(body.len() as u32).to_be_bytes())?;
        output.write_all(&body)?;
        output.flush()?;
    }
}
