//! 把统一请求的 `messages` 整份转写成发给 agy 的一条文字（设计第五节「新开对话时的重放」）。纯函数。
//!
//! 格式照旧版 Miyu 的 `cli_relay/payload.rs`：一段说明加 `<conversation-history>`，里面照先后写 `User:`、`Assistant:` 和各自的字；
//! 最后一轮人说的话（结尾连着的几条 user 消息）单独成块跟在后面。只有这一轮、没有历史的，直接发那几条的字。`request.tail` 有的
//! 接在最后。
//!
//! 历史里不写工具（施工 AG-14）：她调过的工具、工具的结果、供应商侧工具都不进转写，只留人说的话和她说的话。以前照写成
//! `[called tool NAME with ARGS]`、`[tool result]`，模型新开对话以后有时不真调工具，照这个样子写一段字当回复，发进了群里
//! （2026-11-23 起主人的群里 22 次）。旧版 Miyu 的工具循环在 agy 里头，它的历史本来就只有字，这样写的转写里没有工具那几行。
//!
//! 接在活着的 agy 对话上的新的一轮只发新的那几条（[`new_turn`]），一条一段。
//!
//! 整条不超过 [`BUDGET`] 字节：超了从最老的回合丢起、写一句省略；最后一轮不砍。给模型看的字在 [`crate::words`]。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::words;

/// 一条输入至多多少字节：agy 超过 192,000 字节的从尾部悄悄截断（[`crate::agy::TEXT_LIMIT`]，设计第二节「输入上限」），打九折。
/// 照转写出来的字算就对：agy 的上限算在解出来的字上，不算 JSON 转义多出来的（2026-11-18 真机核过）。
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
    let parsed = parse(request)?;
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
    // 一轮做到一半新开的对话（历史以工具的结果、调工具的回复结束；她挂着工具调用时 agy 先说了话就是这样）：这一轮已经做了的
    // （历史里最后一条人说的话以后的：调过的工具、拿到的结果、说过的话）写成一块旁注接在最后一轮前面，不写成对话（施工
    // AG-14），不然她又调一遍。以说话结束的那一轮已经答完了，照历史写。
    let in_progress = history
        .last()
        .is_some_and(|message| matches!(message.role, Role::Tool { .. }) || message.tooled);
    let current_from = if in_progress {
        history
            .iter()
            .rposition(|message| message.role == Role::User)
            .map_or(0, |index| index + 1)
    } else {
        history.len()
    };
    let (earlier, current) = history.split_at(current_from);
    if let Some(done) = this_turn(current) {
        last = format!("{done}\n\n{last}");
    }

    let lines: Vec<(Role, String)> = earlier
        .iter()
        .filter_map(|message| Some((message.role, message.history_line()?)))
        .collect();
    if lines.is_empty() {
        return Ok(last);
    }
    let fixed = words::HISTORY_HEAD.len() + words::HISTORY_FOOT.len() + 2 + last.len();
    let whole: usize = lines.iter().map(|(_, line)| line.len()).sum();
    let keep_from = if fixed + whole <= budget {
        0
    } else {
        oldest_kept(&lines, budget.saturating_sub(fixed + words::OMITTED.len()))
    };

    let mut text = String::from(words::HISTORY_HEAD);
    if keep_from > 0 {
        text.push_str(words::OMITTED);
    }
    for (_, line) in &lines[keep_from..] {
        text.push_str(line);
    }
    text.push_str(words::HISTORY_FOOT);
    text.push_str("\n\n");
    text.push_str(&last);
    Ok(text)
}

/// 接在活着的 agy 对话上的新的一轮（设计第四节第 2 条）：`request.messages` 第 `from` 条起每条一段字，块转成字照
/// [`transcribe`] 的规矩，没有字的不要；`request.tail` 有的接在最后一段后面；一段都没有的是 [`words::CONTINUE`]。
///
/// # Errors
///
/// 同 [`transcribe`]。
pub fn new_turn(request: &Value, from: usize) -> Result<Vec<String>, BadRequest> {
    let parsed = parse(request)?;
    let mut texts: Vec<String> = parsed
        .into_iter()
        .skip(from)
        .map(|message| message.body)
        .filter(|body| !body.is_empty())
        .collect();
    if texts.is_empty() {
        texts.push(words::CONTINUE.to_string());
    }
    if let Some(tail) = request.get("tail").and_then(Value::as_str)
        && let Some(last) = texts.last_mut()
    {
        last.push_str("\n\n");
        last.push_str(tail);
    }
    Ok(texts)
}

/// 第 `index` 条消息的字（块转成字，块之间换行，规矩同 [`transcribe`]）；没有这一条的是 `None`。给回工具结果用
/// （图片、文件换成占位）。
///
/// # Errors
///
/// 同 [`transcribe`]。
pub fn message_text(request: &Value, index: usize) -> Result<Option<String>, BadRequest> {
    Ok(parse(request)?
        .into_iter()
        .nth(index)
        .map(|message| message.body))
}

/// 读出 `request.messages` 的每一条。
fn parse(request: &Value) -> Result<Vec<Message>, BadRequest> {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| BadRequest("request.messages is not an array".into()))?;
    let texts = |key: &str| -> BTreeMap<&str, &str> {
        request
            .get(key)
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(blob, text)| Some((blob.as_str(), text.as_str()?)))
                    .collect()
            })
            .unwrap_or_default()
    };
    let known = Known {
        described: texts("described"),
        saved: texts(SAVED),
    };
    messages
        .iter()
        .map(|message| Message::read(message, &known))
        .collect()
}

/// 请求里存成了文件的图、文件那一格（施工 AG-9）：`{"<blob 哈希>": "<路径>"}`，核心不写这一格，是本包照 `blobs` 存好以后
/// 加进来的（[`with_saved`]）。
pub const SAVED: &str = "miyu_antigravity_saved";

/// 照 `saved`（blob 哈希 → 存成的文件）给 `request` 加上 [`SAVED`] 那一格；没有存成的原样交回。
#[must_use]
pub fn with_saved<'a>(
    request: &'a Value,
    saved: &BTreeMap<String, std::path::PathBuf>,
) -> std::borrow::Cow<'a, Value> {
    if saved.is_empty() {
        return std::borrow::Cow::Borrowed(request);
    }
    let mut request = request.clone();
    if let Some(object) = request.as_object_mut() {
        let paths = saved
            .iter()
            .map(|(hash, path)| (hash.clone(), Value::from(path.display().to_string())))
            .collect();
        object.insert(SAVED.to_string(), Value::Object(paths));
    }
    std::borrow::Cow::Owned(request)
}

/// 图、文件块转成字时要查的：核心交的图的转述，本包存成的文件的路径。
struct Known<'a> {
    described: BTreeMap<&'a str, &'a str>,
    saved: BTreeMap<&'a str, &'a str>,
}

/// 历史从哪一行起留：从最新往回加，放不下 `room` 就停；再往后挪到下一条 user 消息，让转写从一轮的开头起（截在回复中间会让
/// 模型把半截旧回复当上文）。一行都放不下的交 `lines.len()`。
fn oldest_kept(lines: &[(Role, String)], room: usize) -> usize {
    let mut used = 0;
    let mut keep_from = lines.len();
    for (index, (_, line)) in lines.iter().enumerate().rev() {
        if used + line.len() > room {
            break;
        }
        used += line.len();
        keep_from = index;
    }
    lines[keep_from..]
        .iter()
        .position(|(role, _)| *role == Role::User)
        .map_or(lines.len(), |offset| keep_from + offset)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    Tool { failed: bool },
}

/// 一条消息：谁说的，块转成的字（块之间换行），进历史转写的字（不写工具的块，施工 AG-14），写进这一轮旁注的字。
struct Message {
    role: Role,
    body: String,
    spoken: String,
    steps: String,
    /// 有工具的块。
    tooled: bool,
}

impl Message {
    fn read(message: &Value, known: &Known<'_>) -> Result<Message, BadRequest> {
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
        let text = |keep: &dyn Fn(&Value) -> bool| {
            blocks
                .iter()
                .filter(|block| keep(block))
                .filter_map(|block| block_text(block, known))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let body = text(&|_| true);
        let spoken = text(&|block| !is_tool(block));
        let steps = match role {
            Role::Tool { failed } if !body.is_empty() => words::tool_returned(&body, failed),
            Role::Tool { .. } => String::new(),
            _ => blocks
                .iter()
                .filter_map(|block| step_text(block, known))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        Ok(Message {
            role,
            body,
            spoken,
            steps,
            tooled: blocks.iter().any(is_tool),
        })
    }

    /// 历史转写里的一段；不写的（工具的结果，只调了工具、没说话的回复）是 `None`（施工 AG-14）。
    fn history_line(&self) -> Option<String> {
        let label = match self.role {
            Role::User => words::USER,
            Role::Assistant => words::ASSISTANT,
            Role::Tool { .. } => return None,
        };
        // 她以前漏进群里的、照转写编的那几条，标记起的那一截不写（施工 AG-14）。
        let spoken = match self.role {
            Role::Assistant => crate::turn::forged::before_marker(&self.spoken),
            _ => self.spoken.as_str(),
        };
        (!spoken.is_empty()).then(|| format!("{label}{spoken}\n\n"))
    }
}

/// 这一轮已经做了的那一块（施工 AG-14）；什么都没做的没有。
fn this_turn(current: &[Message]) -> Option<String> {
    let steps: Vec<&str> = current
        .iter()
        .map(|message| message.steps.as_str())
        .filter(|steps| !steps.is_empty())
        .collect();
    (!steps.is_empty()).then(|| {
        format!(
            "{}{}\n{}",
            words::THIS_TURN_OPEN,
            steps.join("\n"),
            words::THIS_TURN_CLOSE
        )
    })
}

/// 这一轮旁注里的一块：她说的话、调的工具、供应商替她跑的工具照旁注的口吻写，别的照 [`block_text`]。
fn step_text(block: &Value, known: &Known<'_>) -> Option<String> {
    let field = |key: &str| block.get(key).and_then(Value::as_str).unwrap_or_default();
    match block.get("type").and_then(Value::as_str)? {
        "text" => block_text(block, known).map(|text| words::you_said(&text)),
        "tool_call" => Some(words::you_called(field("name"), field("args"))),
        "hosted_tool" => {
            let mut text = words::provider_ran(field("name"), field("args"));
            if let Some(outcome) = block.get("outcome") {
                let failed = outcome.get("failed").and_then(Value::as_bool) == Some(true);
                let said = outcome
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                text.push('\n');
                text.push_str(&words::tool_returned(said, failed));
            }
            Some(text)
        }
        _ => block_text(block, known),
    }
}

/// 是工具的块：她调的工具、供应商侧工具。历史转写里不写（施工 AG-14）。
fn is_tool(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("tool_call" | "hosted_tool")
    )
}

/// 一块转成的字；不写的（思考、空的字、不认识的块）是 `None`。
fn block_text(block: &Value, known: &Known<'_>) -> Option<String> {
    let field = |key: &str| block.get(key).and_then(Value::as_str);
    let saved = field("blob").and_then(|blob| known.saved.get(blob));
    match field("type")? {
        "text" => field("text")
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        "image" => Some(
            match (
                saved,
                field("blob").and_then(|blob| known.described.get(blob)),
            ) {
                (Some(path), _) => words::image_saved(field("name"), path),
                (None, Some(description)) => words::image_described(field("name"), description),
                (None, None) => words::image_omitted(field("name")),
            },
        ),
        "file" => {
            let name = field("name").unwrap_or_default();
            Some(match saved {
                Some(path) => words::file_saved(name, path),
                None => words::file_omitted(name),
            })
        }
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
