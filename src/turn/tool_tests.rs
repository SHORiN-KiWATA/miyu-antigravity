//! 工具调用：流里的工具步和 MCP 转来的 `tools/call` 两边谁先到都行，两边到齐才交一块工具调用并结束这一次。

use serde_json::{Value, json};

use super::*;
use crate::stream::parse;

fn tool_step(name: &str, state: &str) -> Event {
    parse(
        &json!({"event": "step_update", "step_update": {
            "conversation_id": "c", "step_index": 3, "state": state, "step_type": "tool",
            "tool_name": name, "tool_info": {"parameters": {"command": "ls"}}
        }})
        .to_string(),
    )
    .unwrap()
}

fn text(step: u64, done: bool, text: &str) -> Event {
    Event::Response {
        step_index: step,
        done,
        text: Some(text.to_string()),
        usage: done.then_some(Usage {
            input: 10,
            output: 3,
            thinking: 1,
            cache_read: 7,
        }),
    }
}

fn usage_answer(times: u64) -> Value {
    json!({"usage": {
        "uncached": 10 * times, "cache_read": 7 * times, "cache_write": 0,
        "output": 3 * times, "reasoning": times
    }})
}

fn handed_deltas(index: u64, name: &str, arguments: &str, n: u64) -> Vec<Value> {
    vec![
        json!({"start": {"index": index, "kind": {"tool_call": {"name": name}}}}),
        json!({"text": {"index": index, "text": arguments}}),
        json!({"private": {"index": index, "data": {"mcp": n}}}),
        json!({"end": {"index": index}}),
    ]
}

#[test]
fn the_stream_side_first_then_the_mcp_side_hands_the_call_back() {
    let mut turn = Turn::new("a");
    assert_eq!(turn.feed(text(1, false, "let me look. ")).deltas.len(), 2);
    turn.feed(text(1, true, ""));
    assert_eq!(turn.waiting(), None);

    let fed = turn.feed(tool_step("mcp_miyu_bash", "ACTIVE"));
    assert_eq!(fed, Fed::default(), "第一边到了：还不交");
    assert_eq!(turn.waiting(), Some(Waiting::ForMcp));

    let fed = turn.tool_call(1, "bash", &json!({"command": "ls -l", "n": 2}));
    assert_eq!(
        fed.deltas,
        handed_deltas(1, "bash", r#"{"command":"ls -l","n":2}"#, 1),
        "正文块 0 已经收了，工具调用是块 1，名字是 MCP 收到的原样"
    );
    assert_eq!(fed.answer, Some(usage_answer(1)));
    assert_eq!(fed.handed, Some(1));
    assert_eq!(turn.waiting(), None);
}

#[test]
fn the_mcp_side_first_waits_for_the_stream_and_text_in_between_is_still_read() {
    let mut turn = Turn::new("a");
    let fed = turn.tool_call(4, "read", &json!({}));
    assert_eq!(fed, Fed::default());
    assert_eq!(turn.waiting(), Some(Waiting::ForStream));

    // 等的时候流里来了正文：照常交。
    let fed = turn.feed(text(1, false, "before "));
    assert_eq!(
        fed.deltas,
        vec![
            json!({"start": {"index": 0, "kind": "text"}}),
            json!({"text": {"index": 0, "text": "before "}}),
        ]
    );
    assert_eq!(fed.answer, None);
    assert_eq!(turn.waiting(), Some(Waiting::ForStream), "掐表不重来");

    // 流这边到了：先收掉开着的正文块，再交工具调用。
    let fed = turn.feed(tool_step("mcp_miyu_read", "ACTIVE"));
    let mut expected = vec![json!({"end": {"index": 0}})];
    expected.extend(handed_deltas(1, "read", "{}", 4));
    assert_eq!(fed.deltas, expected);
    assert_eq!(fed.handed, Some(4));
    assert_eq!(fed.answer, Some(json!({})), "没有用量的不报 usage");
}

#[test]
fn only_active_mcp_steps_count_as_the_stream_side() {
    let mut turn = Turn::new("a");
    for step in [
        tool_step("mcp_miyu_bash", "DONE"),
        tool_step("mcp_miyu_bash", "ERROR"),
    ] {
        assert_eq!(turn.feed(step), Fed::default());
        assert_eq!(turn.waiting(), None);
    }
    // 别的工具（agy 自己跑的、别的 MCP 服务的）是供应商侧工具，不算流这边（见 `hosted_tests`）。
    for name in ["search_web", "mcp_other_bash"] {
        let mut other = Turn::new("a");
        other.feed(tool_step(name, "ACTIVE"));
        assert_eq!(other.waiting(), None, "{name}");
    }
    // MCP 那边到了，流这边还没有：在等流。
    turn.tool_call(1, "bash", &json!({}));
    assert_eq!(turn.waiting(), Some(Waiting::ForStream));
}

#[test]
fn the_wait_ending_hands_the_mcp_call_back_when_only_the_mcp_side_came() {
    let mut turn = Turn::new("a");
    turn.feed(text(1, true, "x"));
    turn.tool_call(2, "bash", &json!({"a": 1}));
    let fed = turn.timeout();
    assert_eq!(fed.handed, Some(2));
    assert_eq!(fed.deltas, handed_deltas(1, "bash", r#"{"a":1}"#, 2));
    assert_eq!(fed.answer, Some(usage_answer(1)));
    assert_eq!(turn.waiting(), None);
}

#[test]
fn the_wait_ending_with_only_the_stream_side_is_an_error_naming_the_config() {
    let mut turn = Turn::new("a");
    turn.feed(tool_step("mcp_miyu_bash", "ACTIVE"));
    let fed = turn.timeout();
    assert!(fed.deltas.is_empty());
    assert_eq!(fed.handed, None);
    let error = &fed.answer.unwrap()["error"];
    assert_eq!(error["class"], "unclassified");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("mcp_config.json"),
        "{error}"
    );
}

#[test]
fn the_wait_ending_with_nothing_to_wait_for_does_nothing() {
    let mut turn = Turn::new("a");
    assert_eq!(turn.timeout(), Fed::default());
}

#[test]
fn a_result_while_waiting_ends_the_call_as_usual() {
    for mcp_first in [false, true] {
        let mut turn = Turn::new("a");
        turn.feed(text(1, false, "say"));
        if mcp_first {
            turn.tool_call(1, "bash", &json!({}));
        } else {
            turn.feed(tool_step("mcp_miyu_bash", "ACTIVE"));
        }
        let fed = turn.feed(text(1, true, ""));
        assert_eq!(fed.answer, None);
        let fed = turn.feed(parse(r#"{"event":"result","result":{"status":"SUCCESS"}}"#).unwrap());
        assert_eq!(fed.deltas, Vec::<Value>::new(), "正文块 DONE 时已经收了");
        assert_eq!(fed.answer, Some(usage_answer(1)));
        assert_eq!(fed.handed, None);
    }
}

#[test]
fn a_failed_result_while_waiting_is_classified_as_usual() {
    let mut turn = Turn::new("a");
    turn.feed(tool_step("mcp_miyu_bash", "ACTIVE"));
    let fed = turn.feed(
        parse(r#"{"event":"result","result":{"status":"ERROR","error":"quota exceeded"}}"#)
            .unwrap(),
    );
    assert_eq!(fed.answer.unwrap()["error"]["class"], "quota");
    assert_eq!(fed.handed, None);
}

#[test]
fn usage_before_the_hand_back_takes_the_last_input_and_sums_the_output() {
    // 施工 AG-12：输入、缓存命中照最后一次请求（上下文大小），输出、思考两步加起来。
    let mut turn = Turn::new("a");
    turn.feed(text(1, true, "one"));
    turn.feed(text(2, true, "two"));
    turn.feed(tool_step("mcp_miyu_bash", "ACTIVE"));
    let fed = turn.tool_call(1, "bash", &json!({}));
    assert_eq!(
        fed.answer,
        Some(json!({"usage": {
            "uncached": 10, "cache_read": 7, "cache_write": 0, "output": 6, "reasoning": 2
        }}))
    );
    assert_eq!(fed.deltas.first().unwrap()["start"]["index"], 2);
}

#[test]
fn long_arguments_are_cut_into_pieces_that_join_back_to_the_original() {
    let big = "é".repeat(TEXT_PIECE);
    let arguments = json!({"text": big});
    let original = arguments.to_string();
    let mut turn = Turn::new("a");
    turn.feed(tool_step("mcp_miyu_write", "ACTIVE"));
    let fed = turn.tool_call(1, "write", &arguments);
    let pieces: Vec<&str> = fed
        .deltas
        .iter()
        .filter_map(|delta| delta.pointer("/text/text").and_then(Value::as_str))
        .collect();
    assert!(pieces.len() >= 2);
    assert!(pieces.iter().all(|piece| piece.len() <= TEXT_PIECE));
    assert_eq!(pieces.concat(), original);
}

#[test]
fn arguments_keep_the_order_the_tool_server_received() {
    let arguments: Value =
        serde_json::from_str(r#"{"b":1,"a":[1,null,"x"],"c":{"z":0,"y":1}}"#).unwrap();
    let mut turn = Turn::new("a");
    turn.feed(tool_step("mcp_miyu_bash", "ACTIVE"));
    let fed = turn.tool_call(1, "bash", &arguments);
    assert_eq!(
        fed.deltas[1]["text"]["text"],
        r#"{"b":1,"a":[1,null,"x"],"c":{"z":0,"y":1}}"#
    );
}
