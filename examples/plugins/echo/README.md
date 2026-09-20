# 最小 Echo 插件

本例演示4字节大端长度帧、initialize握手、admin.ui页面和admin.echo调用，不申请Provider或对话能力。

```sh
cargo build --release --manifest-path examples/plugins/echo/Cargo.toml
mkdir -p /tmp/rs-echo-package
cp examples/plugins/echo/target/release/rs-plugin-echo /tmp/rs-echo-package/worker
```

为worker计算SHA256，按 [Manifest合同](../../../docs/plugin-api.md#manifest) 写plugin.json（id为example，version为0.1.0，executable为worker）。然后 `tar -czf example-0.1.0-linux-x64.tar.gz -C /tmp/rs-echo-package .`，通过管理页面的压缩包直链安装或使用CLI安装目录。不要把运行数据加入包。

需要完整交互与发布示例时，参见 [独立快捷对话插件](https://github.com/kumu-ze/rs-chat-test-plugin) 和 [开发指南](../../../docs/plugin-development.md)。
