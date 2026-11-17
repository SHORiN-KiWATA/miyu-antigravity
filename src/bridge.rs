//! 扩展进程和 MCP 服务（`miyu-antigravity mcp`，agy 拉起）之间的本机套接字（设计第三节的「本机套接字（带口令）」）。
//!
//! 协议：Unix 域套接字，**一次操作一条连接，一行一条 JSON**（一行至多 [`crate::process::LINE_LIMIT`] 字节）。
//!
//! 客户端（MCP 服务）连上以后第一行发其中一种，然后一直等服务端回一行：
//!
//! | 请求 | 回话 |
//! |---|---|
//! | `{"token": "<口令>", "op": "list"[, "home": "<路径>"]}` | `{"tools": [{"name", "description", "inputSchema"}, ..]}` |
//! | `{"token": "<口令>", "op": "call", "name": "<工具名>", "arguments": <原文>[, "home": "<路径>"]}` | `{"content": [..], "isError": bool}`，就是要答给 agy 的 `result` |
//! | 口令不认识、写法不对 | `{"error": "<原因>"}`，随后服务端关连接 |
//!
//! 和任务书比有四处补充：
//!
//! - `call` 在回话之前，**客户端要一直开着连接**（不要先半关写端）：服务端靠读到对面关了来发现客户端走了（[`Reply::is_closed`]）。
//! - 服务端丢掉 [`Reply`] 而没回话，连接直接关掉，客户端读到头；`call` 的口令不认识，由处理者调 [`Reply::refuse`]。
//! - 连上以后 10 秒内没发来第一行的，服务端当它坏了、关掉（[`server::FIRST_LINE_WAIT`]）。
//! - `list`、`call` 都可以多带一格可选的 `"home"`：MCP 服务自己环境里的 `HOME`，也就是 agy 实际用的家目录（主人的包装脚本可能给
//!   agy 换了 `HOME`，agy 的对话数据就落在那里）。MCP 服务这边只在 `HOME` 非空、是绝对路径时才带（[`home_from`]、[`with_home`]）；
//!   服务端这边这一格不是字符串、是空的、不是绝对路径的当没带，不报错（[`home_of`]），老的 MCP 服务不带这一格照常。
//!   **安全上**：这一格是拿着口令的进程（agy 的子进程）报的，扩展只用它拼
//!   `<home>/.gemini/antigravity-cli/{conversations/<编号>.db*, brain/<编号>/}`（编号照旧过 [`crate::workspace::valid_conversation`]），
//!   口令不认识的连接报的一概不理（[`Handler::home`] 只对认识的口令生效）。
//!
//! 套接字文件在 `MIYU_RUNTIME_DIR` 下（没有这个变量用 `MIYU_STATE_DIR`），叫 `antigravity-<进程号>.sock`，权限 0600。口令是随机
//! 的（[`new_token`]），扩展起 agy 时连同套接字路径放进 agy 的环境变量（[`ENV_BRIDGE`]、[`ENV_TOKEN`]），agy 再原样传给 MCP 服务。
//!
//! 服务端（[`Server`]）只管接连接、验写法、交给处理者（[`Handler`]），和一次调用怎么走无关；客户端是 [`client::exchange`]。

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub mod client;
pub mod server;

pub use server::{Handler, Reply, Server};

/// agy 传给 MCP 服务的环境变量：套接字路径。
pub const ENV_BRIDGE: &str = "MIYU_ANTIGRAVITY_BRIDGE";

/// agy 传给 MCP 服务的环境变量：口令。
pub const ENV_TOKEN: &str = "MIYU_ANTIGRAVITY_TOKEN";

/// 套接字路径至多多少字节：`sockaddr_un.sun_path` 在 Linux 是 108、macOS 是 104，留余量。
pub const MAX_PATH_BYTES: usize = 100;

/// 口令用多少字节随机数（十六进制写出来是两倍长）：128 位。
const TOKEN_BYTES: usize = 16;

/// 一件工具，答 `list` 用。
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    /// 工具名。
    pub name: String,
    /// 说明。
    pub description: String,
    /// 参数格式（JSON Schema，已经整形过的 [`crate::schema::shape`]）。
    pub input_schema: Value,
}

impl Tool {
    /// 线上的写法：`{"name", "description", "inputSchema"}`。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
        })
    }
}

/// 套接字文件的位置：`MIYU_RUNTIME_DIR` 下（没有、空的用 `MIYU_STATE_DIR`），名字带本进程的进程号。
///
/// # Errors
///
/// 两个变量都没有：说明缺什么。
pub fn socket_path() -> std::io::Result<PathBuf> {
    socket_path_in(
        std::env::var_os("MIYU_RUNTIME_DIR"),
        std::env::var_os("MIYU_STATE_DIR"),
        std::process::id(),
    )
    .ok_or_else(|| std::io::Error::other("neither MIYU_RUNTIME_DIR nor MIYU_STATE_DIR is set"))
}

/// [`socket_path`] 的纯的部分：`runtime`、`state` 是两个变量的值，空的当没设。
fn socket_path_in(
    runtime: Option<std::ffi::OsString>,
    state: Option<std::ffi::OsString>,
    pid: u32,
) -> Option<PathBuf> {
    let dir = runtime
        .filter(|dir| !dir.is_empty())
        .or_else(|| state.filter(|dir| !dir.is_empty()))?;
    Some(Path::new(&dir).join(format!("antigravity-{pid}.sock")))
}

/// 新的随机口令：从 `/dev/urandom` 读 128 位，写成小写十六进制（32 个字符）。
///
/// # Errors
///
/// 读不了 `/dev/urandom`：原样交。
pub fn new_token() -> std::io::Result<String> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(crate::agent::hex(&bytes))
}

/// 环境变量 `HOME` 的值里能报给扩展的：非空、是绝对路径、是 UTF-8（写进 JSON 要用字）。
#[must_use]
pub fn home_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value
        .filter(|home| !home.is_empty())
        .and_then(|home| home.into_string().ok())
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// 给请求 `request`（`list` 或 `call`）加上 `"home"` 一格；`home` 是 `None`（或不是 UTF-8）的原样交回。
#[must_use]
pub fn with_home(mut request: Value, home: Option<&Path>) -> Value {
    if let (Some(object), Some(home)) = (request.as_object_mut(), home.and_then(Path::to_str)) {
        object.insert("home".into(), json!(home));
    }
    request
}

/// 请求里的 `"home"` 一格：是字、非空、是绝对路径的才算；别的（没有、不是字、空的、相对路径）都当没带。
#[must_use]
pub fn home_of(request: &Value) -> Option<PathBuf> {
    request
        .get("home")
        .and_then(Value::as_str)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// `list` 请求。
#[must_use]
pub fn list_request(token: &str) -> Value {
    json!({"token": token, "op": "list"})
}

/// `call` 请求：`arguments` 照原文放进去。
#[must_use]
pub fn call_request(token: &str, name: &str, arguments: &Value) -> Value {
    json!({"token": token, "op": "call", "name": name, "arguments": arguments})
}

#[cfg(test)]
mod tests;
