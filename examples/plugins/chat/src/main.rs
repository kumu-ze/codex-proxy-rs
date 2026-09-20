use futures::StreamExt as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};

const MAX_FRAME: usize = 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    api_key: String,
    #[serde(default = "default_port")]
    port: u16,
    kind: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    messages: Vec<Message>,
}
fn default_port() -> u16 {
    8080
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    role: String,
    content: String,
}
struct Job {
    result: Arc<Mutex<Value>>,
    task: tokio::task::JoinHandle<()>,
    created: Instant,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    let mut jobs = BTreeMap::<String, Job>::new();
    loop {
        let size = match input.read_u32().await {
            Ok(n) => n as usize,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if size == 0 || size > MAX_FRAME {
            return Err("invalid frame".into());
        }
        let mut bytes = vec![0; size];
        input.read_exact(&mut bytes).await?;
        let request: Value = serde_json::from_slice(&bytes)?;
        jobs.retain(|_, job| {
            if job.created.elapsed() > Duration::from_secs(600) {
                job.task.abort();
                false
            } else {
                true
            }
        });
        let params = &request["params"];
        let result: Result<Value, &str> = match request["method"].as_str() {
            Some("initialize") => Ok(params.clone()),
            Some("admin.ui") => Ok(json!({"html":include_str!("page.html")})),
            Some("admin.start") => {
                let parsed = serde_json::from_value::<Input>(params.clone());
                match parsed {
                    Ok(args)
                        if valid(&args)
                            && jobs.values().filter(|job| !job.task.is_finished()).count() < 4 =>
                    {
                        if jobs.len() >= 16 {
                            let oldest = jobs
                                .iter()
                                .filter(|(_, job)| job.task.is_finished())
                                .min_by_key(|(_, job)| job.created)
                                .map(|(id, _)| id.clone());
                            if let Some(id) = oldest {
                                jobs.remove(&id);
                            }
                        }
                        let id = uuid::Uuid::new_v4().to_string();
                        let result = Arc::new(Mutex::new(json!({"state":"running","text":""})));
                        let target = result.clone();
                        let task = tokio::spawn(async move {
                            let started = Instant::now();
                            let response = tokio::time::timeout(
                                Duration::from_secs(120),
                                run(&args, target.clone()),
                            )
                            .await;
                            let mut state = target.lock().await;
                            state["elapsedMs"] = json!(started.elapsed().as_millis() as u64);
                            match response {
                                Ok(Ok(())) => state["state"] = json!("completed"),
                                Ok(Err(error)) => {
                                    state["state"] = json!("failed");
                                    state["error"] =
                                        json!(error.replace(&args.api_key, "[redacted]"));
                                }
                                Err(_) => {
                                    state["state"] = json!("failed");
                                    state["error"] =
                                        json!("请求超过 120 秒，请检查账号或代理后重试");
                                }
                            }
                        });
                        jobs.insert(
                            id.clone(),
                            Job {
                                result,
                                task,
                                created: Instant::now(),
                            },
                        );
                        Ok(json!({"jobId":id}))
                    }
                    _ => Err("invalid request or too many active jobs"),
                }
            }
            Some("admin.poll") => match params["jobId"].as_str().and_then(|id| jobs.get(id)) {
                Some(job) => Ok(job.result.lock().await.clone()),
                None => Err("job not found"),
            },
            Some("admin.cancel") => match params["jobId"].as_str().and_then(|id| jobs.get(id)) {
                Some(job) => {
                    job.task.abort();
                    let mut state = job.result.lock().await;
                    if state["state"] == "running" {
                        state["state"] = json!("cancelled");
                    }
                    Ok(state.clone())
                }
                None => Err("job not found"),
            },
            _ => Err("unsupported method"),
        };
        let reply = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
            Err(message) => {
                json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32602,"message":message}})
            }
        };
        let body = serde_json::to_vec(&reply)?;
        output.write_u32(body.len() as u32).await?;
        output.write_all(&body).await?;
        output.flush().await?;
    }
    for job in jobs.values() {
        job.task.abort();
    }
    Ok(())
}
fn valid(args: &Input) -> bool {
    !args.api_key.trim().is_empty()
        && args.api_key.len() <= 512
        && !args.api_key.chars().any(char::is_control)
        && args.port != 0
        && matches!(args.kind.as_str(), "models" | "chat")
        && (args.kind == "models"
            || (!args.model.trim().is_empty()
                && args.model.len() <= 200
                && !args.messages.is_empty()
                && args.messages.len() <= 40))
        && args.messages.iter().all(|m| {
            matches!(m.role.as_str(), "user" | "assistant")
                && !m.content.is_empty()
                && m.content.len() <= 16000
        })
        && args.messages.iter().map(|m| m.content.len()).sum::<usize>() <= 64000
}
async fn run(args: &Input, state: Arc<Mutex<Value>>) -> Result<(), String> {
    // 仅回连本机 RS 的正常数据面，不提供任意 URL 转发，也不绕过 API Key 鉴权。
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| "无法初始化连接")?;
    let base = format!("http://127.0.0.1:{}", args.port);
    let request = if args.kind == "models" {
        client.get(format!("{base}/v1/models"))
    } else {
        let messages:Vec<Value>=args.messages.iter().map(|m|json!({"role":m.role,"content":[{"type":if m.role=="assistant" {"output_text"} else {"input_text"},"text":m.content}]})).collect();
        client.post(format!("{base}/v1/responses")).json(&json!({"model":args.model,"input":messages,"instructions":"You are a helpful assistant.","stream":true,"store":false}))
    };
    let response = request
        .bearer_auth(&args.api_key)
        .header("user-agent", "rs-plugin-chat-test/0.1.0")
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "连接 RS 超时"
            } else {
                "无法连接 RS，请检查服务端口"
            }
        })?;
    let status = response.status().as_u16();
    {
        let mut s = state.lock().await;
        s["httpStatus"] = json!(status);
        if let Some(id) = response
            .headers()
            .get("x-request-id")
            .and_then(|h| h.to_str().ok())
        {
            s["requestId"] = json!(id.chars().take(128).collect::<String>());
        }
    }
    let sse = response
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.contains("text/event-stream"));
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    let mut pending = Vec::<u8>::new();
    let mut complete = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "响应流中断，请检查 RS 请求日志")?;
        if body.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err("响应超过测试插件的 2 MB 上限".into());
        }
        body.extend_from_slice(&chunk);
        if sse && status < 400 {
            // 先缓冲完整 UTF-8 行；网络分片可以拆开汉字或 SSE 边界。
            pending.extend_from_slice(&chunk);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line =
                    String::from_utf8(pending[..end].to_vec()).map_err(|_| "响应不是有效 UTF-8")?;
                pending.drain(..=end);
                let line = line.trim_end_matches('\r');
                if let Some(data) = line.strip_prefix("data:") {
                    if data.trim() == "[DONE]" {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                        handle_event(&value, &state, &mut complete).await?;
                    }
                }
            }
        }
    }
    if status >= 400 {
        let message = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| {
                v["error"]["message"]
                    .as_str()
                    .map(|s| s.chars().take(800).collect::<String>())
            })
            .unwrap_or_else(|| match status {
                401 => "API Key 无效或已失效".into(),
                403 => "API Key 无权调用该模型".into(),
                _ => "请查看 RS 请求日志".into(),
            });
        return Err(format!("HTTP {status}：{message}"));
    }
    if !sse {
        let value: Value = serde_json::from_slice(&body).map_err(|_| "RS 返回了无法识别的响应")?;
        if args.kind == "models" {
            let models: Vec<&str> = value["data"]
                .as_array()
                .ok_or("模型列表格式异常")?
                .iter()
                .filter_map(|m| m["id"].as_str())
                .take(1000)
                .collect();
            state.lock().await["models"] = json!(models);
        } else {
            let mut s = state.lock().await;
            s["text"] = json!(response_text(&value));
            s["usage"] = value["usage"].clone();
        }
    } else if !complete {
        return Err("响应流未正常完成，请检查 RS 请求日志".into());
    }
    Ok(())
}
fn response_text(value: &Value) -> String {
    value["output"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|v| v["content"].as_array().into_iter().flatten())
        .filter_map(|v| v["text"].as_str())
        .collect::<String>()
        .chars()
        .take(64000)
        .collect()
}
async fn handle_event(
    value: &Value,
    state: &Arc<Mutex<Value>>,
    complete: &mut bool,
) -> Result<(), String> {
    match value["type"].as_str() {
        Some("response.output_text.delta") => {
            let mut s = state.lock().await;
            let mut text = s["text"].as_str().unwrap_or("").to_owned();
            text.push_str(value["delta"].as_str().unwrap_or(""));
            if text.len() > 64000 {
                return Err("回复超过 64 KB，请开始新对话".into());
            }
            s["text"] = json!(text);
        }
        Some("response.completed") => {
            *complete = true;
            let mut s = state.lock().await;
            if s["text"].as_str().unwrap_or("").is_empty() {
                s["text"] = json!(response_text(&value["response"]));
            }
            s["usage"] = value["response"]["usage"].clone();
            s["responseId"] = value["response"]["id"].clone();
        }
        Some("response.failed" | "response.incomplete" | "error") => {
            return Err(value["response"]["error"]["message"]
                .as_str()
                .or_else(|| value["error"]["message"].as_str())
                .unwrap_or("响应失败或未完成，请检查 RS 请求日志")
                .chars()
                .take(800)
                .collect());
        }
        _ => {}
    }
    Ok(())
}
