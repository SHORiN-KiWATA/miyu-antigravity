//! 带工具的 `driver.call`：agy 经 MCP 调 Miyu 的工具，这一次交回一块工具调用，下一次带着结果来再回给 agy（设计第四节第 4 条）。
//! 假 agy 照 `plan:` 剧本演（`tests/fake_agy.py`），MCP 服务是真的 `miyu-antigravity mcp`。

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::support::{Core, alive, hex, next_turn, request};
use crate::tool_rig::{listed_tools, sockets, stdin_text, text_blocks, tool_request, with_result};

const ARGUMENTS: &str = r#"{"command":"ls -l","n":2}"#;

fn sha256(text: &str) -> String {
    format!("sha256:{}", hex(&Sha256::digest(text.as_bytes())))
}

fn usage_once() -> Value {
    json!({"uncached": 10, "cache_read": 7, "cache_write": 0, "output": 3, "reasoning": 1, "context": 20})
}

/// 一件工具调用走完：交回、带结果来、接着说完、下一轮还是同一个 agy 进程（编号 1）。
#[test]
fn one_tool_call_round_trips_and_the_same_agy_serves_the_next_round() {
    let mut core = Core::start("t-one", "plan:one");
    let first = tool_request("s", "list the files");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    assert_eq!(
        one.deltas,
        vec![
            json!({"start": {"index": 0, "kind": "text"}}),
            json!({"text": {"index": 0, "text": "Let me look. "}}),
            json!({"end": {"index": 0}}),
            json!({"start": {"index": 1, "kind": {"tool_call": {"name": "bash"}}}}),
            json!({"text": {"index": 1, "text": ARGUMENTS}}),
            json!({"private": {"index": 1, "data": {"mcp": 1}}}),
            json!({"end": {"index": 1}}),
        ]
    );
    assert_eq!(one.response["result"]["usage"], usage_once());
    assert!(
        one.response["result"].get("error").is_none(),
        "{}",
        one.response
    );
    let records = core.records();
    assert_eq!(records.len(), 1);
    let pid = records[0]["pid"].as_i64().unwrap();
    assert!(alive(pid), "agy 那头挂着等结果，进程还活着");

    let result = "file-a\nfile-b";
    let second = with_result(&first, &one, "call_1", text_blocks(result), false);
    core.call("r2", "d2", &second);
    let two = core.collect("r2", "d2");
    assert_eq!(two.text(), "Listing received.");
    assert_eq!(two.response["result"]["usage"], usage_once());
    assert_eq!(
        two.response["result"]["sent"],
        sha256(result),
        "sent 是回给 agy 的那段字"
    );
    let record = core.record_of(pid);
    assert_eq!(
        record["replies"],
        json!([{"content": [{"type": "text", "text": result}], "isError": false}])
    );
    assert_eq!(
        record["calls"],
        json!([{"name": "bash", "arguments": {"command": "ls -l", "n": 2}}])
    );
    assert_eq!(
        record["lines"].as_array().unwrap().len(),
        1,
        "结果不经标准输入"
    );

    // 第三次是普通的新一轮，还是同一个 agy。
    let third = next_turn(&second, "Listing received.", &["thanks"]);
    core.call("r3", "d3", &third);
    assert_eq!(core.collect("r3", "d3").text(), "echo: thanks");
    let records = core.records();
    assert_eq!(records.len(), 1, "没有再起 agy");
    assert_eq!(records[0]["pid"], pid);
    assert_eq!(records[0]["lines"].as_array().unwrap().len(), 2);
    assert_eq!(stdin_text(&records[0], 1), "thanks");
}

/// 连着两件工具调用：交回两次，编号 1、2。
#[test]
fn two_tool_calls_in_a_row_are_handed_back_twice_numbered_one_and_two() {
    let mut core = Core::start("t-two", "plan:two");
    let first = tool_request("s", "go");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    assert_eq!(
        one.deltas[1],
        json!({"text": {"index": 0, "text": ARGUMENTS}})
    );
    assert_eq!(
        one.deltas[2],
        json!({"private": {"index": 0, "data": {"mcp": 1}}})
    );
    let pid = core.records()[0]["pid"].as_i64().unwrap();

    let second = with_result(&first, &one, "call_1", text_blocks("one"), false);
    core.call("r2", "d2", &second);
    let two = core.collect("r2", "d2");
    assert_eq!(
        two.deltas,
        vec![
            json!({"start": {"index": 0, "kind": {"tool_call": {"name": "read"}}}}),
            json!({"text": {"index": 0, "text": r#"{"path":"a.txt"}"#}}),
            json!({"private": {"index": 0, "data": {"mcp": 2}}}),
            json!({"end": {"index": 0}}),
        ]
    );
    assert!(
        two.response["result"].get("error").is_none(),
        "{}",
        two.response
    );
    assert!(alive(pid));

    let third = with_result(&second, &two, "call_2", text_blocks("two"), false);
    core.call("r3", "d3", &third);
    assert_eq!(core.collect("r3", "d3").text(), "both done");
    let records = core.records();
    assert_eq!(records.len(), 1, "三次调用都在同一个 agy 上");
    let replies = records[0]["replies"].as_array().unwrap();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["content"][0]["text"], "two");
}

/// tool 那条 `error: true` 回给 agy 是 `isError: true`。
#[test]
fn a_failed_tool_result_reaches_agy_as_is_error() {
    let mut core = Core::start("t-err", "plan:one");
    let first = tool_request("s", "go");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    let second = with_result(
        &first,
        &one,
        "call_1",
        text_blocks("permission denied"),
        true,
    );
    core.call("r2", "d2", &second);
    core.collect("r2", "d2");
    let record = &core.records()[0];
    assert_eq!(
        record["replies"],
        json!([{"content": [{"type": "text", "text": "permission denied"}], "isError": true}])
    );
}

/// blocks 里有图：文字接起来、图换成一句占位。
#[test]
fn images_in_a_tool_result_become_a_placeholder_line() {
    let mut core = Core::start("t-img", "plan:one");
    let first = tool_request("s", "go");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    let blocks = json!([
        {"type": "text", "text": "before"},
        {"type": "image", "blob": "sha256:abc", "name": "shot.png"},
        {"type": "text", "text": "after"}
    ]);
    let second = with_result(&first, &one, "call_1", blocks, false);
    core.call("r2", "d2", &second);
    let two = core.collect("r2", "d2");
    let text = "before\n[image \"shot.png\" omitted: this model reads text only]\nafter";
    assert_eq!(two.response["result"]["sent"], sha256(text));
    assert_eq!(
        core.records()[0]["replies"][0]["content"],
        json!([{"type": "text", "text": text}])
    );
}

/// `mcp_config.json`：别的服务原样、`miyu` 一条照规矩写、`tools/list` 是整过形的。
#[test]
fn the_mcp_config_keeps_other_servers_and_the_tool_list_is_shaped() {
    let mut core = Core::start_with(
        "t-cfg",
        "plan:one",
        &[],
        &json!({"antigravity.tool_wait_seconds": 120}),
    );
    let file = core.home.join(".gemini/config/mcp_config.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let other = json!({"command": "/usr/bin/other", "args": ["--x"], "env": {"A": "1"}});
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&json!({"theme": "dark", "mcpServers": {"other": other}}))
            .unwrap(),
    )
    .unwrap();

    let mut first = tool_request("s", "go");
    first["tools"][0]["parameters"] = json!({
        "type": "object",
        "properties": {"command": {"type": ["string", "null"], "description": "what to run"}},
        "additionalProperties": false
    });
    core.call("r1", "d1", &first);
    core.collect("r1", "d1");

    let config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(config["theme"], "dark");
    assert_eq!(config["mcpServers"]["other"], other, "别的服务原样在");
    let miyu = &config["mcpServers"]["miyu"];
    let program = std::fs::canonicalize(env!("CARGO_BIN_EXE_miyu-antigravity")).unwrap();
    assert_eq!(miyu["command"], program.to_str().unwrap());
    assert_eq!(miyu["args"], json!(["mcp"]));
    assert_eq!(miyu["timeoutSeconds"], 180, "tool_wait 120 秒加 60");
    assert_eq!(
        miyu["tools"],
        json!({"bash": {"eager": true}, "read": {"eager": true}})
    );
    assert!(miyu.get("env").is_none());

    let pid = core.records()[0]["pid"].as_i64().unwrap();
    let listed = listed_tools(&core, pid);
    assert_eq!(listed["tools"].as_array().unwrap().len(), 2);
    assert_eq!(listed["tools"][0]["name"], "bash");
    assert_eq!(listed["tools"][0]["description"], "Run a shell command");
    assert_eq!(
        listed["tools"][0]["inputSchema"]["properties"]["command"],
        json!({"type": "string", "nullable": true, "description": "what to run"})
    );
    assert!(
        listed["tools"][0]["inputSchema"]
            .get("additionalProperties")
            .is_none()
    );
    assert_eq!(listed["tools"][1]["name"], "read");
}

/// 请求不带工具：不建 `mcp_config.json`；agy 的环境里有那两个变量（总台照样接上、工具表是空的，好让工具服务报家目录）。
#[test]
fn a_request_without_tools_makes_no_mcp_config_but_still_gives_agy_the_variables() {
    let mut core = Core::start("t-none", "echo");
    core.call("r1", "d1", &request("s", "hello there"));
    assert_eq!(core.collect("r1", "d1").text(), "echo: hello there");
    assert!(!core.home.join(".gemini/config/mcp_config.json").exists());
    let record = &core.records()[0];
    assert_eq!(
        record["bridge_env"],
        json!(["MIYU_ANTIGRAVITY_BRIDGE", "MIYU_ANTIGRAVITY_TOKEN"])
    );
    assert!(record.get("mcp_pid").is_none());
    assert_eq!(sockets(&core), 1, "套接字服务端照常起着");
}

/// 文件是坏的 JSON：`unclassified`、原话带路径、文件原样、没起 agy。
#[test]
fn a_broken_mcp_config_fails_the_call_and_is_left_alone() {
    let mut core = Core::start("t-bad", "plan:one");
    let file = core.home.join(".gemini/config/mcp_config.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{ not json").unwrap();
    core.call("r1", "d1", &tool_request("s", "go"));
    let one = core.collect("r1", "d1");
    let error = &one.response["result"]["error"];
    assert_eq!(error["class"], "unclassified", "{}", one.response);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains(file.to_str().unwrap()), "{message}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
    assert!(core.records().is_empty(), "没起 agy");
    assert_eq!(core.work_dirs_left(), 0);
}

/// 扩展正常退出（核心关它的标准输入：停核心、关掉或卸掉这个包）：自己登记的 `miyu` 一条拿掉，别的服务原样。卸了包以后
/// agy 的配置里不留一条指着已经删掉的程序的服务（审计 #163）。
#[test]
fn the_miyu_entry_is_removed_when_the_extension_stops() {
    let mut core = Core::start("t-rel", "plan:one");
    let file = core.home.join(".gemini/config/mcp_config.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let other = json!({"command": "/usr/bin/other", "args": ["--x"]});
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&json!({"theme": "dark", "mcpServers": {"other": other}}))
            .unwrap(),
    )
    .unwrap();
    core.call("r1", "d1", &tool_request("s", "go"));
    core.collect("r1", "d1");
    let config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(config["mcpServers"]["miyu"].is_object(), "跑着的时候在");

    assert!(core.finish().success());
    let config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(config["mcpServers"].get("miyu").is_none(), "{config}");
    assert_eq!(config["mcpServers"]["other"], other, "别的服务原样在");
    assert_eq!(config["theme"], "dark");
}

/// 退出时那一条已经不是指着本程序的了（另一处装的 Miyu 登记的）：不动它。
#[test]
fn an_entry_that_points_at_another_program_is_left_when_the_extension_stops() {
    let mut core = Core::start("t-rel-o", "plan:one");
    let file = core.home.join(".gemini/config/mcp_config.json");
    core.call("r1", "d1", &tool_request("s", "go"));
    core.collect("r1", "d1");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    config["mcpServers"]["miyu"]["command"] = json!("/somewhere/else/miyu-antigravity");
    let text = serde_json::to_string_pretty(&config).unwrap();
    std::fs::write(&file, &text).unwrap();

    assert!(core.finish().success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text, "一个字没动");
}

/// 文件是 0 字节的（agy 头一次运行自己建的就是这样）：照常登记、照常调用。
#[test]
fn an_empty_mcp_config_is_filled_in_and_the_call_goes_through() {
    let mut core = Core::start("t-empty", "plan:one");
    let file = core.home.join(".gemini/config/mcp_config.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "").unwrap();
    core.call("r1", "d1", &tool_request("s", "go"));
    let one = core.collect("r1", "d1");
    assert!(
        one.response["result"].get("error").is_none(),
        "{}",
        one.response
    );
    let config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(config["mcpServers"]["miyu"]["args"], json!(["mcp"]));
    assert_eq!(core.records().len(), 1, "起了 agy");
}

/// 套接字起不来（路径太长）：扩展不退出，不带工具的照常，带工具的回出错、原话带原因。
#[test]
fn when_the_socket_cannot_be_made_only_calls_with_tools_fail() {
    let long = format!("/tmp/ag-{}", "x".repeat(120));
    let mut core = Core::start_with(
        "t-sock",
        "echo",
        &[("MIYU_RUNTIME_DIR", long.as_str())],
        &json!({}),
    );
    core.call("r1", "d1", &request("s", "plain"));
    assert_eq!(core.collect("r1", "d1").text(), "echo: plain");
    core.call("r2", "d2", &tool_request("s2", "with tools"));
    let two = core.collect("r2", "d2");
    let error = &two.response["result"]["error"];
    assert_eq!(error["class"], "unclassified", "{}", two.response);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("over the 100-byte limit"),
        "{error}"
    );
    assert_eq!(core.records().len(), 1, "带工具的没起 agy");
    assert!(!core.home.join(".gemini/config/mcp_config.json").exists());
}
