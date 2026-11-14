//! 把统一请求的 `messages` 整份转写成发给 agy 的一条文字（设计第五节「新开对话时的重放」）。纯函数。
//!
//! 格式照旧版 Miyu 的 `cli_relay/payload.rs`：一段说明加 `<conversation-history>`，里面照先后写 `User:`、`Assistant:`、
//! `[called tool NAME with ARGS]`、`[tool result]`；最后一轮人说的话（结尾连着的几条 user 消息）单独成块跟在后面。只有这
//! 一轮、没有历史的，直接发那几条的字。`request.tail` 有的接在最后。
//!
//! 整条不超过 [`BUDGET`] 字节：超了从最老的回合丢起、写一句省略；最后一轮不砍。给模型看的字在 [`crate::words`]。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::words;

/// 一条输入至多多少字节：agy 超过 192,000 字节的从尾部悄悄截断（设计第二节「输入上限」），打九折。
pub const BUDGET: usize = 172_800;

/// 请求的形状不对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadRequest(pub String);

impl std::fmt::Display for BadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bad request: {}", self.0)
    }
}

impl std::error::Error for BadRequest {}

/// 转写 `request`（统一请求的 JSON），整条至多 `budget` 字节（最后一轮自己就超了的照发、历史全丢）。
///
/// # Errors
///
/// `messages` 不是数组、一条消息不是对象或 `role` 不是 `user`、`assistant`、`tool`、`blocks` 不是数组：[`BadRequest`]。
pub fn transcribe(request: &Value, budget: usize) -> Result<String, BadRequest> {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| BadRequest("request.messages is not an array".into()))?;
    let described: BTreeMap<&str, &str> = request
        .get("described")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(blob, text)| Some((blob.as_str(), text.as_str()?)))
                .collect()
        })
        .unwrap_or_default();

    let mut parsed = Vec::with_capacity(messages.len());
    for message in messages {
        parsed.push(Message::read(message, &described)?);
    }
    // 最后一轮：结尾连着的 user 消息；之前的都是历史。
    let live_from = parsed
        .iter()
        .rposition(|message| message.role != Role::User)
        .map_or(0, |index| index + 1);
    let (history, live) = parsed.split_at(live_from);

    let mut last = live
        .iter()
        .map(|message| message.body.as_str())
        .filter(|body| !body.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if last.is_empty() && (!history.is_empty() || request.get("tail").is_none()) {
        last = words::CONTINUE.to_string();
    }
    if let Some(tail) = request.get("tail").and_then(Value::as_str) {
        if !last.is_empty() {
            last.push_str("\n\n");
        }
        last.push_str(tail);
    }
    if history.is_empty() {
        return Ok(last);
    }

    let lines: Vec<String> = history.iter().map(Message::history_line).collect();
    let fixed = words::HISTORY_HEAD.len() + words::HISTORY_FOOT.len() + 2 + last.len();
    let whole: usize = lines.iter().map(String::len).sum();
    let keep_from = if fixed + whole <= budget {
        0
    } else {
        oldest_kept(
            &lines,
            history,
            budget.saturating_sub(fixed + words::OMITTED.len()),
        )
    };

    let mut text = String::from(words::HISTORY_HEAD);
    if keep_from > 0 {
        text.push_str(words::OMITTED);
    }
    for line in &lines[keep_from..] {
        text.push_str(line);
    }
    text.push_str(words::HISTORY_FOOT);
    text.push_str("\n\n");
    text.push_str(&last);
    Ok(text)
}

/// 历史从哪一条起留：从最新往回加，放不下 `room` 就停；再往后挪到下一条 user 消息，让转写从一轮的开头起（截在回复、工具
/// 结果中间会让模型把半截旧回复当上文）。一条都放不下的交 `lines.len()`。
fn oldest_kept(lines: &[String], history: &[Message], room: usize) -> usize {
    let mut used = 0;
    let mut keep_from = lines.len();
    for (index, line) in lines.iter().enumerate().rev() {
        if used + line.len() > room {
            break;
        }
        used += line.len();
        keep_from = index;
    }
    history[keep_from..]
        .iter()
        .position(|message| message.role == Role::User)
        .map_or(lines.len(), |offset| keep_from + offset)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    Tool { failed: bool },
}

/// 一条消息：谁说的，块转成的字（块之间换行）。
struct Message {
    role: Role,
    body: String,
}

impl Message {
    fn read(message: &Value, described: &BTreeMap<&str, &str>) -> Result<Message, BadRequest> {
        let role = match message.get("role").and_then(Value::as_str) {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            Some("tool") => Role::Tool {
                failed: message.get("error").and_then(Value::as_bool) == Some(true),
            },
            other => {
                return Err(BadRequest(format!(
                    "a message has role {other:?}, not user, assistant or tool"
                )));
            }
        };
        let blocks = message
            .get("blocks")
            .and_then(Value::as_array)
            .ok_or_else(|| BadRequest("a message has no blocks array".into()))?;
        let body = blocks
            .iter()
            .filter_map(|block| block_text(block, described))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Message { role, body })
    }

    fn history_line(&self) -> String {
        let label = match self.role {
            Role::User => words::USER,
            Role::Assistant => words::ASSISTANT,
            Role::Tool { failed: false } => words::TOOL_RESULT,
            Role::Tool { failed: true } => words::TOOL_FAILED,
        };
        format!("{label}{}\n\n", self.body)
    }
}

/// 一块转成的字；不写的（思考、空的字、不认识的块）是 `None`。
fn block_text(block: &Value, described: &BTreeMap<&str, &str>) -> Option<String> {
    let field = |key: &str| block.get(key).and_then(Value::as_str);
    match field("type")? {
        "text" => field("text")
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        "image" => Some(match field("blob").and_then(|blob| described.get(blob)) {
            Some(description) => words::image_described(field("name"), description),
            None => words::image_omitted(field("name")),
        }),
        "file" => Some(words::file_omitted(field("name").unwrap_or_default())),
        "path" => Some(words::path_attached(
            field("name").unwrap_or_default(),
            field("path").unwrap_or_default(),
        )),
        "tool_call" => Some(words::called_tool(
            field("name").unwrap_or_default(),
            field("args").unwrap_or_default(),
        )),
        "hosted_tool" => {
            let mut text = words::provider_tool(
                field("name").unwrap_or_default(),
                field("args").unwrap_or_default(),
            );
            if let Some(outcome) = block.get("outcome") {
                let failed = outcome.get("failed").and_then(Value::as_bool) == Some(true);
                let said = outcome
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                text.push('\n');
                text.push_str(&words::provider_result(said, failed));
            }
            Some(text)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
