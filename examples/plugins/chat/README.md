# 对话测试插件 0.1.0

独立 Rust 原生插件，用 RS 标准 `/v1/models` 和 `/v1/responses` 验证真实请求链路，不直接调用 Provider、不需要 OAuth。适配 fork 插件 API 1。

## 下载

[安装包与 SHA256](https://github.com/kumu-ze/codex-proxy-rs/releases/tag/chat-test-v0.1.0)

URL 安装地址：`https://github.com/kumu-ze/codex-proxy-rs/releases/download/chat-test-v0.1.0/rs-chat-test-0.1.0-linux-x64.tar.gz`

## 使用

在“插件”页面安装并启用包，左侧打开“对话测试”。填入 **RS API 密钥页生成的 Client Key**，加载模型或手动输入模型名，输入消息并发送。需要 Key 所属分组中有可用账号。请求会消耗正常模型用量；可观察 RS 使用统计与打标插件行为。

默认连接同容器本机端口 8080，特殊部署可展开连接设置调整端口。只支持本机 HTTP，不允许任意地址转发。API Key 仅用于当前页面，不写磁盘；插件作业内存十分钟后在下一次 RPC 时清理。离开页面/点击停止会尽力取消，但不能撤销上游已处理的请求。

支持：多轮文本、模型列表、SSE 分片显示、取消、HTTP 错误、请求 ID、耗时、用量。暂不支持工具调用/附件/Markdown/WS。页面回复按纯文本显示，不能执行模型返回的 HTML。

## 构建

```sh
cargo build --release --locked
python3 scripts/package.py target/release/rs-plugin-chat-test dist/chat-test-0.1.0
python3 tests/workflow.py dist/chat-test-0.1.0/worker
```

产物为 `dist/rs-chat-test-0.1.0-linux-x64.tar.gz`，根目录包含 worker、plugin.json。不得打包密钥、对话或运行数据。Cargo.lock 和与宿主一致的 hyper-util 修复提交均已锁定。

## 验证范围

workflow.py 启动真实插件进程和本机模拟 HTTP/SSE 服务，覆盖握手、页面、模型、逐字节中文 UTF-8 分片、多轮正文、用量、错误脱敏、取消及无文件持久化。它不调用真实付费模型。源码不引用宿主内部 crate，manifest 不申请 provider.openai 或 request.openai 能力。
