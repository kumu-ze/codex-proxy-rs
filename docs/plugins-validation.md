# v3.12.1-plugin.2 验证与上游参考

日期：2026-09-20。对象为本 fork `codex/plugin-host` 分支的 `3.12.1-plugin.2` 变更；独立打标插件为 `0.2.1`。此记录区分本次运行证据、此前证据和未验证项，不宣称生产就绪。

## 本次实际通过

| 检查 | 命令 / 环境 | 结果与边界 |
| --- | --- | --- |
| 宿主插件 | `cargo test --offline --locked --manifest-path backend/Cargo.toml -p gateway-host --test main plugins -- --test-threads=1` | 17 项通过；Linux x86_64，真实子进程、局域网 HTTP 测试包服务 |
| 管理接口 | 同命令改为 `-p gateway-api` | 2 项通过，管理员鉴权、no-store、合法管理动作、未知动作和字段拒绝 |
| 静态检查 | `cargo clippy --offline --locked --manifest-path backend/Cargo.toml -p gateway-host -p gateway-api --all-targets -- -D warnings -A clippy::result_large_err` | 通过；result_large_err 为仓库既有例外，不表示零 lint 例外 |
| 架构 | `cargo test --offline --locked --manifest-path backend/Cargo.toml -p codex-proxy-rs -p gateway-api --test main architecture -- --test-threads=1` | 组合根 12 项、API 5 项通过 |
| Rust 格式 | `cargo fmt --all --manifest-path backend/Cargo.toml` | 已应用，变更文件经过格式化 |
| 前端 | 在 frontend 下运行 `node node_modules/eslint/bin/eslint.js .`、`node node_modules/vue-tsc/bin/vue-tsc.js -b --pretty false`、`node node_modules/vite/bin/vite.js build` | ESLint、类型检查与构建通过；最后手机布局修改后重跑相应文件 ESLint、类型和构建 |
| 打标独立进程 | 私有插件仓库 `python3 tests/workflow.py dist/turn-state-0.2.1/worker` | 保存、手动探测、身份门禁、代理导入、出口采样、日志、持续并发、按需自动、重启恢复通过；Provider 为模拟服务，不访问真实上游 |
| 部署 | 独立 Docker 验证实例、克隆数据库、保留历史迁移原字节的兼容构建 | healthz 204；版本 3.12.1-plugin.2；turn-state 0.2.1 可用；原生产实例未切换 |
| 浏览器 | Playwright CLI，实际部署的 HTTP 页面，1440×1000、390×844 | 从侧栏直接打开打标、停用隐藏菜单、容器重启后仍停用、重新启用恢复入口；URL 安装示例包默认停用，启用后打开页面，停用卸载，错误 SHA256 提示均通过 |

新增管理测试覆盖：旧配置数据目录与 URL 新包重叠拒绝、重复安装拒绝、安装失败临时目录清理、符号链接包拒绝、错误整包摘要、不支持协议和回环/链路本地地址拒绝、写注册表失败不改变运行状态、启停跨重启、卸载保留数据、相同 ID 再安装恢复数据、停用扩展跳过而启用故障不静默放行。

页面截图仅保留无账号凭据区域：

![插件管理](images/plugins/management.png)

![原生侧栏入口](images/plugins/sidebar.png)

![手机布局](images/plugins/mobile.png)

![深色管理页](images/plugins/dark.png)

## 已处理的验收问题

- API 无效 JSON 的实际合同是 422，测试最初期待 400，已按现有 AdminJson 行为修正；未修改 API 以迎合测试。
- 旧临时集成测试文件留在 NAS 构建树、未声明模块，导致架构检查失败；将临时文件移出生产测试树后 17 项通过，仓库未放宽规则。
- Windows 沙箱禁止 Vite 依赖启动子进程；在批准的宿主构建环境完成构建。
- 浏览器发现手机插件名称被操作按钮挤窄，调整名称最小宽度和操作行布局后重新构建并验收。
- 第一次部署产物因共享构建缓存混用了官方 16 条迁移，验证实例拒绝启动。先恢复旧镜像，再清理 gateway-store 的编译缓存，用历史 18 条迁移的只读挂载重新构建，将结果立即复制到独立部署产物后再打镜像。最终启动正常；未修改数据库、迁移内容或 checksum。这个兼容构建仅服务历史 fork 验证实例，不能充当官方数据库迁移方案。

## 继承的此前证据

plugin.1 阶段已跑过 Provider 的 HTTP/WS 状态替换、凭据过滤、身份摘要失效及真实完整打标插件的宿主握手/注入集成测试；本次没有重新执行所有 Provider 集成测试。当前变更未修改 Provider wire 逻辑，这些历史结果不等同于本版本生产上游实测。

## 尚未验证 / 尚未实现

- 未向真实 OpenAI 账号执行完整生产推理或自动打票验收；验证实例自动打标仍关闭。
- 未做高并发、长期稳定性、16 个插件同时运行、数据面/管理面争用的性能基准。
- 未穷尽 HTTP 重定向、DNS 重绑定、压缩炸弹和中途断连的网络故障矩阵；这些代码路径有防护但不等于已有全部测试覆盖。
- 未完成恶意原生插件隔离、子孙进程清理、CPU/内存限额、发布者签名、细粒度能力授权。
- 未完成数据库级管理员审计；当前仅记录管理成功的插件 ID 和动作。
- 未验证 Windows/macOS/ARM 原生插件、私有 GitHub 登录下载、自动更新及跨版本数据迁移。
- 插件 iframe 不自动继承宿主深色主题；本次深色截图验收的是宿主管理页。
- 未执行全仓 PostgreSQL/Redis 集成套件、完整 Cargo/pnpm 安全审计或正式生产回滚演练。
- registry.json 在普通文件失败时有原子替换，但没有断电事务日志、跨进程锁和自动垃圾回收。
- 当前验证镜像包含构建工具，使用开发 profile 的剥离二进制，不作为面向外部用户的生产发行镜像。

## 上游讨论建议

可独立评估三个边界：外部进程协议和失效行为、Provider 受限服务端口、固定路由与隔离页面。动态安装和原生代码信任策略可作为独立决策，不必为了某一业务插件一次接受全部框架。

合入前建议先确定 API 兼容策略、发行签名与平台元数据、权限粒度、管理员审计 owner、更新/回滚合同，以及请求扩展对时延与故障域的影响。此分支提供可运行实现与已知限制，不把原型实现作为已经冻结的上游设计。
