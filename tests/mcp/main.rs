//! 集成测试：起 `miyu-antigravity mcp`，测试扮 agy（标准输入输出上的 JSON-RPC），另起库里的套接字服务端扮扩展进程。
//!
//! - `handshake`：照真 agy 的顺序握手、没有环境变量时的空表和 `isError`。
//! - `agy_home`：`mcp` 随每次请求把自己的 `HOME` 报给扩展；老的不带这一格照常。
//! - `bridge`：有环境变量时 `tools/list`、`tools/call` 经套接字转给扩展；等回话、扩展断了、挂着时别的请求照答、标准输入关了退出。

#[cfg(test)]
mod agy_home;
#[cfg(test)]
mod bridge;
#[cfg(test)]
mod handshake;
#[cfg(test)]
mod support;
