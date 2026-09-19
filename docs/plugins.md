# 外部进程插件（实验实现）

当前实现为插件框架原型，包含安装、进程管理、管理员接口、隔离页面和初步请求扩展，尚未完成 Turn-State 插件抽取。
接口尚未冻结，不应直接替换生产版本。

## 安装与启用

仅安装管理员信任的本地原生程序。独立进程和清空环境变量不是操作系统沙箱；插件仍拥有运行 RS 的系统用户权限。

包包含 `plugin.json` 与清单列出的文件。例如：

```json
{
  "id": "example",
  "version": "1.0.0",
  "apiVersion": 1,
  "executable": "worker",
  "files": { "worker": "<文件内容的 SHA-256 小写十六进制摘要>" }
}
```

运行 `codex-proxy-rs plugin-install <包目录> <安装目录>`。
安装只复制已登记文件并复核摘要，不自动执行；同一 ID 和版本拒绝覆盖。
摘要用于发现内容改变，不能证明发布者身份。

在配置文件中明确添加插件，路径相对于配置文件：

```yaml
plugins:
  - directory: ../plugins/example-1.0.0
    data_directory: ../plugin-data/example
```

重启后启用。移除配置并重启停用，数据保留；版本升级通过安装新目录再修改配置完成。
所有插件的数据目录必须彼此独立，不能嵌套，也不能与任何插件程序目录重叠。
宿主在启动插件前检查全部目录。程序目录和配置仅允许管理员修改。

## 通信

stdin/stdout 仅传输协议帧，禁止在 stdout 输出日志。每帧由 4 字节大端长度和 UTF-8 JSON 组成，最大 1 MiB。
宿主发送 JSON-RPC 2.0 请求，插件回复相同 ID 的 result。当前不支持插件主动回调宿主。

首次调用 `initialize`，参数为 `{ "apiVersion": 1, "pluginId": "example" }`；插件应原样返回此对象。
握手限时 5 秒。进程不继承宿主环境，只显式传入 `RS_PLUGIN_DATA_DIR`；原生插件不得依赖 PATH 查找解释器。

管理入口：

- `GET /api/admin/plugins`：已配置插件 ID、版本和可用标记。
- `POST /api/admin/plugins/invoke`：`{ "id": "example", "method": "admin.example", "input": {} }`。

接口复用管理员鉴权和 no-store。仅允许 `admin.` 方法；不允许调用握手及内部数据面方法。
每个插件只允许一个同时进行的管理调用，不建立等待队列；最多 5 秒。
协议失败、超时或取消后不复用未完成的通信流；插件失效需要重新启动宿主恢复。
合法 JSON-RPC error（整数 code 和字符串 message，不能同时包含 result）只拒绝本次操作，进程仍可用。
管理接口返回固定错误文案及 HTTP 400，不转发插件错误正文；请求扩展拒绝时仍阻止本次业务请求。
插件返回值只向管理员提供，不能把原始响应写入公开日志。
读取列表时会检查空闲进程是否已退出，或上次通信是否被取消；这些情况显示为不可用。
关闭时先拒绝所有新调用，再并发终止插件；等待通信锁最多 6 秒，回收子进程最多 1 秒，数据保留。

## 管理页面

侧栏「插件」展示已配置插件。点击打开后，宿主调用 `admin.ui`，插件返回 `{ "html": "..." }`。
页面运行在只允许脚本的 sandbox iframe 中，不具有宿主同源权限；内容安全策略禁止直接网络访问。
页面不得携带真实账号凭据。

iframe 通过 `parent.postMessage({type: 'rs-plugin-call', id: '唯一请求ID', method: 'admin.example', input: {}}, '*')` 发起调用。
父页面核对消息来源窗口，只能调用当前打开插件的 `admin.` 方法，并向相同窗口返回 `rs-plugin-result`（data 或 error）。
来源是不透明的 sandbox origin，因此消息目标为 `*`，但发送窗口和接收窗口均固定，不能用消息参数切换插件 ID。

`examples/plugins/echo` 提供可独立编译的原生进程与页面示例。

## 请求扩展原型

manifest 可声明 `capabilities: ["request.openai"]`，在实际账号选择与身份清理后接收 `request.before_send`。
参数包含 provider、accountId、model、credentialScope，不包含请求正文或认证材料。
credentialScope 由 Provider 按账号和认证材料计算；OAuth 同时绑定上游账号、用户及 AT/RT/ID Token。
名称、套餐与非认证配置的 revision 更新不改变绑定，重新授权或身份变化则使旧状态失效。
插件返回 `{ "deny": false, "values": { "session_state": "..." } }`。当前 OpenAI adapter 仅允许 session_state 字段，并校验大小与字符集。
多个插件修改同一字段时拒绝请求，避免加载顺序隐式改变结果。启用的扩展不可用时拒绝调用，不能静默绕过保护策略。
每个插件最多 16 个数据面调用等待/执行，等待和通信分别限时 500ms；管理调用仍无等待队列。
这一接口仍需完成账号认证代次、HTTP/WS、探测与故障恢复的完整验收后才能冻结。

## 尚待完成

- 原生程序信任、平台兼容和权限声明的完整安装合同。
- 持久化启停、审计、健康探测及有界退出监督。
- Provider 作用域、探测能力、请求前扩展及 HTTP/WS 一致行为。
- Turn-State 插件抽取、旧数据导入、性能对比和端到端回滚测试。

