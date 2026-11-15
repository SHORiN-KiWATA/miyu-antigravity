//! 带工具的集成测试共用的：请求、把一次调用交上来的增量拼回 assistant 消息（核心就是这么记的）、读假 agy 的记录。

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::support::{Collected, Core, WAIT};

/// 带两件工具（`bash`、`read`）的最小请求：系统提示词 `system`，人说一句 `text`。
pub fn tool_request(system: &str, text: &str) -> Value {
    json!({
        "tools": [
            {"name": "bash", "description": "Run a shell command", "parameters": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }},
            {"name": "read", "description": "Read a file", "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}}
            }}
        ],
        "system": system,
        "messages": [{"role": "user", "blocks": [{"type": "text", "text": text}]}],
        "stable": 0
    })
}

/// 把一次调用交上来的增量拼成 assistant 消息：正文块照样，工具调用块带 `call_id`、名字、参数原文和私有数据
/// （`{"driver": "antigravity", "data": ...}`）。
pub fn assistant_message(got: &Collected, call_id: &str) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    for delta in &got.deltas {
        if let Some(start) = delta.get("start") {
            let kind = &start["kind"];
            blocks.push(if kind == "text" {
                json!({"type": "text", "text": ""})
            } else {
                json!({"type": "tool_call", "call_id": call_id, "name": kind["tool_call"]["name"], "args": ""})
            });
        } else if let Some(text) = delta.get("text") {
            let last = blocks.last_mut().unwrap();
            let key = if last["type"] == "text" {
                "text"
            } else {
                "args"
            };
            let joined = format!(
                "{}{}",
                last[key].as_str().unwrap(),
                text["text"].as_str().unwrap()
            );
            last[key] = json!(joined);
        } else if let Some(private) = delta.get("private") {
            blocks.last_mut().unwrap()["private"] =
                json!({"driver": "antigravity", "data": private["data"]});
        }
    }
    json!({"role": "assistant", "blocks": blocks})
}

/// 一条 tool 消息。
pub fn tool_message(call_id: &str, blocks: Value, error: bool) -> Value {
    let mut message = json!({"role": "tool", "call_id": call_id, "error": error});
    message["blocks"] = blocks;
    message
}

/// 只有一块字的 `blocks`。
pub fn text_blocks(text: &str) -> Value {
    json!([{"type": "text", "text": text}])
}

/// 在 `request` 后面接上 `got` 交上来的 assistant 消息和一条 tool 结果：核心执行完工具以后发的下一次请求。
pub fn with_result(
    request: &Value,
    got: &Collected,
    call_id: &str,
    blocks: Value,
    error: bool,
) -> Value {
    let mut next = request.clone();
    let messages = next["messages"].as_array_mut().unwrap();
    messages.push(assistant_message(got, call_id));
    messages.push(tool_message(call_id, blocks, error));
    next
}

/// 第 `index` 行标准输入里最后一个 text 块的字。
pub fn stdin_text(record: &Value, index: usize) -> String {
    let line = record["lines"][index].as_str().unwrap();
    let message: Value = serde_json::from_str(line).unwrap();
    message["message"]["content"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string()
}

/// 等到至少有 `count` 个假 agy 写了记录（每个读到第一行标准输入时写）。
pub fn wait_records(core: &Core, count: usize) -> Vec<Value> {
    let deadline = Instant::now() + WAIT;
    loop {
        let records = core.records();
        if records.len() >= count {
            return records;
        }
        assert!(
            Instant::now() < deadline,
            "假 agy 只有 {} 个记录",
            records.len()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 不是 `old` 的那个假 agy 的记录（新开的）。
pub fn record_other_than(core: &Core, old: i64) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(record) = core
            .records()
            .into_iter()
            .find(|record| record["pid"] != old)
        {
            return record;
        }
        assert!(Instant::now() < deadline, "没有新起的假 agy");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 运行目录里套接字文件有几个。
pub fn sockets(core: &Core) -> usize {
    std::fs::read_dir(&core.runtime)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "sock")
        })
        .count()
}

/// 假 agy 进程 `pid` 的 MCP 服务拿到的 `tools/list` 结果。
pub fn listed_tools(core: &Core, pid: i64) -> Value {
    let text = std::fs::read_to_string(core.record.join(format!("{pid}.tools"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// 假 agy 进程的 MCP 服务的进程号。
pub fn mcp_pid(record: &Value) -> i64 {
    record["mcp_pid"].as_i64().unwrap()
}

/// 发 `driver.cancel`。
pub fn cancel(core: &mut Core, call: &str) {
    core.send(&json!({"jsonrpc": "2.0", "method": "driver.cancel", "params": {"call": call}}));
}
