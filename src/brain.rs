//! agy 自己的转录里读原生工具的结果（设计第二节「原生工具的结果在哪」、第七节）。
//!
//! 文件：`<家目录>/.gemini/antigravity-cli/brain/<对话编号>/.system_generated/logs/transcript_full.jsonl`，一行一条 JSON；
//! 要的是 `step_index` 等于这一步的那一行的 `content`。原生工具（`search_web`）的 `content` 开头是两行时间
//! （`Created At: …`、`Completed At: …`），去掉，剩下的才是结果。
//!
//! 解析是纯的（[`result_in`]），读文件的是 [`read`]（一次）和 [`read_waiting`]（agy 写转录可能比流里的 `DONE` 晚一点，隔一会儿再
//! 读，到点还没有就算读不到）。只读，不写、不删转录。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::process::{self, Line};
use crate::workspace;

/// 转录还没有这一步时，隔多久再读。
pub const POLL: Duration = Duration::from_millis(50);

/// 转录还没有这一步时，至多等多久。
pub const WAIT: Duration = Duration::from_secs(1);

/// 对话 `conversation` 的转录文件在 `home` 下的位置；编号不合 [`workspace::valid_conversation`] 的（不拿来拼路径）是 `None`。
#[must_use]
pub fn path(home: &Path, conversation: &str) -> Option<PathBuf> {
    workspace::valid_conversation(conversation).then(|| {
        home.join(".gemini/antigravity-cli/brain")
            .join(conversation)
            .join(".system_generated/logs/transcript_full.jsonl")
    })
}

/// 去掉 `content` 开头的两行时间（`Created At: …`、`Completed At: …`）；不是这个开头的原样交。
#[must_use]
pub fn strip_times(content: &str) -> &str {
    let Some((first, rest)) = content.split_once('\n') else {
        return content;
    };
    let Some((second, body)) = rest.split_once('\n') else {
        return content;
    };
    if first.starts_with("Created At:") && second.starts_with("Completed At:") {
        body
    } else {
        content
    }
}

/// 一行转录是不是第 `step` 步的、带字的：是就交去掉时间以后的字（空的当没有）。
#[must_use]
pub fn result_of(line: &str, step: u64) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("step_index")?.as_u64()? != step {
        return None;
    }
    let body = strip_times(value.get("content")?.as_str()?);
    (!body.trim().is_empty()).then(|| body.to_string())
}

/// 从 `reader` 一行一行读转录，找第 `step` 步的结果；同一步有几行的取最后一条带字的。坏行（不是 JSON、太长）跳过；一行超过
/// [`process::LINE_LIMIT`] 的不读进内存，读到换行就丢掉。
///
/// # Errors
///
/// 读的时候出错：原样交。
pub fn result_in(reader: impl BufRead, step: u64) -> std::io::Result<Option<String>> {
    result_within(reader, step, process::LINE_LIMIT)
}

/// 同 [`result_in`]，一行的上限是 `limit` 字节。
fn result_within(
    mut reader: impl BufRead,
    step: u64,
    limit: usize,
) -> std::io::Result<Option<String>> {
    let mut found = None;
    while let Some(line) = process::next_line(&mut reader, limit)? {
        match line {
            Line::Text(text) => {
                if let Some(body) = result_of(&text, step) {
                    found = Some(body);
                }
            }
            Line::TooLong => skip_line(&mut reader)?,
        }
    }
    Ok(found)
}

/// 丢掉当前这一行剩下的字，读到换行（含）或文件尾为止。
fn skip_line(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let (used, done) = {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                return Ok(());
            }
            match buffer.iter().position(|byte| *byte == b'\n') {
                Some(at) => (at + 1, true),
                None => (buffer.len(), false),
            }
        };
        reader.consume(used);
        if done {
            return Ok(());
        }
    }
}

/// 读一次：转录里第 `step` 步的结果。文件没有、没有这一步、读不了的都是 `None`（读不了的记一行日志）。
#[must_use]
pub fn read(home: &Path, conversation: &str, step: u64) -> Option<String> {
    let path = path(home, conversation)?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!(
                "could not open agy's transcript {}: {error}",
                path.display()
            );
            return None;
        }
    };
    match result_in(BufReader::new(file), step) {
        Ok(found) => found,
        Err(error) => {
            eprintln!(
                "could not read agy's transcript {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// 同 [`read`]，读不到的隔 `poll` 再读，至多等 `wait`；到点还没有是 `None`。调用的人不能拿着锁调它。
#[must_use]
pub fn read_waiting(
    home: &Path,
    conversation: &str,
    step: u64,
    wait: Duration,
    poll: Duration,
) -> Option<String> {
    let until = Instant::now() + wait;
    loop {
        if let Some(found) = read(home, conversation, step) {
            return Some(found);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || path(home, conversation).is_none() {
            return None;
        }
        std::thread::sleep(poll.min(left));
    }
}

#[cfg(test)]
mod tests;
