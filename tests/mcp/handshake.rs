use std::time::Duration;

use serde_json::json;

use crate::support::Agy;

const NO_BRIDGE: &str =
    "This tool server is not attached to a Miyu session, so it cannot run tools.";

/// 照真 agy 1.4.0 的顺序握手。
#[test]
fn the_real_handshake_order_works() {
    let mut agy = Agy::start(None);
    let discover = agy.ask(1, "server/discover", &json!({}));
    assert_eq!(discover["error"]["code"], -32601, "{discover}");
    assert!(discover.get("result").is_none());

    agy.notify("notifications/roots/list_changed");
    agy.assert_silent(Duration::from_millis(200));

    let init = agy.ask(
        2,
        "initialize",
        &json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "agy"}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(init["result"]["capabilities"], json!({"tools": {}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "miyu-antigravity");
    assert_eq!(
        init["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );

    agy.notify("notifications/initialized");
    agy.assert_silent(Duration::from_millis(200));
    let list = agy.ask(3, "tools/list", &json!({}));
    assert_eq!(list["result"], json!({"tools": []}));

    agy.notify("notifications/roots/list_changed");
    agy.assert_silent(Duration::from_millis(200));
    let ping = agy.ask(4, "ping", &json!({}));
    assert_eq!(ping["error"]["code"], -32601);
}

#[test]
fn initialize_writes_back_any_version_and_defaults_when_missing() {
    let mut agy = Agy::start(None);
    let other = agy.ask(1, "initialize", &json!({"protocolVersion": "2024-11-05"}));
    assert_eq!(other["result"]["protocolVersion"], "2024-11-05");
    let none = agy.ask(2, "initialize", &json!({}));
    assert_eq!(none["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn lines_that_are_not_json_are_skipped_and_ids_of_any_kind_are_echoed() {
    let mut agy = Agy::start(None);
    agy.send_line("this is not json");
    agy.send_line("");
    agy.send_line("[1, 2]");
    agy.send(&json!({"jsonrpc": "2.0", "id": "abc", "method": "tools/list"}));
    let reply = agy.recv();
    assert_eq!(reply["id"], "abc");
    assert_eq!(reply["result"], json!({"tools": []}));
}

#[test]
fn a_tools_call_without_a_name_is_an_invalid_params_error() {
    let mut agy = Agy::start(None);
    let reply = agy.ask(1, "tools/call", &json!({"arguments": {}}));
    assert_eq!(reply["error"]["code"], -32602, "{reply}");
}

#[test]
fn without_the_environment_variables_the_list_is_empty_and_calls_fail_as_tool_errors() {
    let mut agy = Agy::start(None);
    let list = agy.ask(1, "tools/list", &json!({}));
    assert_eq!(list["result"], json!({"tools": []}));
    let call = agy.ask(
        2,
        "tools/call",
        &json!({"name": "bash", "arguments": {"command": "ls"}}),
    );
    assert_eq!(
        call["result"],
        json!({"content": [{"type": "text", "text": NO_BRIDGE}], "isError": true})
    );
    assert!(call.get("error").is_none(), "工具失败不回 JSON-RPC 错误");
}

#[test]
fn with_only_one_of_the_variables_it_is_the_same_as_none() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_miyu-antigravity"))
        .arg("mcp")
        .env("MIYU_ANTIGRAVITY_TOKEN", "t")
        .env_remove("MIYU_ANTIGRAVITY_BRIDGE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#).unwrap();
    // 读到回话再关标准输入：关了就退出，不等还在办的请求。
    let mut text = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut text)
        .unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let reply: serde_json::Value = serde_json::from_str(text.trim_end()).unwrap();
    assert_eq!(
        reply,
        json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": []}})
    );
}
