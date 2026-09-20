# 插件开发指南

先阅读 [运行与安全边界](plugins.md) 和 [接口参考](plugin-api.md)。原生程序不是OS沙箱，只加载受信代码。

## 从最小示例开始

`examples/plugins/echo` 展示长度帧、initialize、admin.ui 和 admin.echo；不依赖宿主内部crate。运行 `cargo build --release --manifest-path examples/plugins/echo/Cargo.toml`，将二进制复制为包中的worker，并按接口参考生成plugin.json和文件SHA256。

包结构：

```text
package/
  plugin.json
  worker
```

`tar -czf example-0.1.0-linux-x64.tar.gz -C package .` 生成安装包。程序必须匹配目标平台；不要携带凭据、用户配置、运行数据或开发缓存。

## 实现步骤

1. stdin/stdout仅处理协议帧，实现initialize原样回显；将诊断与协议输出分开。
2. 需要页面时，实现admin.ui，返回内联脚本/样式的HTML；网络与宿主数据通过受限消息桥访问。
3. 长任务应快速返回任务ID，并设计有界轮询、取消和结果保留；管理RPC上限5秒。
4. 只声明实际使用的能力。request.openai用于账号选定后的请求处理，provider.openai提供受限服务；ui.chat提供同源对话页面桥。
5. 所有持久化文件放在RS_PLUGIN_DATA_DIR，使用原子写入与版本检查，避免后台刷新覆盖用户编辑。重启必须能恢复有效数据。
6. 在manifest声明作者与仓库；如需检测更新，发布SemVer tag的Release，并声明releaseAsset模板。安装时填写 Release 附件的 tar.gz 直链；仓库链接仅用于展示来源和检查更新。

## 完整参考插件

- [快捷对话](https://github.com/kumu-ze/rs-chat-test-plugin)：独立仓库，含接口/开发/安全文档。演示当前URL与已有Key选择、ui.chat、Markdown及打包发布。
- [Turn-State](https://github.com/kumu-ze/rs-turn-state-plugin)：业务插件示例，演示后台批次调度、并发配额、持久化、代理池及自动保存；其策略不内置到宿主。

## 验证清单与命令

```sh
cargo test --manifest-path backend/Cargo.toml -p gateway-host -p gateway-api --test main plugins -- --test-threads=1
cargo test --manifest-path backend/Cargo.toml -p provider-openai --test main extension
```

同时检查畸形帧、取消/超时后复用、非法路径、文件校验、重复/残留目录恢复、未授权管理请求、启停重启、配置写失败和数据保留。页面需在真实宿主iframe中验证，不能只打开HTML：检查来源绑定、忙碌/失败、窄屏、取消与不泄漏凭据。

请求扩展还应覆盖实际账号与凭据变化、HTTP/WS、多个插件写同字段及故障拒绝行为。Mock用于验证调度/协议，不能替代真实上游与负载验收。

## 版本与更新

API 1仍属实验合同。升级保留程序版本目录与独立数据目录；同ID不能并行注册多个版本。检查更新只读取Release，不自动执行新程序。当前更新步骤是停用、卸载、安装新包再启用，数据保留；提前备份，并确认数据格式兼容。未登记但完全一致的旧程序目录可恢复登记，不同内容会被拒绝而不是覆盖。
