# 对话测试插件 0.2.1

最低宿主 fork **3.12.1-plugin.4**，推荐 **3.12.1-plugin.5**。API 1 manifest 声明 `ui.chat`，宿主负责地址、Key 选择与正常 Responses 请求；原生程序只提供隔离页面。

## 下载

[安装包与校验值](https://github.com/kumu-ze/codex-proxy-rs/releases/tag/chat-test-v0.2.1)

URL 安装地址：`https://github.com/kumu-ze/codex-proxy-rs/releases/download/chat-test-v0.2.1/rs-chat-test-0.2.1-linux-x64.tar.gz`

## 使用

启用插件后从侧栏打开“对话测试”。页面自动识别当前站点地址，并读取已启用的 Client API Key 名称与前缀：只有一个时自动选中，多个时下拉选择。点击加载模型，或手动填写模型名后发送。无需复制 Key 或配置内部端口；至少需要选定 Key 所属范围内有可用账号。

页面的“API 地址”显示当前站点加 `/v1`；实际请求始终使用 `/v1/models` 和 `/v1/responses`，不会重复拼接 `/v1`。

请求使用浏览器当前同源 `/v1/models`、`/v1/responses`；开发预览沿用宿主 `/dev` 代理前缀。保留正常鉴权、路由、计量和请求扩展行为。真实调用会消耗模型用量。

Key 明文由宿主管理页按选定 ID 获取，仅用于该次 fetch 的 Authorization，不进入 iframe、原生插件 RPC、磁盘或 localStorage。对话仅留页面内存；关闭页面会取消未完成的本页请求。宿主正常计量与请求诊断日志仍按配置记录。手动停止不能撤回上游已执行用量。

## 权限

`ui.chat` 允许可信插件页面列出已启用 Key 的 ID/名称/前缀，并使用选定 ID 发起同源模型/对话请求；它不返回 Key 明文。宿主每次开始任务前检查插件仍启用且声明此权限，拒绝外部 URL、自定义请求头与重定向。未声明此权限的插件不能使用该页面桥。此授权仍有模型用量影响，只安装可信代码。

支持多轮文本、SSE 逐步回复、停止、HTTP 状态/请求 ID/耗时/用量。暂不支持工具、附件、Markdown 或 WebSocket。回复按纯文本渲染。

## 构建和验证

```sh
cargo build --release --locked
python3 scripts/package.py target/release/rs-plugin-chat-test dist/chat-test-0.2.1
python3 tests/workflow.py dist/chat-test-0.2.1/worker
```

附件仅含 worker 和 plugin.json，源码只依赖 serde_json。进程测试覆盖握手、页面以及拒绝旧的密钥 RPC；同源请求、Key 选择、SSE、取消和权限隔离由宿主浏览器验收覆盖，结果见宿主 docs/plugins-validation.md。

0.1.0 的手动 Key/内部端口模式已被替换；0.2.1 不能用于尚不识别 ui.chat 的旧宿主。
