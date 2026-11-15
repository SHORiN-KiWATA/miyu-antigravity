use std::time::Duration;

use serde_json::{Value, json};

use crate::support::{Agy, Rig, TOKEN, tool};

const GONE: &str = "The Miyu side of this tool call is gone, so the call did not complete.";

fn gone() -> Value {
    json!({"content": [{"type": "text", "text": GONE}], "isError": true})
}

#[test]
fn tools_list_returns_the_extensions_table_as_is() {
    let rig = Rig::new("m-list");
    let mut agy = rig.agy();
    let reply = agy.ask(1, "tools/list", &json!({}));
    assert_eq!(
        reply["result"],
        json!({"tools": [tool("bash").to_json(), tool("read").to_json()]})
    );
}

#[test]
fn tools_call_waits_for_the_reply_and_answers_it_verbatim() {
    let rig = Rig::new("m-call");
    let mut agy = rig.agy();
    let arguments = json!({"command": "ls -l", "n": 2, "nested": {"a": [1, null, "x"]}});
    agy.request(
        7,
        "tools/call",
        &json!({"name": "bash", "arguments": arguments}),
    );
    let call = rig.next_call();
    assert_eq!(call.name, "bash");
    assert_eq!(call.arguments, arguments, "arguments 原文");
    agy.assert_silent(Duration::from_millis(300));
    let content = vec![
        json!({"type": "text", "text": "line one"}),
        json!({"type": "text", "text": "line two"}),
    ];
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        call.reply.send(&content, false).unwrap();
    });
    let reply = agy.recv();
    assert_eq!(reply["id"], 7);
    assert_eq!(
        reply["result"],
        json!({"content": [
            {"type": "text", "text": "line one"},
            {"type": "text", "text": "line two"}
        ], "isError": false})
    );
}

#[test]
fn a_tool_error_from_the_extension_stays_an_is_error_result() {
    let rig = Rig::new("m-iserr");
    let mut agy = rig.agy();
    agy.request(1, "tools/call", &json!({"name": "bash", "arguments": {}}));
    rig.next_call()
        .reply
        .send(&[json!({"type": "text", "text": "exit 2"})], true)
        .unwrap();
    let reply = agy.recv();
    assert_eq!(
        reply["result"],
        json!({"content": [{"type": "text", "text": "exit 2"}], "isError": true})
    );
    assert!(reply.get("error").is_none());
}

#[test]
fn a_call_without_arguments_reaches_the_extension_as_an_empty_object() {
    let rig = Rig::new("m-noargs");
    let mut agy = rig.agy();
    agy.request(1, "tools/call", &json!({"name": "read"}));
    let call = rig.next_call();
    assert_eq!(call.arguments, json!({}));
    call.reply.send(&[], false).unwrap();
    assert_eq!(
        agy.recv()["result"],
        json!({"content": [], "isError": false})
    );
}

#[test]
fn the_extension_going_away_while_waiting_is_an_is_error_result() {
    let rig = Rig::new("m-away");
    let mut agy = rig.agy();
    agy.request(1, "tools/call", &json!({"name": "bash", "arguments": {}}));
    let call = rig.next_call();
    drop(call.reply);
    let reply = agy.recv();
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"], gone());
}

#[test]
fn a_wrong_token_or_an_unreachable_socket_degrades_without_a_json_rpc_error() {
    let rig = Rig::new("m-bad");
    let mut wrong = Agy::start(Some((&rig.path, "wrong-token")));
    assert_eq!(
        wrong.ask(1, "tools/list", &json!({}))["result"],
        json!({"tools": []})
    );
    let reply = wrong.ask(2, "tools/call", &json!({"name": "bash", "arguments": {}}));
    assert_eq!(reply["result"], gone());

    let missing = rig.dir.join("nobody-home.sock");
    let mut lost = Agy::start(Some((&missing, TOKEN)));
    assert_eq!(
        lost.ask(1, "tools/list", &json!({}))["result"],
        json!({"tools": []})
    );
    let reply = lost.ask(2, "tools/call", &json!({"name": "bash", "arguments": {}}));
    assert_eq!(reply["result"], gone());
}

#[test]
fn other_requests_are_answered_while_a_call_hangs() {
    let rig = Rig::new("m-hang");
    let mut agy = rig.agy();
    agy.request(10, "tools/call", &json!({"name": "bash", "arguments": {}}));
    let first = rig.next_call();

    let list = agy.ask(11, "tools/list", &json!({}));
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 2);
    let init = agy.ask(12, "initialize", &json!({"protocolVersion": "2025-11-25"}));
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");

    // 再来一条 tools/call 也照样转给扩展，先回的先答。
    agy.request(
        13,
        "tools/call",
        &json!({"name": "read", "arguments": {"p": 1}}),
    );
    let second = rig.next_call();
    assert_eq!(second.name, "read");
    second
        .reply
        .send(&[json!({"type": "text", "text": "second"})], false)
        .unwrap();
    let reply = agy.recv();
    assert_eq!(reply["id"], 13);
    assert_eq!(reply["result"]["content"][0]["text"], "second");

    first
        .reply
        .send(&[json!({"type": "text", "text": "first"})], false)
        .unwrap();
    let reply = agy.recv();
    assert_eq!(reply["id"], 10);
    assert_eq!(reply["result"]["content"][0]["text"], "first");
}

#[test]
fn closing_stdin_exits_with_code_zero_even_with_a_call_hanging() {
    let rig = Rig::new("m-eof");
    let mut agy = rig.agy();
    agy.request(1, "tools/call", &json!({"name": "bash", "arguments": {}}));
    let call = rig.next_call();
    agy.close_stdin();
    let status = agy.wait_exit();
    assert_eq!(status.code(), Some(0));
    // 挂着的那条连接随进程退出断了：扩展那头看得出来。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !call.reply.is_closed() {
        assert!(
            std::time::Instant::now() < deadline,
            "扩展没发现 MCP 服务走了"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn closing_stdin_with_nothing_pending_exits_with_code_zero() {
    let mut agy = Agy::start(None);
    agy.close_stdin();
    assert_eq!(agy.wait_exit().code(), Some(0));
}
