# miyu-antigravity

Miyu 的扩展包 `antigravity`：登记模型驱动 `antigravity/antigravity`，把这家供应商的每一次调用交给本机的
Antigravity CLI（`agy`），用你在 agy 里已有的登录和订阅额度。设计见 [docs/00-设计.md](docs/00-设计.md)。

现在做到 AG-1：握手、`driver.models`（照 `agy models` 列）、一问一答的 `driver.call`——每次新开一段 agy 对话，整份
历史转写成一条文字发过去，回复照增量流回来。还没有工具，agy 的原生工具全关。只支持 Linux、macOS。

供应商条目照这样手写（模型名照 `agy models` 的第一列；价格写 0，用的是 agy 的订阅额度）：

```toml
[providers.agy]
driver = "antigravity/antigravity"

[models]
chat = "agy/gemini-3.8-flash-low"
```

两个配置项在设置页「软件包」里填：`antigravity.binary`（agy 在哪，空着在 `PATH` 里找）、`antigravity.home`
（用哪个家目录跑 agy，登录在它的 `.gemini` 里；空着用真的家目录）。

许可证：AGPL-3.0-or-later。
