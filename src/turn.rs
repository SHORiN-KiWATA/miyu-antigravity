//! 一轮 agy 的流 → `driver.delta` 的增量和 `driver.call` 的回答（设计第四节第 4、5 条，第八节）。纯的状态机，不碰 I/O。
//!
//! - `agent_response` 的 `text_delta`：一步第一次有字时开一块正文（`start {kind: "text"}`），之后交字；这一步 `DONE` 时收块；
//!   下一个 `agent_response` 步另开一块；没有字的步不开块。一轮里第一段正文的开头先攒着认内容策略拦截（`turn/hold.rs`）。
//! - 工具步骤：名字以 [`MCP_PREFIX`] 开头、状态是 `ACTIVE` 的，记「流这边到了」；别的工具步（agy 自己跑了的）照实交成供应商侧
//!   工具块（`turn/hosted.rs`）。MCP 服务转来的那一件调用（[`Turn::tool_call`]）记「MCP 那边到了」。**两边都到了**才交一块工具调用并结束这一次（回答照常是
//!   用量，[`Fed::handed`] 带调用编号）：先后以流为准（它前面的正文都已读完），内容（名字、`arguments` 原文）以 MCP 那边为准。
//!   只到了一边的，[`Turn::waiting`] 说在等哪一边，等多久由调用的人管（[`Turn::timeout`] 说到点了怎么办）。
//! - `result`：`SUCCESS` 的回用量（这一次里各个 `agent_response DONE` 的 `usage`：输入、缓存命中照最后一个，输出、思考加起来，
//!   施工 AG-12；一条都没有的不报）；不是成功的照
//!   [`failure`] 归类；正文就是拦截文案的回 `content_policy`（不管 `status`）。
//! - `init.agent` 不是要的代理：当场结束，`unclassified`。

use serde_json::{Value, json};

use crate::failure::{self, Class};
use crate::stream::{Event, ToolState, Usage};

mod hold;
mod hosted;

pub use hosted::{OUTCOME_LIMIT, cut};

/// agy 里 Miyu 的 MCP 工具的名字前缀（`mcp_<服务名>_<工具名>`，服务名见 [`crate::mcp_config::SERVER`]）。
pub const MCP_PREFIX: &str = "mcp_miyu_";

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
    /// 流这边：一步 MCP 工具步 `ACTIVE` 到了。
    stream_tool: bool,
    /// MCP 那边：转来的那一件调用。
    mcp: Option<Held>,
    /// 已经交回了一件工具调用。
    handed: bool,
    /// 攒着的正文开头、认出的拦截文案。
    hold: hold::Hold,
    /// 开着的供应商侧工具块。
    hosting: hosted::Hosting,
}

/// MCP 那边到了的一件调用：编号、名字、`arguments` 的 JSON 原文。
#[derive(Debug)]
struct Held {
    n: u64,
    name: String,
    arguments: String,
}

/// 只到了一边，在等另一边。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// 流这边到了，等 MCP 那边的调用。
    ForMcp,
    /// MCP 那边到了，等流这边的工具步。
    ForStream,
}

/// 读一条以后：要交的增量，和读完了的话这一次的回答（`driver.call` 回应的 `result`）。
#[derive(Debug, Default, PartialEq)]
pub struct Fed {
    /// 照先后要交的增量。
    pub deltas: Vec<Value>,
    /// 这一次结束了：回答。
    pub answer: Option<Value>,
    /// 这一次是交回一件工具调用结束的：那件调用的编号（回答是用量）。
    pub handed: Option<u64>,
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
            stream_tool: false,
            mcp: None,
            handed: false,
            hold: hold::Hold::default(),
            hosting: hosted::Hosting::default(),
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
                if let Some(shown) = self.policy_blocked(&outcome) {
                    fed.answer = Some(failed(Class::ContentPolicy, &hold::block_message(&shown)));
                } else if outcome.succeeded() {
                    self.close_open(&mut fed.deltas);
                    fed.answer = Some(self.usage_answer());
                } else {
                    self.release_held(&mut fed.deltas);
                    self.close_hosted(&mut fed.deltas);
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
            Event::Tool(step) => {
                if !step.name.starts_with(MCP_PREFIX) {
                    self.hosted_step(&step, &mut fed.deltas);
                } else if step.state == ToolState::Active {
                    self.stream_tool = true;
                    self.close_hosted(&mut fed.deltas);
                    self.pair(&mut fed);
                }
            }
            Event::UserInput | Event::Ignored => {}
        }
        fed
    }

    /// MCP 那边到了：调用 `n`，工具名 `name`（MCP 收到的原样），参数 `arguments`。`reply` 不进状态机，由调用的人拿着。
    pub fn tool_call(&mut self, n: u64, name: &str, arguments: &Value) -> Fed {
        self.mcp = Some(Held {
            n,
            name: name.to_string(),
            arguments: arguments.to_string(),
        });
        let mut fed = Fed::default();
        self.pair(&mut fed);
        fed
    }

    /// 在等哪一边；两边都没到、或者已经交回了的是 `None`。
    #[must_use]
    pub fn waiting(&self) -> Option<Waiting> {
        if self.handed {
            return None;
        }
        match (self.stream_tool, self.mcp.is_some()) {
            (true, false) => Some(Waiting::ForMcp),
            (false, true) => Some(Waiting::ForStream),
            _ => None,
        }
    }

    /// 等另一边等到点了：只有 MCP 那边到了的照交；只有流那边到了的这一次出错（agy 的工具服务没连上 Miyu）。没在等的什么都不做。
    pub fn timeout(&mut self) -> Fed {
        match self.waiting() {
            Some(Waiting::ForStream) => {
                let mut fed = Fed::default();
                self.hand_back(&mut fed);
                fed
            }
            Some(Waiting::ForMcp) => Fed {
                answer: Some(failed(
                    Class::Unclassified,
                    "agy called a Miyu tool, but its tool server never reached Miyu; check that the miyu entry in mcp_config.json under agy's configuration folder starts miyu-antigravity mcp",
                )),
                ..Fed::default()
            },
            None => Fed::default(),
        }
    }

    /// 两边都到了就把这件工具调用交回，增量和回答记进 `fed`。
    fn pair(&mut self, fed: &mut Fed) {
        if self.stream_tool && self.mcp.is_some() {
            self.hand_back(fed);
        }
    }

    /// 收掉开着的块（攒着的正文先交出去，正文块、供应商侧工具块都收）。
    fn close_open(&mut self, deltas: &mut Vec<Value>) {
        self.close_text(deltas);
        self.close_hosted(deltas);
    }

    /// 收掉开着的块，交一块工具调用（`start`、`text`、`private`、`end`），这一次以用量结束。
    fn hand_back(&mut self, fed: &mut Fed) {
        let Some(held) = self.mcp.take() else {
            return;
        };
        self.close_open(&mut fed.deltas);
        let index = self.next_block;
        self.next_block += 1;
        self.delivered = true;
        self.handed = true;
        fed.deltas
            .push(json!({"start": {"index": index, "kind": {"tool_call": {"name": held.name}}}}));
        for piece in pieces(&held.arguments, TEXT_PIECE) {
            fed.deltas
                .push(json!({"text": {"index": index, "text": piece}}));
        }
        fed.deltas
            .push(json!({"private": {"index": index, "data": {"mcp": held.n}}}));
        fed.deltas.push(json!({"end": {"index": index}}));
        fed.answer = Some(self.usage_answer());
        fed.handed = Some(held.n);
    }

    /// 回答：这一次各个 `agent_response DONE` 加起来的用量；一条都没有的是空的。
    fn usage_answer(&self) -> Value {
        match self.usage {
            Some(usage) => json!({"usage": {
                "uncached": usage.input,
                "cache_read": usage.cache_read,
                "cache_write": 0,
                "output": usage.output,
                "reasoning": usage.thinking,
            }}),
            None => json!({}),
        }
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
        self.close_hosted(deltas);
        self.leave_held_step(step_index, deltas);
        if let Some((step, index)) = self.open
            && step != step_index
        {
            deltas.push(json!({"end": {"index": index}}));
            self.open = None;
        }
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            self.text(step_index, text, deltas);
        }
        if done {
            if let Some(usage) = usage {
                self.usage = Some(self.usage.map_or(usage, |sum| sum.then(usage)));
            }
            self.close_text(deltas);
        }
    }

    /// 流断了、没等到 `result`（agy 退出了、读不下去了）：`why` 是英文的一句原因。回答照 [`failure`] 归类；认出了拦截文案的是
    /// `content_policy`。攒着没交的字先由 [`Turn::flush`] 交掉再调它（交过增量才算交过）。
    #[must_use]
    pub fn broken(&self, why: &str) -> Value {
        if let Some(shown) = self.blocked_text() {
            return failed(Class::ContentPolicy, &hold::block_message(shown));
        }
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
mod hold_tests;
#[cfg(test)]
mod hosted_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_tests;
