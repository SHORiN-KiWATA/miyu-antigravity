//! 供应商侧工具：流里不是 `mcp_miyu_` 开头的工具步，都是 agy 那边自己跑了的（设计第七节），照实交成 `hosted_tool` 块。
//!
//! - 头一次见到这一步（`ACTIVE`；没见过 `ACTIVE` 直接来了 `DONE`、`ERROR` 的也算）：先收掉开着的正文块，`start {kind:
//!   {"hosted_tool": {"name": 流里的名字原样}}}`，再把 `parameters` 的 JSON 原文交成 `text`（没有的不交）。
//! - `DONE`：有结果的交 `outcome`，再 `end`；`ERROR`：`outcome` 带 `failed: true`，字是流里的出错原话（没有就 `failed`）。
//! - 结果先截到 [`OUTCOME_LIMIT`]（按字节、不切开一个字）。同一步的 `ACTIVE` 来几次只开一次块。
//! - 开块以后紧跟一条 `private`（`{"hosted": true}`，内容没人读，施工 AG-16）：核心照私有数据认块是不是这个驱动自己家的，没带的当别家，
//!   下一轮换成 `<hosted-tool name=…>` 那段字交回来，agy 读了照着写进回复（2026-11-28 群里漏过两条）。带上了原样交回块，
//!   转写照块的规矩不写它。
//! - 这一步没收的时候来了别的步（正文、下一件工具、`result`、要交回的 MCP 调用）：把它收掉，不交结果。
//!
//! 结果从哪来由调用的人定：流里带了 `output` 的在 [`ToolStep::output`] 里，没带的由 `call/talk.rs` 读了转录（[`crate::brain`]）
//! 填进去再喂，这里不碰 I/O。

use serde_json::{Value, json};

use super::{TEXT_PIECE, Turn, pieces};
use crate::stream::{ToolState, ToolStep};

/// 一件供应商侧工具的结果至多多少字节（核心也照这个数截）。
pub const OUTCOME_LIMIT: usize = 64 * 1024;

/// 供应商侧工具块的状态。
#[derive(Debug, Default)]
pub(super) struct Hosting {
    /// 开着的块：哪一步、第几块。
    open: Option<(u64, u64)>,
    /// 开过块的步（收了以后同一步再来的条不再开）。
    seen: Vec<u64>,
}

impl Turn {
    /// 读一条非 MCP 的工具步。
    pub(super) fn hosted_step(&mut self, step: &ToolStep, deltas: &mut Vec<Value>) {
        if step.name.is_empty() || matches!(step.state, ToolState::Other(_)) {
            return;
        }
        let number = step.step_index;
        if self.hosting.open.is_some_and(|(open, _)| open != number) {
            self.close_hosted(deltas);
        }
        let index = if let Some((_, index)) = self.hosting.open {
            index
        } else {
            if self.hosting.seen.contains(&number) {
                return;
            }
            self.close_text(deltas);
            let index = self.next_block;
            self.next_block += 1;
            self.delivered = true;
            self.hosting.open = Some((number, index));
            self.hosting.seen.push(number);
            deltas.push(
                json!({"start": {"index": index, "kind": {"hosted_tool": {"name": step.name}}}}),
            );
            deltas.push(json!({"private": {"index": index, "data": {"hosted": true}}}));
            if let Some(parameters) = &step.parameters {
                for piece in pieces(parameters, TEXT_PIECE) {
                    deltas.push(json!({"text": {"index": index, "text": piece}}));
                }
            }
            index
        };
        match step.state {
            ToolState::Done => {
                if let Some(text) = step.output.as_deref().filter(|text| !text.is_empty()) {
                    deltas.push(json!({"outcome": {"index": index, "text": cut(text)}}));
                }
            }
            ToolState::Error => {
                let said = step
                    .error
                    .as_ref()
                    .map(|error| error.message.as_str())
                    .filter(|message| !message.is_empty())
                    .unwrap_or("failed");
                deltas
                    .push(json!({"outcome": {"index": index, "text": cut(said), "failed": true}}));
            }
            _ => return,
        }
        self.close_hosted(deltas);
    }

    /// 收掉开着的供应商侧工具块（没有结果的）。
    pub(super) fn close_hosted(&mut self, deltas: &mut Vec<Value>) {
        if let Some((_, index)) = self.hosting.open.take() {
            deltas.push(json!({"end": {"index": index}}));
        }
    }
}

/// `text` 截到至多 [`OUTCOME_LIMIT`] 字节，不切开一个字。
#[must_use]
pub fn cut(text: &str) -> &str {
    if text.len() <= OUTCOME_LIMIT {
        return text;
    }
    let mut end = OUTCOME_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
