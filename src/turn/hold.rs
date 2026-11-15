//! 攒正文的开头，认内容策略拦截（设计第八节）。
//!
//! agy 被 Google 的内容策略拦下时，不报错，而是把一段固定的拦截文案当正文流出来、`result` 还报成功（[`failure::POLICY_BLOCK`]）。
//! 拦截文案不能当回复交出去，所以**一轮里第一段正文**的开头先攒着，只攒到看得出它「还可能是拦截文案」为止（[`failure::policy_start`]）：
//!
//! - 不是（写出来的字和 [`failure::POLICY_BLOCK`] 对不上了）：攒着的字一块交出去，每条 `text_delta` 还是各自一条增量，和不攒时
//!   逐字节一样，只是开头那几个字晚一点交；
//! - 是（固定措辞写全了）：之后这一步的字都不交，这一轮以 `content_policy` 结束；
//! - 还没看出来时这一步 `DONE` 了、来了别的步、`result`、流断了、被取消：看得出不是完整的拦截文案，攒着的照常交。
//!
//! 已经开过块（正文、工具）的不再攒：拦截文案是整个回复。

use serde_json::{Value, json};

use super::{TEXT_PIECE, Turn, pieces};
use crate::failure::{self, Policy};
use crate::stream::Outcome;

/// 记在报错里的拦截文案至多多少个字。
const PREVIEW_CHARS: usize = 200;

/// 攒着的开头和认出来的拦截。
#[derive(Debug, Default)]
pub(super) struct Hold {
    /// 攒着的正文开头（还没交）。
    held: Option<Held>,
    /// 认出了拦截文案：它的开头一段（写进报错）。
    blocked: Option<String>,
}

/// 攒着的一步正文。
#[derive(Debug)]
struct Held {
    step: u64,
    /// 一条条 `text_delta`，交出去时还是一条一条。
    pieces: Vec<String>,
    /// 接起来的全文。
    text: String,
}

impl Turn {
    /// 一步 `step` 来了一段字 `text`：还能攒的攒着，认出拦截的记下，别的开块交出去。
    pub(super) fn text(&mut self, step: u64, text: String, deltas: &mut Vec<Value>) {
        if let Some(blocked) = self.hold.blocked.as_mut() {
            // 拦截文案接着往下写：不交，只留开头一段写报错。
            if blocked.chars().count() < PREVIEW_CHARS {
                blocked.push_str(&text);
            }
            return;
        }
        if self.open.is_none() && self.next_block == 0 {
            let held = self.hold.held.get_or_insert_with(|| Held {
                step,
                pieces: Vec::new(),
                text: String::new(),
            });
            held.text.push_str(&text);
            held.pieces.push(text);
            match failure::policy_start(&held.text) {
                Policy::Maybe => {}
                Policy::Block => {
                    self.hold.blocked = self.hold.held.take().map(|held| held.text);
                }
                Policy::No => self.release_held(deltas),
            }
            return;
        }
        let index = if let Some((_, index)) = self.open {
            index
        } else {
            let index = self.next_block;
            self.next_block += 1;
            self.open = Some((step, index));
            self.delivered = true;
            deltas.push(json!({"start": {"index": index, "kind": "text"}}));
            index
        };
        push_text(deltas, index, &text);
    }

    /// 攒着的不是拦截文案（或者不用再攒了）：开块，攒着的每一条照原样交；块留着开着，由后面照常收。
    pub(super) fn release_held(&mut self, deltas: &mut Vec<Value>) {
        let Some(held) = self.hold.held.take() else {
            return;
        };
        let index = self.next_block;
        self.next_block += 1;
        self.open = Some((held.step, index));
        self.delivered = true;
        deltas.push(json!({"start": {"index": index, "kind": "text"}}));
        for piece in &held.pieces {
            push_text(deltas, index, piece);
        }
    }

    /// 攒着的是别一步的：交出去，收块（那一步已经过去了）。
    pub(super) fn leave_held_step(&mut self, step: u64, deltas: &mut Vec<Value>) {
        if self
            .hold
            .held
            .as_ref()
            .is_some_and(|held| held.step != step)
        {
            self.release_held(deltas);
        }
    }

    /// 收掉开着的正文块；攒着的先交出去。
    pub(super) fn close_text(&mut self, deltas: &mut Vec<Value>) {
        self.release_held(deltas);
        if let Some((_, index)) = self.open.take() {
            deltas.push(json!({"end": {"index": index}}));
        }
    }

    /// 流断了、被取消、核心不要了：攒着的该交的交出去（块不收，和没攒时一样）。
    pub fn flush(&mut self) -> Vec<Value> {
        let mut deltas = Vec::new();
        self.release_held(&mut deltas);
        deltas
    }

    /// 这一轮是不是被内容策略拦了：认出了拦截文案，或者什么都没交、`result.response` 本身就是拦截文案。交拦截文案的开头一段。
    pub(super) fn policy_blocked(&self, outcome: &Outcome) -> Option<String> {
        if let Some(blocked) = &self.hold.blocked {
            return Some(blocked.clone());
        }
        let nothing = self.next_block == 0 && self.hold.held.is_none();
        (nothing && failure::policy_start(&outcome.response) == Policy::Block)
            .then(|| outcome.response.clone())
    }

    /// 认出了拦截文案、还没收到 `result`（流断了）：交拦截文案的开头一段。
    pub(super) fn blocked_text(&self) -> Option<&str> {
        self.hold.blocked.as_deref()
    }
}

/// 交一条 `text`，太长的切成几条。
fn push_text(deltas: &mut Vec<Value>, index: u64, text: &str) {
    for piece in pieces(text, TEXT_PIECE) {
        deltas.push(json!({"text": {"index": index, "text": piece}}));
    }
}

/// 拦截的报错原话（英文，给人看）：`shown` 是拦截文案的开头一段。
#[must_use]
pub(super) fn block_message(shown: &str) -> String {
    let shown: String = shown.trim().chars().take(PREVIEW_CHARS).collect();
    format!(
        "Google's content policy blocked the prompt (agy streamed its block notice instead of a reply): {shown}"
    )
}
