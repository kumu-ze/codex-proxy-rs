# 插件参考实现

这些示例用于解释 kumu-ze fork 的插件合同，供上游 PR 讨论和本地复现。当前插件 API 仍属实验阶段。

| 示例 | 用途 | 主要调用与权限 |
| --- | --- | --- |
| [Echo](echo) | 最小外部进程，演示长度帧、initialize 和 admin.ui | stdin/stdout JSON-RPC、隔离页面的 admin.* 消息桥 |
| [对话测试](chat/README.md) | 自动识别当前站点、选择已有 Key、模型列表与多轮 Responses | ui.chat 能力；原生进程只提供页面，宿主页代发同源模型/对话请求，Key 明文不进入插件 |
| [Turn-State](https://github.com/kumu-ze/rs-turn-state-plugin) | 独立业务插件，演示调度、持久化、代理池和请求状态扩展 | request.openai、provider.openai；OAuth 由 Provider 保有，可信原生插件可取得代理地址 |

## PR 引用建议

可以引用本目录说明基础合同，再链接对话插件源码、[公开安装包](https://github.com/kumu-ze/codex-proxy-rs/releases?q=chat-test&expanded=true) 和 [验证记录](../../docs/plugins-validation.md)。对话示例演示页面和宿主能力如何交互，不代表已提供面向任意第三方代码的安全沙箱。

完整接口、故障行为、权限与已知限制见 [插件开发文档](../../docs/plugins.md)。示例代码和包只能证明记录中实际验证的场景；模型目录、账号授权与上游连通性仍由部署环境决定。未来 PR 应引用具体 commit 或 tag，以便复核。
