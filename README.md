# miyu-antigravity

[English](README.en.md)

Miyu 的扩展包 `antigravity`：登记模型驱动 `antigravity/antigravity`，把这家供应商的每一次模型调用交给本机的
Antigravity CLI（`agy`），用你在 agy 里已有的登录和订阅额度。

agy 在这里只当模型用：读文件、跑命令、改文件这些工具照旧由 Miyu 执行，权限、确认、撤销都照 Miyu 的来。agy 自己的原生工具
全部关掉，只留联网搜索（`search_web`）。

只支持 Linux、macOS。设计和实测记录见 [docs/00-设计.md](docs/00-设计.md)。

## 要先有什么

- Antigravity CLI 1.4.0 或更新，已经登录：在终端里跑一次 `agy`，能正常对话就行。
- Miyu 核心要认得「供应商侧工具」（清单 `[[driver]]` 的 `hosted`）；太旧的核心装不上这个包。

## 装和配

```bash
miyu pkg install antigravity
```

装的时候会问你批不批它的两样能力：提供模型驱动、联网（agy 自己连 Google 的服务）。

然后在 `~/.miyu/etc/config.toml` 里手写一家供应商（扩展改不了配置）：

```toml
[providers.agy]
name = "Antigravity"
driver = "antigravity/antigravity"
local = true            # 用的是 agy 的订阅额度：写上 Miyu 才按 0 记账，不写会照模型目录里的价算钱

# 模型名照 `agy models` 的第一列写；档位（low、medium、high、xhigh）在模型名里。
[providers.agy.models."gemini-4-hydrogen-high"]
window = 1000000        # agy 不报窗口，照这个模型实际的写；不写的话 Miyu 不会提前压缩上下文
effort = "high"         # 高档位写上：Miyu 等第一个字的时限会跟着放宽，见「要知道的」

[models]
chat = "agy/gemini-3.8-flash-low"
```

价格不用写，但 `local = true` 要写：Miyu 认得 `gemini-…`、`claude-…` 这些模型名，不写的话会照模型目录里的价给每一轮算钱
（实际花的是 agy 的订阅额度）。

设置项在设置页「软件包」里填：

| 设置 | 默认 | 说明 |
|---|---|---|
| `antigravity.binary` | 空 | agy 的完整路径。空着在 `PATH` 里找 |
| `antigravity.keep_seconds` | 600 | 一段对话说完以后，agy 进程留着等下一轮多久 |
| `antigravity.tool_wait_seconds` | 3600 | agy 把一次工具调用交回 Miyu 后最多等多久。Miyu 执行工具、等你点确认的时间都算在里面 |

用哪个账号不归本包管：agy 照 Miyu 的 `HOME` 跑，用的是 `~/.gemini` 里的登录。

## 它会往 agy 的配置里写什么

agy 的系统提示词和工具只能经它自己的配置文件给，所以本包会写你 `~/.gemini/config/` 下的两处，别的不碰：

- `agents/miyu-<哈希>/agent.md`：Miyu 的系统提示词，一份提示词一个目录，一小时没用过的自动删。
- `mcp_config.json` 里叫 `miyu` 的那一条：agy 靠它找到本包的工具服务。只改这一条，你自己配的别的服务原样留着；文件读不懂时
  不覆盖，那一次调用报错。

你自己开 agy 时会看到 `miyu-…` 这些代理和 `miyu` 这个 MCP 服务，后者在不是 Miyu 拉起的 agy 里是一张空的工具表。

每段对话 agy 自己存的会话文件（`~/.gemini/antigravity-cli/` 下的 `conversations/`、`brain/`），对话收掉时本包一起删。

卸载前把这两处收拾掉：

```bash
miyu antigravity unregister
miyu pkg remove antigravity
```

## 要知道的

- **你在 agy 里自己配的 MCP 服务、插件，模型在 Miyu 里也调得到。** agy 的 MCP 配置是全局的，本包关不掉。这些工具由 agy
  直接执行，不过 Miyu 的权限和确认；本包做得到的是不瞒着：agy 自己跑了的每一件工具都照实显示成「供应商侧工具」。不想让
  模型在 Miyu 里用到的，在 agy 那边关：`agy mcp disable <名>`。
- **联网搜索是 agy 自己做的**，Miyu 只显示它搜了什么、搜到什么。读网页走 Miyu 自己的工具。
- **agy 不流思考的字**，想的时候没有任何输出。实测最高档位（xhigh）一道难题从发出到第一个字要一分半；Miyu 默认 180 秒没
  动静就判超时重试。高档位的模型在模型条目里写上对应的 `effort`，时限会放宽（high 两倍、xhigh 三倍）。
- **每次请求 agy 自己带约 3,400 token 的固定开销**，算在你的 agy 额度里。
- **图片**：agy 只收文字，图由 Miyu 配的看图模型转述以后再给它。
- **对话怎么接**：同一段对话的下一轮接在还活着的 agy 进程上，只发新的话。接不上的时候（核心重启、压缩、撤销、换了模型或
  人格、打断了工具又接着说）新开一段 agy 对话，把整份历史转写成一段文字发过去，超过 17 万字节的从最老的丢起。
- **同时至多留 4 段 agy 对话**，一段一个 agy 进程；多出来的收最久没用的。
- **额度、限流、没登录、内容策略拦截**照 agy 的原话归类报给 Miyu，换池、冷却照 Miyu 的规矩。

## 出了问题

| 现象 | 多半是 |
|---|---|
| `agy (the Antigravity CLI) was not found on PATH` | Miyu 的环境里找不到 agy：在设置里填 `antigravity.binary` |
| 报没登录 | 在终端里跑一次 `agy` 登录 |
| `cannot understand …/mcp_config.json` | 那份文件不是合法的 JSON：修好它，本包不会覆盖。0.1.0 把 agy 自己建的空文件也当成了这种，升到 0.1.1 |
| `agy called a Miyu tool, but its tool server never reached Miyu` | agy 没把本包的工具服务连上：看 `agy mcp list` 里有没有 `miyu`、是不是被关了 |
| 工具调了、Miyu 这边却重放了整段历史 | agy 那边先判了超时，或者核心重启过：属正常，只是这一次贵一些 |

本包自己的日志在数据根的 `var/log/antigravity/stderr`（每次调用里 agy 跑了哪件工具、哪段对话作废重开了都记在这）。

## 开发

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test          # 集成测试用假 agy（tests/fake_agy.py，要 python3），不碰真的 agy
```

许可证：AGPL-3.0-or-later。
