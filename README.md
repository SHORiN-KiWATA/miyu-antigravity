# miyu-antigravity

Miyu 的扩展包 `antigravity`：登记模型驱动 `antigravity/antigravity`，把这家供应商的每一次调用交给本机的
Antigravity CLI（`agy`），用你在 agy 里已有的登录和订阅额度。设计见 [docs/00-设计.md](docs/00-设计.md)。

现在做到 AG-2：握手、`driver.models`（照 `agy models` 列）、`driver.call`——同一段对话的下一轮接在还活着的那个 agy
进程上，只把新的话写进去；认不上的（第一次、重新生成、换了模型或系统提示词、夹着工具结果）新开一段 agy 对话，整份历史
转写成一条文字发过去。回复照增量流回来。出了错、被取消的那段 agy 对话作废；闲着的 agy 到点收掉，至多留 4 段。还没有
工具，agy 的原生工具全关。只支持 Linux、macOS。

供应商条目照这样手写（模型名照 `agy models` 的第一列；价格写 0，用的是 agy 的订阅额度）：

```toml
[providers.agy]
driver = "antigravity/antigravity"

[models]
chat = "agy/gemini-3.8-flash-low"
```

配置项在设置页「软件包」里填：`antigravity.binary`（agy 在哪，空着在 `PATH` 里找）、`antigravity.keep_seconds`（一段
对话说完以后 agy 进程留着等下一轮多久，默认 600 秒）。用哪个账号不归本包管：agy 照 Miyu 拉起扩展时的 `HOME` 跑，用的是那个家目录下 `.gemini` 里的登录。

许可证：AGPL-3.0-or-later。
