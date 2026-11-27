//! 怎么调 agy：命令行、环境、标准输入那一行（设计第二节「怎么调」）。只拼，不起。

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use serde_json::json;

use crate::config::Config;

/// `--print-timeout`：一整轮给一天，活不活靠核心的空闲时限判（设计第十节「整轮」）。
pub const PRINT_TIMEOUT: &str = "86400s";

/// 一问一答的参数：模型 `model`、代理 `agent`、`--add-dir` 给 `dir`。
///
/// 带 `--disable-slash-commands`（AG-14 补，2026-11-27）：交给 agy 的是 Miyu 拼好的对话，不是人在 agy 里打的命令，里面的
/// `/goal`、`/plan` 这类斜杠命令和技能不该被 agy 展开。主人群里有人发了「@Miyu /goal …」，agy 把它当成自己的目标模式：
/// 不写 `<!-- GOAL_COMPLETE -->` 就逼着接着干，她此后每句后面都带这一段，发进了群里；那一句留在历史里，重放时又触发。
#[must_use]
pub fn chat_args(model: &str, agent: &str, dir: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--print=",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--dangerously-skip-permissions",
        "--disable-slash-commands",
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

/// agy 一条输入至多收多少字节的字：超过的从尾部悄悄截掉、照样报成功。算在解出来的字上，不是算在标准输入那一行的字节上
/// （2026-11-18 真机：字 200 KB 的尾部没了；字 170 KB、全是引号换行、那一行 297 KB 的首尾都在）。
pub const TEXT_LIMIT: usize = 192_000;

/// 这几段字加起来超过 [`TEXT_LIMIT`] 的，交一共多少字节（好记一行日志）；没超的是 `None`。
#[must_use]
pub fn over_limit(texts: &[impl AsRef<str>]) -> Option<usize> {
    let total: usize = texts.iter().map(|text| text.as_ref().len()).sum();
    (total > TEXT_LIMIT).then_some(total)
}

/// 记一行日志：这一次（编号 `call`）发给 agy 的字超过了它收的上限，尾部会被它截掉。没超的不记。
pub fn warn_long(call: &str, texts: &[impl AsRef<str>]) {
    if let Some(total) = over_limit(texts) {
        eprintln!(
            "call {call}: this turn's input is {total} bytes; agy silently cuts input over {TEXT_LIMIT} bytes at the tail"
        );
    }
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
