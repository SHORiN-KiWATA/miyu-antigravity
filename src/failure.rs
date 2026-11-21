//! agy 出错归成核心认的类（设计第八节）。纯函数。
//!
//! | 字里有 | 归成 |
//! |---|---|
//! | [`CONTEXT_TOO_LONG`] 里的一种 | `context_too_long`（核心先压缩再来） |
//! | [`REJECTED`] 里的一种 | `unclassified`（请求本身有错） |
//! | [`AUTH`] 里的一种 | `auth` |
//! | [`QUOTA`] 里的一种 | `quota` |
//! | [`RATE_LIMITED`] 里的一种 | `rate_limited` |
//! | 别的 | 还没交过增量的 `retryable`，交过的 `unclassified` |
//!
//! 重试由核心照类决定（核心 `retry.rs`：只有 `retryable`、`rate_limited` 这几类再来，`auth`、`unclassified` 不来，`context_too_long` 先压缩；
//! 经池调用的 `auth` 还会冷却这个模型、换下一个）。所以再来也一样的错（账号被停用、请求本身有错）不能落到「别的」那一格。
//!
//! 内容策略拦截不走这张表：agy 把 Google 的拦截文案当正文流出来、`result` 还报成功，认法见 [`policy_start`]。
//!
//! 先后照表：几种都有的取靠前的。上下文超长也是 400，要排在 [`REJECTED`] 前面；[`REJECTED`] 排在其余几组前面：请求有错的原话会点出出错的属性名、工具名（`properties[quota]`），
//! 名字里的字眼不该把它归成额度、认证。不分大小写。只看 `result.error`、`error_message` 步的字：agy 的标准错误每次起来都印一行
//! 「not logged into Antigravity」再悄悄登录成功，拿它判会误杀（旧版 `stream.rs` 记的）。

use serde_json::{Value, json};

/// 上下文超长的字眼。Gemini 超长时报 400 `INVALID_ARGUMENT`，原话是「The input token count (N) exceeds the maximum number of
/// tokens allowed (M).」（Google 官方开发者论坛的多份报告）；Vertex AI 的出错说明把「Request exceeds the model's input token
/// limit」列为 400 的例子，旧的 Vertex 写法是「Input request contains (N) tokens, which is more than the max tokens limit」。
/// 只认这几句说的是输入的 token 数，别的 400（工具参数格式、输出上限写错）照旧归 [`REJECTED`]。
pub const CONTEXT_TOO_LONG: &[&str] = &[
    "exceeds the maximum number of tokens allowed",
    "input token limit",
    "which is more than the max tokens limit",
];

/// 请求本身有错的字眼：agy 照 gRPC 的状态名报（`INVALID_ARGUMENT (code 400): …`，2026-11-20 主人本机）。Gemini 的
/// 出错说明里 400、404 是请求写错、模型不存在，`failed_precondition` 是账号或地区的前提不满足，都要人改了才行。
pub const REJECTED: &[&str] = &[
    "invalid_argument",
    "failed_precondition",
    "not_found",
    "(code 400)",
    "(code 404)",
];

/// 没登录、账号用不了的字眼。账号被停用的原话（2026-11-20 主人本机）：「This service has been disabled in this account for
/// violation of Terms of Service. Please submit an appeal to continue」；`permission_denied`（403）是账号没有权限。
pub const AUTH: &[&str] = &[
    "not logged in",
    "unauthenticated",
    "login required",
    "sign in",
    "authentication",
    "terms of service",
    "submit an appeal",
    "permission_denied",
    "(code 401)",
    "(code 403)",
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
    /// `context_too_long`：输入超过了模型的上下文，核心先压缩再来。
    ContextTooLong,
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
            Class::ContextTooLong => "context_too_long",
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
    if has(CONTEXT_TOO_LONG) {
        Class::ContextTooLong
    } else if has(REJECTED) {
        Class::Unclassified
    } else if has(AUTH) {
        Class::Auth
    } else if has(QUOTA) {
        Class::Quota
    } else if has(RATE_LIMITED) {
        Class::RateLimited
    } else {
        otherwise(delivered)
    }
}

/// 认不出原因的出错（包括没有 `result` 就退出了、5xx 这种对面一时的毛病）：还没交过增量的 `retryable`，交过的 `unclassified`。
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
