# miyu-antigravity

[中文](README.md)

The Miyu extension package `antigravity`. It registers the model driver `antigravity/antigravity` and hands every model
call of that provider to the local Antigravity CLI (`agy`), using the login and subscription quota you already have in agy.

agy is used as a model only. Tools such as reading files, running commands and editing files are still run by Miyu, under
Miyu's permissions, confirmations and undo. agy's own native tools are all turned off, except web search (`search_web`).

Linux and macOS only. The design and the measurements behind it are in [docs/00-设计.md](docs/00-设计.md) (Chinese).

## Requirements

- Antigravity CLI 1.4.0 or newer, logged in: run `agy` once in a terminal and check that it answers.
- A Miyu core that knows "hosted tools" (`hosted` under `[[driver]]` in the manifest). Older cores cannot install this package.

## Install and configure

```bash
miyu pkg install antigravity
```

Installing asks you to approve two capabilities: providing a model driver, and network access (agy talks to Google's services).

Then add a provider that uses this driver (an extension cannot change provider settings itself, so this step is yours). Pick
any id; `agy` is used below:

```bash
miyu config set providers.agy.driver antigravity/antigravity
miyu config set providers.agy.name Antigravity        # the name shown in the interface; optional
```

After that `/model` in a conversation lists its models (such as `agy/gemini-3.8-flash-low`): the list comes from `agy models`,
and the tier (low, medium, high, xhigh) is part of the model name. The terminal interface can do the same without commands:
`/config`, "Providers and models", press `a`, "Custom…", enter `agy` as the ID, switch "Interface" to
`antigravity/antigravity`, leave the address and the key empty, and save.

To give a model its own context window or reasoning effort, add a model entry to `~/.miyu/etc/config.toml`:

```toml
[providers.agy.models."gemini-4-hydrogen-high"]
window = 1000000        # agy does not report the window; write the model's real one if it matters
effort = "high"         # write it for high tiers: Miyu then waits longer for the first token, see "Good to know"
```

Amounts: Miyu knows model names such as `gemini-…` and `claude-…` and shows an amount for every turn at the API price from its
model catalog; what you actually spend is your agy subscription. To have it counted as 0, run
`miyu config set providers.agy.local true`.

Settings, on the "Packages" page of the settings:

| Setting | Key | Default | Meaning |
|---|---|---|---|
| Executable path | `antigravity.binary` | empty | Full path of agy (or of a wrapper script around it); a leading `~` is your home folder. Empty: look for `agy` on `PATH` |
| Process retention after a turn (seconds) | `antigravity.keep_seconds` | 600 | After a turn ends, how long its agy process is kept for the next turn of the same conversation. 0: not kept |
| Tool call timeout (seconds) | `antigravity.tool_wait_seconds` | 3600 | How long agy waits for a tool call it handed back to Miyu, including the time you take to confirm |

Which account is used is not this package's business: agy runs with Miyu's `HOME` and uses the login under `~/.gemini`.
"Executable path" may also point to your own wrapper script around agy (one that spreads work over several accounts, say): it has
to pass the arguments, standard input and standard output through unchanged. A wrapper that gives agy another `HOME` works too,
as long as `.gemini/config` under that home is the same folder as the real one (a symlink): the system prompt and the tool server
are written there. Wherever agy keeps its conversation files, this package follows it to read and remove them.

## What it writes into agy's configuration

agy takes its system prompt and tools only from its own configuration files, so this package writes two places under
`~/.gemini/config/` and nothing else:

- `agents/miyu-<hash>/agent.md`: Miyu's system prompt, one folder per prompt, removed after an hour without use.
- The entry named `miyu` in `mcp_config.json`: how agy finds this package's tool server. Only this entry is changed; servers
  you configured yourself stay as they are. If the file cannot be understood it is not overwritten, and that call fails.

When you run agy yourself you will see the `miyu-…` agents and the `miyu` MCP server; in an agy that Miyu did not start, the
server offers an empty tool list.

The conversation files agy keeps for each conversation (`conversations/` and `brain/` under `~/.gemini/antigravity-cli/`)
are deleted when the conversation is dropped.

Before uninstalling, clean those two places:

```bash
miyu antigravity unregister
miyu pkg remove antigravity
```

## Good to know

- **MCP servers and plugins you configured in agy are also available to the model inside Miyu.** agy's MCP configuration is
  global and this package cannot switch it off. Those tools are run by agy directly, outside Miyu's permissions and
  confirmations. What the package does is not hide them: every tool agy ran itself is shown as a "hosted tool". To keep the
  model from using one inside Miyu, disable it in agy: `agy mcp disable <name>`.
- **Web search is done by agy.** Miyu only shows what was searched and what came back. Reading web pages goes through Miyu's
  own tool.
- **agy does not stream its thinking**, so nothing arrives while it thinks. Measured on the highest tier (xhigh), a hard
  question took a minute and a half until the first token; Miyu treats 180 seconds of silence as a timeout and retries.
  Write the matching `effort` in the model entry of high tiers and the limit grows (twice for high, three times for xhigh).
- **agy adds about 3,400 tokens of its own to every request**, counted against your agy quota.
- **Images**: agy takes text only, so images are described by the vision model configured in Miyu first.
- **How turns connect**: the next turn of a conversation goes to the agy process that is still alive, and only the new
  messages are sent. When that is not possible (the core restarted, compaction, undo, a different model or persona, a tool
  interrupted and then more said), a new agy conversation is started and the whole history is sent as one text; beyond about
  170 KB the oldest turns are dropped.
- **At most 4 agy conversations are kept**, one agy process each; beyond that the least recently used one is stopped.
- **Quota, rate limits, not being logged in and content-policy blocks** are classified from agy's own words and reported to
  Miyu, which then follows its usual rules for pools and cooldowns.

## Troubleshooting

| What you see | Most likely |
|---|---|
| `agy (the Antigravity CLI) was not found on PATH` | agy is not on the `PATH` Miyu runs with: set `antigravity.binary` |
| A "not logged in" error | Run `agy` once in a terminal and log in |
| `cannot understand …/mcp_config.json` | That file is not valid JSON: fix it; this package will not overwrite it. 0.1.0 also took the empty file agy creates itself for this: upgrade to 0.1.1 |
| `agy called a Miyu tool, but its tool server never reached Miyu` | agy did not connect this package's tool server: check that `agy mcp list` shows `miyu` and that it is enabled |
| A tool ran and then the whole history was replayed | agy gave up waiting first, or the core restarted: expected, that call just costs more |

The package's own log is `var/log/antigravity/stderr` under the data root: which tools agy ran in each call, and which
conversations were dropped and restarted.

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test          # integration tests use a fake agy (tests/fake_agy.py, needs python3) and never touch the real one
```

License: AGPL-3.0-or-later.
