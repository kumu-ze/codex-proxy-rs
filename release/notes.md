# v3.12.1-plugin.6

实验插件系统新增作者、源码仓库和检查更新。检查公开GitHub Release并显示版本/附件，不自动替换程序；安装仍使用tar.gz直链。

修复未登记的旧程序目录导致“包校验失败”：清单、文件摘要完全一致且无额外文件时恢复登记，不覆盖已有数据；不同或损坏目录明确报冲突。

ui.chat回复增加经marked和DOMPurify白名单过滤的Markdown HTML。独立快捷对话参考插件0.3.0支持标题、表格、列表、代码块和复制；Key明文仍不进入插件。开发与接口文档见docs/plugin-development.md和docs/plugin-api.md。
