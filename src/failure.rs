//! agy 出错归成核心认的类（设计第八节）。纯函数。
//!
//! | 字里有 | 归成 |
//! |---|---|
//! | [`AUTH`] 里的一种 | `auth` |
//! | [`QUOTA`] 里的一种 | `quota` |
//! | [`RATE_LIMITED`] 里的一种 | `rate_limited` |
//! | 别的 | 还没交过增量的 `retryable`，交过的 `unclassified` |
//!
//! 内容策略拦截不走这张表：agy 把 Google 的拦截文案当正文流出来、`result` 还报成功，认法见 [`policy_start`]。
//!
//! 先后照表：几种都有的取靠前的。不分大小写。只看 `result.error`、`error_message` 步的字：agy 的标准错误每次起来都印一行
//! 「not logged into Antigravity」再悄悄登录成功，拿它判会误杀（旧版 `stream.rs` 记的）。

use serde_json::{Value, json};

/// 没登录的字眼。
pub const AUTH: &[&str] = &[
    "not logged in",
    "unauthenticated",
    "login required",
    "sign in",
    "authentication",
];

/// 额度用完的字眼（agy 把额度用完报成 `RESOURCE_EXHAUSTED`，旧版 2026-09-15 实测）。
pub const QUOTA: &[&str] = &[
    "quota",
    "resource_exhausted",
    "resource exhausted",
    "usage limit",
    "credits",
];

/// 限流的字眼。
pub const RATE_LIMITED: &[&str] = &["rate limit", "rate-limit", "too many requests"];

/// Google 内容策略拦截文案的固定开头（小写）。agy 不把拦截当错误，而是把「The prompt could not be submitted. …」当一段普通回复正文
/// 流出来、`result` 报成功；认法照旧版 Miyu 的 `antigravity/stream.rs`（`google_policy_block`）：正文头一段就是这句固定措辞。
/// 旧版还认「prohibited use policy」，本包只认这个开头：拦截文案整段都是模型不会自己写出来的固定字，开头认得出就够。
pub const POLICY_BLOCK: &str = "the prompt could not be submitted";

/// 一段正文开头和拦截文案的关系（[`policy_start`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// 目前写出来的还只是 [`POLICY_BLOCK`] 的一个开头（含还没有字）：要等更多字才看得出。
    Maybe,
    /// 已经是拦截文案：开头写全了 [`POLICY_BLOCK`]。
    Block,
    /// 不是拦截文案。
    No,
}

/// `text` 是一步正文到现在为止攒下的字：开头的空白不算、不分大小写，看它是不是以 [`POLICY_BLOCK`] 开头。
#[must_use]
pub fn policy_start(text: &str) -> Policy {
    let lower = text.trim_start().to_ascii_lowercase();
    if lower.starts_with(POLICY_BLOCK) {
        Policy::Block
    } else if POLICY_BLOCK.starts_with(&lower) {
        Policy::Maybe
    } else {
        Policy::No
    }
}

/// 回答里 `message` 至多多少个字（核心照 500 截）。
pub const MESSAGE_CHARS: usize = 500;

/// 核心认的出错类（参考文档 `driver.call` 的「回应」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `retryable`。
    Retryable,
    /// `rate_limited`。
    RateLimited,
    /// `auth`。
    Auth,
    /// `quota`。
    Quota,
    /// `content_policy`：Google 的内容策略拦下了这条提示词。
    ContentPolicy,
    /// `unclassified`。
    Unclassified,
}

impl Class {
    /// 协议里的写法。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Retryable => "retryable",
            Class::RateLimited => "rate_limited",
            Class::Auth => "auth",
            Class::Quota => "quota",
            Class::ContentPolicy => "content_policy",
            Class::Unclassified => "unclassified",
        }
    }
}

/// agy 报的错 `said` 归哪一类；`delivered` 是这一次已经交过增量。
#[must_use]
pub fn classify(said: &str, delivered: bool) -> Class {
    let lower = said.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    if has(AUTH) {
        Class::Auth
    } else if has(QUOTA) {
        Class::Quota
    } else if has(RATE_LIMITED) {
        Class::RateLimited
    } else {
        otherwise(delivered)
    }
}

/// 认不出原因的出错（包括没有 `result` 就退出了）：还没交过增量的 `retryable`，交过的 `unclassified`。
#[must_use]
pub fn otherwise(delivered: bool) -> Class {
    if delivered {
        Class::Unclassified
    } else {
        Class::Retryable
    }
}

/// 回答里的 `error`：`{"class", "message"}`，`message` 截到 [`MESSAGE_CHARS`] 个字。
#[must_use]
pub fn error(class: Class, message: &str) -> Value {
    let message: String = message.chars().take(MESSAGE_CHARS).collect();
    json!({"class": class.as_str(), "message": message})
}

#[cfg(test)]
mod tests;
