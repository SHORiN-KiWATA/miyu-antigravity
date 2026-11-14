//! 怎么调 agy：命令行、环境、标准输入那一行（设计第二节「怎么调」）。只拼，不起。

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use serde_json::json;

use crate::config::Config;

/// `--print-timeout`：一整轮给一天，活不活靠核心的空闲时限判（设计第十节「整轮」）。
pub const PRINT_TIMEOUT: &str = "86400s";

/// 一问一答的参数：模型 `model`、代理 `agent`、`--add-dir` 给 `dir`。
#[must_use]
pub fn chat_args(model: &str, agent: &str, dir: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--print=",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--dangerously-skip-permissions",
        "--model",
        model,
        "--agent",
        agent,
        "--add-dir",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(dir.as_os_str().to_owned());
    args.push("--print-timeout".into());
    args.push(PRINT_TIMEOUT.into());
    args
}

/// 列模型的参数。
pub const MODELS_ARGS: [&str; 1] = ["models"];

/// 起 agy 的命令：程序照配置（没配的在 `PATH` 里找 `agy`），参数 `args`。环境（`HOME` 在内）照扩展自己的原样传下去：
/// 用哪个账号不归本包管。
#[must_use]
pub fn command(
    config: &Config,
    args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
) -> Command {
    let mut command = Command::new(config.program());
    command.args(args);
    command
}

/// 写进标准输入的一行：一轮人说的话 `text`，带换行。
#[must_use]
pub fn stdin_line(text: &str) -> String {
    stdin_texts(&[text])
}

/// 写进标准输入的一行：一轮人说的几段话 `texts`，一段一个 text 块，带换行。
#[must_use]
pub fn stdin_texts(texts: &[impl AsRef<str>]) -> String {
    let content: Vec<_> = texts
        .iter()
        .map(|text| json!({"type": "text", "text": text.as_ref()}))
        .collect();
    let mut line = json!({"event": "user", "message": {"content": content}}).to_string();
    line.push('\n');
    line
}

#[cfg(test)]
mod tests;
