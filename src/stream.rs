//! agy 的 stream-json 输出：一行 → 一个事件（设计第二节「输出」「正文」「工具步骤」，第八节）。纯函数，不碰 I/O。
//!
//! 字段照真机录的样本（`tests/samples/`，agy 1.4.0）。样本里没有的形状（工具 `ERROR`、`error_message` 步、`result` 出错）照
//! 设计第二节和旧版 Miyu 的 `antigravity/stream.rs` 写。不认识的事件、不认识的 `step_type` 当 [`Event::Ignored`]。

use serde_json::Value;

/// agy 一行输出说的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 开头一条：这段对话的编号、挂上的代理。
    Init {
        /// `conversation_id`。
        conversation_id: Option<String>,
        /// `init.agent`：实际用的代理名。
        agent: Option<String>,
    },
    /// 人的话那一步（`step_type: "user_input"`）。
    UserInput,
    /// 模型回复的一段（`step_type: "agent_response"`）。
    Response {
        /// `step_index`：同一步的几条同一个数。
        step_index: u64,
        /// `state` 是 `DONE`：这一步说完了。
        done: bool,
        /// `text_delta`：这一条新来的字。
        text: Option<String>,
        /// `DONE` 带的这一次模型调用的用量。
        usage: Option<Usage>,
    },
    /// 一次工具调用的一条（`step_type: "tool"`）。
    Tool(ToolStep),
    /// agy 报的一段错（`step_type: "error_message"` 的 `text_delta`）。
    ErrorMessage {
        /// 原话。
        text: String,
    },
    /// 最后一条：这一轮说完了（`event: "result"`）。
    Result(Outcome),
    /// 不认识的事件、不认识的 `step_type`。
    Ignored,
}

/// 一次模型调用的用量（`usage`）。没写的格当 0。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// `input_tokens`：没命中缓存的输入。
    pub input: u64,
    /// `output_tokens`。
    pub output: u64,
    /// `thinking_tokens`。
    pub thinking: u64,
    /// `cache_read_tokens`。
    pub cache_read: u64,
}

impl Usage {
    /// 一轮里接着的下一次请求（施工 AG-12）：输入、缓存命中照后一次（那就是这时的上下文大小，核心拿它当上下文用了多少），
    /// 输出、思考加起来。agy 跑自带工具时一轮里来回请求几次，每次都带整段上下文，输入加起来就虚高。
    #[must_use]
    pub fn then(self, later: Usage) -> Usage {
        Usage {
            input: later.input,
            output: self.output + later.output,
            thinking: self.thinking + later.thinking,
            cache_read: later.cache_read,
        }
    }
}

/// 工具步骤的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    /// `ACTIVE`：开始了，参数一次给全。
    Active,
    /// `DONE`：跑完了。
    Done,
    /// `ERROR`：跑失败了。
    Error,
    /// 别的字，原样。
    Other(String),
}

/// 工具步骤的一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStep {
    /// `step_index`。
    pub step_index: u64,
    /// `state`。
    pub state: ToolState,
    /// `tool_name`（没有的照 `tool_info.name`）。
    pub name: String,
    /// `tool_info.parameters` 的 JSON 原文（键的先后照 agy 给的）。
    pub parameters: Option<String>,
    /// `tool_info.output`：MCP 工具的结果；不是字的写成 JSON。
    pub output: Option<String>,
    /// `tool_info.error`。
    pub error: Option<ToolError>,
}

impl ToolStep {
    /// 写进日志的一句：第几步、名字、状态、参数、结果多长、错。
    #[must_use]
    pub fn describe(&self) -> String {
        let state = match &self.state {
            ToolState::Active => "ACTIVE",
            ToolState::Done => "DONE",
            ToolState::Error => "ERROR",
            ToolState::Other(other) => other.as_str(),
        };
        let mut parts = vec![format!(
            "tool step {} {} {state}",
            self.step_index, self.name
        )];
        if let Some(parameters) = &self.parameters {
            parts.push(format!("parameters={parameters}"));
        }
        if let Some(output) = &self.output {
            parts.push(format!("output={} bytes", output.len()));
        }
        if let Some(error) = &self.error {
            parts.push(format!(
                "error={}: {}",
                error.kind.as_deref().unwrap_or("?"),
                error.message
            ));
        }
        parts.join(" ")
    }
}

/// 工具步骤报的错（`tool_info.error`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    /// `type`。
    pub kind: Option<String>,
    /// `message`。
    pub message: String,
}

/// `result` 那一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// `conversation_id`。
    pub conversation_id: Option<String>,
    /// `status`：`SUCCESS` 是成了。
    pub status: String,
    /// `response`：这一轮回复的全文。
    pub response: String,
    /// `error`：不是字的写成 JSON；没有、`null` 的是 `None`。
    pub error: Option<String>,
    /// `usage`：整段会话累计的。
    pub usage: Option<Usage>,
}

impl Outcome {
    /// `status` 是不是 `SUCCESS`。
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.status == "SUCCESS"
    }
}

/// 不是 JSON 的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotJson(pub String);

impl std::fmt::Display for NotJson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agy wrote a line that is not JSON: {}", self.0)
    }
}

impl std::error::Error for NotJson {}

/// 读一行。
///
/// # Errors
///
/// 这一行不是 JSON：[`NotJson`] 带着 serde 的原话。
pub fn parse(line: &str) -> Result<Event, NotJson> {
    let value: Value = serde_json::from_str(line).map_err(|error| NotJson(error.to_string()))?;
    Ok(match value.get("event").and_then(Value::as_str) {
        Some("init") => Event::Init {
            conversation_id: string(&value, "conversation_id"),
            agent: value.get("init").and_then(|init| string(init, "agent")),
        },
        Some("step_update") => value.get("step_update").map_or(Event::Ignored, step),
        Some("result") => value.get("result").map_or(Event::Ignored, |result| {
            Event::Result(Outcome {
                conversation_id: string(result, "conversation_id"),
                status: string(result, "status").unwrap_or_default(),
                response: string(result, "response").unwrap_or_default(),
                error: written(result.get("error")),
                usage: result.get("usage").map(usage),
            })
        }),
        _ => Event::Ignored,
    })
}

/// `step_update` 里的一步。
fn step(step: &Value) -> Event {
    let step_index = step.get("step_index").and_then(Value::as_u64).unwrap_or(0);
    let state = string(step, "state").unwrap_or_default();
    match step.get("step_type").and_then(Value::as_str) {
        Some("user_input") => Event::UserInput,
        Some("agent_response") => Event::Response {
            step_index,
            done: state == "DONE",
            text: string(step, "text_delta"),
            usage: step.get("usage").map(usage),
        },
        Some("tool") => {
            let info = step.get("tool_info");
            let name = string(step, "tool_name")
                .or_else(|| info.and_then(|info| string(info, "name")))
                .unwrap_or_default();
            Event::Tool(ToolStep {
                step_index,
                state: match state.as_str() {
                    "ACTIVE" => ToolState::Active,
                    "DONE" => ToolState::Done,
                    "ERROR" => ToolState::Error,
                    _ => ToolState::Other(state),
                },
                name,
                parameters: info
                    .and_then(|info| info.get("parameters"))
                    .map(Value::to_string),
                output: written(info.and_then(|info| info.get("output"))),
                error: info
                    .and_then(|info| info.get("error"))
                    .and_then(|error| match error {
                        Value::Null => None,
                        Value::String(message) => Some(ToolError {
                            kind: None,
                            message: message.clone(),
                        }),
                        _ => Some(ToolError {
                            kind: string(error, "type"),
                            message: string(error, "message").unwrap_or_default(),
                        }),
                    }),
            })
        }
        Some("error_message") => Event::ErrorMessage {
            text: string(step, "text_delta").unwrap_or_default(),
        },
        _ => Event::Ignored,
    }
}

fn usage(value: &Value) -> Usage {
    let count = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input: count("input_tokens"),
        output: count("output_tokens"),
        thinking: count("thinking_tokens"),
        cache_read: count("cache_read_tokens"),
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// 字照字，别的写成 JSON；没有、`null` 的是 `None`。
fn written(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests;
