use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::support::{Core, assert_conversation_removed, request, wait_gone};

/// 照 `mode` 起一个，调一次，交回应里的 `error`。
fn error_of(mode: &str) -> Value {
    let mut core = Core::start("error", mode);
    core.call("r1", "d1", &request("s", "hi"));
    let got = core.collect("r1", "d1");
    assert!(
        got.response["result"].get("usage").is_none(),
        "{}",
        got.response
    );
    got.response["result"]["error"].clone()
}

#[test]
fn errors_are_classified() {
    let cases = [
        ("fail:You are not logged into Antigravity", "auth"),
        ("fail:RESOURCE_EXHAUSTED: Quota exceeded", "quota"),
        ("fail:Rate limit exceeded", "rate_limited"),
        ("fail:internal server hiccup", "retryable"),
        ("noresult", "retryable"),
        ("noresult-after-text", "unclassified"),
    ];
    for (mode, class) in cases {
        let error = error_of(mode);
        assert_eq!(error["class"], class, "{mode}: {error}");
    }
    let error = error_of("fail:internal server hiccup");
    assert_eq!(
        error["message"],
        "agy failed (ERROR): internal server hiccup"
    );
    assert_eq!(
        error_of("noresult")["message"],
        "agy exited without a result"
    );
}

#[test]
fn a_missing_agy_says_how_to_configure_it() {
    let mut core = Core::start("missing", "sample:real-pong.jsonl");
    let missing = core.state.join("no-such-agy");
    core.send(&json!({"jsonrpc": "2.0", "method": "extension.config", "params": {"keys": {"antigravity.binary": missing.to_str().unwrap()}}}));
    core.call("r1", "d1", &request("s", "hi"));
    let error = core.collect("r1", "d1").response["result"]["error"].clone();
    assert_eq!(error["class"], "unclassified");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("was not found") && message.contains("antigravity.binary"),
        "{message}"
    );
    assert_eq!(core.work_dirs_left(), 0, "起不来也删工作目录");

    // 改回来就又能用了。
    let binary = core.binary.to_str().unwrap().to_string();
    core.send(&json!({"jsonrpc": "2.0", "method": "extension.config", "params": {"keys": {"antigravity.binary": binary}}}));
    core.call("r2", "d2", &request("s", "hi"));
    assert_eq!(core.collect("r2", "d2").text(), "pong\n");
}

#[test]
fn cancel_stops_agy_and_its_children() {
    let mut core = Core::start("cancel", "hang");
    core.call("r1", "d1", &request("s", "hi"));
    let record = core.wait_record("child");
    core.send(&json!({"jsonrpc": "2.0", "method": "driver.cancel", "params": {"call": "d1"}}));
    let got = core.collect("r1", "d1");
    assert_eq!(
        got.response["result"]["error"]["message"],
        "the call was cancelled"
    );
    wait_gone(record["pid"].as_i64().unwrap());
    wait_gone(record["child"].as_i64().unwrap());
    assert_conversation_removed(&core.home, record["conversation"].as_str().unwrap());
    assert_eq!(core.work_dirs_left(), 0);
    // 不认识的调用的取消不理。
    core.send(&json!({"jsonrpc": "2.0", "method": "driver.cancel", "params": {"call": "nope"}}));
    assert!(core.finish().success());
}

#[test]
fn cancel_kills_agy_that_ignores_sigterm() {
    let mut core = Core::start("stubborn", "stubborn");
    core.call("r1", "d1", &request("s", "hi"));
    let record = core.wait_record("ready");
    let started = Instant::now();
    core.send(&json!({"jsonrpc": "2.0", "method": "driver.cancel", "params": {"call": "d1"}}));
    core.collect("r1", "d1");
    assert!(
        started.elapsed() >= Duration::from_secs(5),
        "先等了 SIGTERM 的宽限"
    );
    wait_gone(record["pid"].as_i64().unwrap());
}

#[test]
fn quitting_stops_running_calls() {
    let mut core = Core::start("quit", "hang");
    core.call("r1", "d1", &request("s", "hi"));
    let record = core.wait_record("child");
    assert!(core.finish().success());
    wait_gone(record["pid"].as_i64().unwrap());
    wait_gone(record["child"].as_i64().unwrap());
}

#[test]
fn a_wrong_agent_is_unclassified_and_agy_is_stopped() {
    let mut core = Core::start("agent", "badagent");
    core.call("r1", "d1", &request("s", "hi"));
    let got = core.collect("r1", "d1");
    let error = &got.response["result"]["error"];
    assert_eq!(error["class"], "unclassified");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("\"someone-else\"") && message.contains("miyu-"),
        "{message}"
    );
    let record = &core.records()[0];
    wait_gone(record["pid"].as_i64().unwrap());
    assert_conversation_removed(&core.home, record["conversation"].as_str().unwrap());
}

#[test]
fn a_line_that_is_not_json_is_skipped() {
    let mut core = Core::start("junk", "junk");
    core.call("r1", "d1", &request("s", "ping"));
    let got = core.collect("r1", "d1");
    assert_eq!(got.text(), "pong\n");
    assert!(got.response["result"]["usage"].is_object());
}

#[test]
fn a_very_long_line_is_passed_on_in_pieces() {
    let mut core = Core::start("long", "long");
    core.call("r1", "d1", &request("s", "hi"));
    let got = core.collect("r1", "d1");
    assert_eq!(got.text(), "x".repeat(3_000_000));
    assert!(got.line_lengths.len() > 1);
    assert!(
        got.line_lengths.iter().all(|length| *length < 1024 * 1024),
        "交给核心的每一行都不到 1 MiB：{:?}",
        got.line_lengths
    );
    assert!(got.response["result"]["usage"].is_object());
}

#[test]
fn agy_that_never_reads_its_input_is_retryable() {
    let mut core = Core::start("noread", "noread");
    // 比管道的缓冲大得多：写的线程会碰上对面关了。
    let mut req = request("s", "now");
    let mut messages = Vec::new();
    for index in 0..200 {
        messages.push(json!({"role": "user", "blocks": [{"type": "text", "text": format!("q{index} {}", "x".repeat(1000))}]}));
        messages.push(json!({"role": "assistant", "blocks": [{"type": "text", "text": "ok"}]}));
    }
    messages.push(json!({"role": "user", "blocks": [{"type": "text", "text": "now"}]}));
    req["messages"] = Value::Array(messages);
    core.call("r1", "d1", &req);
    let error = core.collect("r1", "d1").response["result"]["error"].clone();
    assert_eq!(error["class"], "retryable", "{error}");
    // 扩展照样活着。
    core.send(&json!({"jsonrpc": "2.0", "id": "m1", "method": "driver.models", "params": {}}));
    assert!(core.recv()["result"]["models"].is_array());
}

#[test]
fn bad_params_are_refused() {
    let mut core = Core::start("params", "sample:real-pong.jsonl");
    core.send(&json!({"jsonrpc": "2.0", "id": "r1", "method": "driver.call", "params": {"call": "d1", "request": request("s", "hi"), "params": {}}}));
    let refused = core.recv();
    assert_eq!(refused["id"], "r1");
    assert_eq!(refused["error"]["code"], -32602);
    assert!(core.records().is_empty(), "没起 agy");
}

#[test]
fn a_refused_handshake_exits_with_1() {
    let mut core = Core::spawn("refused", "sample:real-pong.jsonl");
    core.answer_hello(
        &Value::Null,
        Some(&json!({"code": -32010, "message": "no", "data": {"reason": "protocol_mismatch"}})),
    );
    assert_eq!(core.wait_exit().code(), Some(1));
}

#[test]
fn unreadable_config_exits_with_1() {
    let mut core = Core::spawn("bad-config", "sample:real-pong.jsonl");
    core.answer_hello(
        &json!({"protocol": 1, "config": {"antigravity.home": 3}}),
        None,
    );
    assert_eq!(core.wait_exit().code(), Some(1));

    let mut later = Core::start("bad-config-later", "sample:real-pong.jsonl");
    later.send(&json!({"jsonrpc": "2.0", "method": "extension.config", "params": {"keys": {"antigravity.binary": ["x"]}}}));
    assert_eq!(later.wait_exit().code(), Some(1));
}
