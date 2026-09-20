# 插件接口参考（实验 API 1）

安装与运行方式见 [插件指南](plugins.md)，实现步骤见 [开发指南](plugin-development.md)。接口只对启用此实现的宿主有效，尚未冻结为长期兼容合同。

## Manifest

包根目录必须包含 plugin.json 和清单内文件：

```json
{
  "id":"example", "version":"1.0.0", "apiVersion":1,
  "executable":"worker", "files":{"worker":"<SHA256小写十六进制>"},
  "menuLabel":"示例", "capabilities":["ui.chat"],
  "author":"Publisher", "repository":"https://github.com/owner/plugin",
  "releaseAsset":"plugin-{version}-linux-x64.tar.gz"
}
```

必填：id（1–64 个小写字母/数字/连字符）、SemVer version、apiVersion=1、executable、files。清单上限64 KiB、256文件、文件合计128 MiB，executable必须在files中。拒绝路径穿越、链接与摘要不匹配。

可选：menuLabel（1–24字符）、capabilities（request.openai/provider.openai/ui.chat）、author（最多100字符，不含控制字符）、HTTPS repository（不得含用户名密码）、releaseAsset（含 `{version}` 的附件名称模板）。author/repository是发布者声明，不能替代签名或信任审查。旧包缺少这些字段仍可安装；未声明更新信息时不显示可用更新能力。

## 管理 HTTP API

统一使用宿主管理员鉴权、JSON envelope 与 no-store；未知 JSON 字段或动作由 AdminJson 返回422。

| 路径 | 方法与请求 | 返回 data |
| --- | --- | --- |
| /api/admin/plugins | GET | 插件列表 |
| /api/admin/plugins/invoke | POST `{id,method,input}` | 插件方法结果；只允许 admin.* |
| /api/admin/plugins/manage | POST `{action:"install",url,sha256?,proxyId?}` | 更新后的列表 |
| /api/admin/plugins/manage | POST `{action:"enable"或"disable"或"uninstall",id}` | 更新后的列表；action使用其中一个值 |
| /api/admin/plugins/check-update | POST `{id,proxyId?}` | 更新信息；只读 |

列表字段：id、version、enabled、available、menuLabel、capabilities、author、repository、updateSupported。

安装 url 支持 HTTP/HTTPS tar.gz 插件包直链，不支持仓库主页、源码 ZIP 或源码自动编译。可选择保存的代理 ID，认证由宿主解析，不回传浏览器。sha256 是可选的整包摘要；包内部文件仍必须通过 manifest 摘要校验。

安装后默认停用。相同ID已登记时拒绝重复安装；未登记的旧版本目录只有在完整清单、文件摘要一致且没有额外文件时才恢复登记，不覆盖旧内容。不同/损坏旧目录返回专门冲突提示。卸载要求先停用，保留业务数据。

check-update 返回：currentVersion、latestVersion（可能null）、updateAvailable、releaseUrl、downloadUrl、sha256（可能null）、prerelease。仅支持公开GitHub仓库及声明的releaseAsset模板，忽略草稿；预发行会明确标记。检查不自动替换程序，更新仍使用停用/卸载/安装流程。私有仓库认证、签名验证和自动升级尚未实现。

错误语义：404不存在；503忙碌/插件不可用；400细分输入、下载连接/超时/HTTP状态、校验、旧目录冲突、更新源、代理和存储失败。不返回原生插件错误正文、认证头或带签名的下载URL。

## 原生进程 RPC

stdin/stdout 帧：4字节大端长度 + UTF-8 JSON，每帧1 MiB上限。stdout不能混入日志。JSON-RPC 2.0请求/响应id必须一致，result/error互斥：

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"apiVersion":1,"pluginId":"example"}}
```

initialize必须原样返回params，握手5秒。宿主清空环境后传入RS_PLUGIN_DATA_DIR；声明provider.openai时还传入RS_PLUGIN_SERVICES_URL、RS_PLUGIN_SERVICES_TOKEN。程序工作目录是安装目录，数据目录独立；不得依赖继承的PATH。

admin.ui 输入 `{}`，返回 `{html:"..."}`；浏览器要求html.length≤262144，仍受RPC帧上限限制。其他admin.*由插件定义。每插件一条通信流，管理调用不排队、最长5秒；合法JSON-RPC error只结束该操作，超时/取消/损坏帧会淘汰进程。

## 请求扩展 request.openai

Provider选定账号并清理客户端身份后调用request.before_send，HTTP/WS共用：

```json
{"provider":"openai","accountId":"...","model":"...","credentialScope":"...","authenticationKind":"oauth","planType":"plus","accountEligible":true}
```

planType可为null；credentialScope由Provider绑定账号与真实凭据，不是令牌。回复 `{deny:false,values:{session_state:"..."}}`。最多8字段，key≤64/value≤4096；OpenAI只使用session_state并继续验证内容。多个插件覆盖同字段时报错。启用但故障的扩展不会静默放行；显式停用则跳过。每插件最多16个请求等待/执行，等锁与通信各500ms。

## Provider 回调 provider.openai

向RS_PLUGIN_SERVICES_URL POST `{method,input}`，Authorization使用RS_PLUGIN_SERVICES_TOKEN。每插件独立随机令牌，监听回环，64 KiB请求体、16并发、30秒上限。

| 方法 | input | 返回 |
| --- | --- | --- |
| accounts.list | `{}` | 账号视图数组 |
| accounts.get | `{accountId}` | 账号视图或null |
| responses.probe | `{accountId,model,proxy,credentialScope}` | `[httpStatus,state,retryAfterSeconds,message]` |
| network.exit | `{proxy}` | IP字符串 |
| proxies.list | `{page?:1}` | `{items:[{id,name,endpoint,hasAuthentication}],page:{totalPages}}`，每页100 |
| proxies.resolve | `{id}` | 可用代理URL字符串，可能含认证 |

账号视图：id、name、plan、eligible、binding、revision、authenticationKind；binding与plan可能null。responses.probe仅使用启用且Ready的OAuth账号，核对credentialScope，固定上游目标及诊断正文；网络失败返回回调错误；业务插件可自行投影为诊断状态0。不返回OAuth token或生成正文。network.exit是独立同源采样，不能证明某次业务请求出口。proxies.resolve仅供受信原生程序使用，禁止把认证地址发给UI。

## 页面消息桥

页面位于 sandbox="allow-scripts" iframe，无同源权限，CSP禁止直接网络和原生表单提交。宿主绑定event.source与当前插件，不接受页面传入目标插件ID。

原生管理方法：发送 `{type:"rs-plugin-call",id,method:"admin.xxx",input}`，接收 `{type:"rs-plugin-result",id,data}` 或error。调用只绑定当前插件。

声明ui.chat后可发送 `{type:"rs-plugin-host-call",id,method,input}`，回复type为rs-plugin-host-result：

| 方法 | input | data |
| --- | --- | --- |
| chat.context | `{}` | `{baseUrl,keys:[{id,name,prefix}]}`，只含已启用Key元数据 |
| chat.start | `{kind:"models",keyId}` 或 `{kind:"chat",keyId,model,messages:[{role,content}]}` | `{jobId}` |
| chat.poll | `{jobId}` | 当前任务状态 |
| chat.cancel | `{jobId}` | 取消后的状态 |

角色为user/assistant。任务状态包含state（running/completed/failed/cancelled）、text，以及可选html/error/models/httpStatus/requestId/elapsedMs/usage。html为marked解析并经DOMPurify白名单过滤的Markdown，禁止脚本、事件属性、表单、SVG和图片；插件仍须只信任parent返回值。密钥明文只在宿主页面使用，不进入iframe或原生RPC。

一页一个在途任务，保留8个结果；最多40条消息、单条16000字符、序列化消息64K字符，响应2 MiB/120秒。地址固定为当前同源/v1/models和/v1/responses，拒绝重定向；正常Key权限/分组/路由/计量继续生效。关闭页面取消本页任务，不撤销已产生用量。
