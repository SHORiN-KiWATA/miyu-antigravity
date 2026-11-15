//! miyu-antigravity：Miyu 的扩展包 `antigravity`，把本机的 Antigravity CLI（`agy`）当模型供应商（`docs/00-设计.md`）。
//!
//! - `miyu-antigravity serve`：核心拉起的扩展，提供模型驱动 `antigravity/antigravity`（[`rpc`]）。一次 `driver.call`（[`call`]）
//!   认得出是哪段活着的 agy 对话（[`sessions`]、[`registry`]）的下一轮的，只把新的话写进那个 agy（[`live`]）；认不上的新开
//!   一段，整份历史转写成一条文字（[`transcript`]）。agy 的流（[`stream`]）照 [`turn`] 交成增量。
//! - `miyu-antigravity mcp`：agy 拉起的 MCP 服务（[`mcp`]），经本机套接字（[`bridge`]）把工具表和工具调用转给扩展进程；工具
//!   参数格式照 [`schema`] 整形，往 agy 的 `mcp_config.json` 登记照 [`mcp_config`]。
//! - `miyu-antigravity unregister`：撤掉写进 agy 配置的东西（[`unregister`]）。
//!
//! 做成库加一个薄的 `main`：集成测试要直接用套接字服务端（[`bridge`]）扮扩展。
//!
//! 这一版是施工 AG-4：一次调用带工具时，agy 经 MCP 服务调 Miyu 的工具，扩展把它当工具调用交回核心（总台 [`hub`]），下一次请求
//! 带着结果来再回给 agy；agy 的原生工具只开 `search_web`，流里 agy 自己跑的工具照实交成供应商侧工具块，结果从 agy 的转录
//! （[`brain`]）读；正文是 Google 内容策略拦截文案的回 `content_policy`（[`failure`]）。

pub mod agent;
pub mod agy;
pub mod brain;
pub mod bridge;
pub mod call;
pub mod config;
pub mod failure;
pub mod hub;
pub mod leftovers;
pub mod live;
pub mod mcp;
pub mod mcp_config;
pub mod models;
pub mod process;
pub mod registry;
pub mod rpc;
pub mod schema;
pub mod sessions;
pub mod stream;
pub mod transcript;
pub mod turn;
pub mod unregister;
pub mod words;
pub mod workspace;
