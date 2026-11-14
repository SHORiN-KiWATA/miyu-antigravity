//! 集成测试：起 `miyu-antigravity serve`，测试扮核心，假 agy 扮 agy（`support`、`tests/fake_agy.py`）。
//!
//! - `calls`：握手、`driver.models`、一问一答的 `driver.call`（增量、用量、命令行、标准输入、代理文件、收尾）、同时几个。
//! - `failures`：取消、出错归类、往坏里测（夹着不是 JSON 的行、超长的一行、代理对不上、不读标准输入）、配置。
//! - `turns`：同一段对话的下一轮接在活着的 agy 上，认不上的新开。
//! - `teardown`：作废（出错、取消、流断了）和回收（agy 自己退了、闲置到点、超过 4 段、扩展退出、旧的代理目录）。

#[cfg(test)]
mod calls;
#[cfg(test)]
mod failures;
#[cfg(test)]
mod support;
#[cfg(test)]
mod teardown;
#[cfg(test)]
mod turns;
