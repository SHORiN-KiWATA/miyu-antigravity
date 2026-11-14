//! 一轮 agy 的流 → `driver.delta` 的增量和 `driver.call` 的回答（设计第四节第 4、5 条，第八节）。纯的状态机，不碰 I/O。
//!
//! - `agent_response` 的 `text_delta`：一步第一次有字时开一块正文（`start {kind: "text"}`），之后交字；这一步 `DONE` 时收块；
//!   下一个 `agent_response` 步另开一块；没有字的步不开块。
//! - 工具步骤 AG-1 不管（AG-3、AG-4 接），照常往下读。
//! - `result`：`SUCCESS` 的回用量（这一次里每个 `agent_response DONE` 的 `usage` 加起来；一条都没有的不报）；不是成功的照
//!   [`failure`] 归类。
//! - `init.agent` 不是要的代理：当场结束，`unclassified`。

use serde_json::{Value, json};

use crate::failure::{self, Class};
use crate::stream::{Event, Usage};

/// 一段字至多多少字节：交给核心的一行不能超过 1 MiB，字里的控制字符转义后至多六倍长。
pub const TEXT_PIECE: usize = 128 * 1024;

/// 一轮的状态。
#[derive(Debug)]
pub struct Turn {
    agent: String,
    next_block: u64,
    /// 开着的正文块：哪一步、第几块。
    open: Option<(u64, u64)>,
    usage: Option<Usage>,
    delivered: bool,
    conversation: Option<String>,
    errors: Vec<String>,
}

/// 读一条以后：要交的增量，和读完了的话这一次的回答（`driver.call` 回应的 `result`）。
#[derive(Debug, Default, PartialEq)]
pub struct Fed {
    /// 照先后要交的增量。
    pub deltas: Vec<Value>,
    /// 这一次结束了：回答。
    pub answer: Option<Value>,
}

impl Turn {
    /// 新的一轮；`agent` 是挂上的代理该叫的名字。
    #[must_use]
    pub fn new(agent: &str) -> Turn {
        Turn {
            agent: agent.to_string(),
            next_block: 0,
            open: None,
            usage: None,
            delivered: false,
            conversation: None,
            errors: Vec::new(),
        }
    }

    /// agy 这段对话的编号（`init` 或 `result` 里的）。
    #[must_use]
    pub fn conversation(&self) -> Option<&str> {
        self.conversation.as_deref()
    }

    /// 读一条事件。
    pub fn feed(&mut self, event: Event) -> Fed {
        let mut fed = Fed::default();
        match event {
            Event::Init {
                conversation_id,
                agent,
            } => {
                self.conversation = self.conversation.take().or(conversation_id);
                if agent.as_deref() != Some(self.agent.as_str()) {
                    let got = agent.map_or_else(
                        || "no agent".to_string(),
                        |agent| format!("agent {agent:?}"),
                    );
                    fed.answer = Some(failed(
                        Class::Unclassified,
                        &format!(
                            "agy started with {got} instead of {:?}: the agent file under .gemini/config/agents was not picked up",
                            self.agent
                        ),
                    ));
                }
            }
            Event::Response {
                step_index,
                done,
                text,
                usage,
            } => self.respond(step_index, done, text, usage, &mut fed.deltas),
            Event::ErrorMessage { text } => {
                if !text.trim().is_empty() {
                    self.errors.push(text.trim().to_string());
                }
            }
            Event::Result(outcome) => {
                self.conversation = self.conversation.take().or(outcome.conversation_id.clone());
                if outcome.succeeded() {
                    if let Some((_, index)) = self.open.take() {
                        fed.deltas.push(json!({"end": {"index": index}}));
                    }
                    fed.answer = Some(match self.usage {
                        Some(usage) => json!({"usage": {
                            "uncached": usage.input,
                            "cache_read": usage.cache_read,
                            "cache_write": 0,
                            "output": usage.output,
                            "reasoning": usage.thinking,
                        }}),
                        None => json!({}),
                    });
                } else {
                    let error = outcome.error.clone().unwrap_or_default();
                    let said = format!("{error}\n{}", self.errors.join("\n"));
                    let detail = [
                        error.trim(),
                        &self.errors.join("; "),
                        outcome.response.trim(),
                    ]
                    .into_iter()
                    .find(|text| !text.is_empty())
                    .unwrap_or("no detail")
                    .to_string();
                    fed.answer = Some(failed(
                        failure::classify(&said, self.delivered),
                        &format!("agy failed ({}): {detail}", outcome.status),
                    ));
                }
            }
            Event::UserInput | Event::Tool(_) | Event::Ignored => {}
        }
        fed
    }

    /// 读一条 `agent_response`：开块、交字、`DONE` 时记用量、收块。
    fn respond(
        &mut self,
        step_index: u64,
        done: bool,
        text: Option<String>,
        usage: Option<Usage>,
        deltas: &mut Vec<Value>,
    ) {
        if let Some((step, index)) = self.open
            && step != step_index
        {
            deltas.push(json!({"end": {"index": index}}));
            self.open = None;
        }
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            let index = if let Some((_, index)) = self.open {
                index
            } else {
                let index = self.next_block;
                self.next_block += 1;
                self.open = Some((step_index, index));
                self.delivered = true;
                deltas.push(json!({"start": {"index": index, "kind": "text"}}));
                index
            };
            for piece in pieces(&text, TEXT_PIECE) {
                deltas.push(json!({"text": {"index": index, "text": piece}}));
            }
        }
        if done {
            if let Some(usage) = usage {
                self.usage = Some(self.usage.map_or(usage, |sum| sum.plus(usage)));
            }
            if let Some((_, index)) = self.open.take() {
                deltas.push(json!({"end": {"index": index}}));
            }
        }
    }

    /// 流断了、没等到 `result`（agy 退出了、读不下去了）：`why` 是英文的一句原因。回答照 [`failure`] 归类。
    #[must_use]
    pub fn broken(&self, why: &str) -> Value {
        let mut message = why.to_string();
        if !self.errors.is_empty() {
            message.push_str(": ");
            message.push_str(&self.errors.join("; "));
        }
        let class = if self.errors.is_empty() {
            failure::otherwise(self.delivered)
        } else {
            failure::classify(&self.errors.join("\n"), self.delivered)
        };
        failed(class, &message)
    }
}

/// 回答里只有 `error` 的那种。
#[must_use]
pub fn failed(class: Class, message: &str) -> Value {
    json!({"error": failure::error(class, message)})
}

/// 把 `text` 切成至多 `limit` 字节的几段，不切开一个字。
fn pieces(text: &str, limit: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.len() > limit {
        let mut cut = limit;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            // 上限比一个字还短：这个字单独成一段。
            cut = rest.char_indices().nth(1).map_or(rest.len(), |(at, _)| at);
        }
        let (piece, tail) = rest.split_at(cut);
        pieces.push(piece);
        rest = tail;
    }
    pieces.push(rest);
    pieces
}

#[cfg(test)]
mod tests;
