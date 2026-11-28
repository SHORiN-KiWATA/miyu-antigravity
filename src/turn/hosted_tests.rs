//! 供应商侧工具：agy 自己跑的工具步照实交成 `hosted_tool` 块（设计第七节）。

use std::io::Cursor;

use serde_json::{Value, json};

use super::*;
use crate::stream::parse;

const SEARCH: &str = include_str!("../../tests/samples/real-search-read.jsonl");
const TRANSCRIPT: &str = include_str!("../../tests/samples/real-search-read.transcript.jsonl");

/// 一条工具步：`info` 是 `tool_info` 里除了名字以外的字。
fn tool(step: u64, name: &str, state: &str, info: &Value) -> Event {
    let mut tool_info = json!({"name": name});
    if let (Some(into), Some(from)) = (tool_info.as_object_mut(), info.as_object()) {
        into.extend(from.clone());
    }
    parse(
        &json!({"event": "step_update", "step_update": {
            "conversation_id": "c", "step_index": step, "state": state, "step_type": "tool",
            "tool_name": name, "tool_info": tool_info
        }})
        .to_string(),
    )
    .unwrap()
}

fn params() -> Value {
    json!({"parameters": {"query": "rust"}})
}

fn text(step: u64, done: bool, text: &str) -> Event {
    Event::Response {
        step_index: step,
        done,
        text: Some(text.to_string()),
        usage: None,
    }
}

fn start(index: u64, name: &str) -> Value {
    json!({"start": {"index": index, "kind": {"hosted_tool": {"name": name}}}})
}

/// 开块以后紧跟的私有数据（施工 AG-16）。
fn mark(index: u64) -> Value {
    json!({"private": {"index": index, "data": {"hosted": true}}})
}

fn end(index: u64) -> Value {
    json!({"end": {"index": index}})
}

fn feed_all(turn: &mut Turn, events: Vec<Event>) -> Vec<Value> {
    events
        .into_iter()
        .flat_map(|event| turn.feed(event).deltas)
        .collect()
}

#[test]
fn the_real_recording_gives_a_block_per_tool_with_the_transcript_result_and_the_right_order() {
    let mut turn = Turn::new("probe-a");
    let mut deltas = Vec::new();
    for line in SEARCH.lines() {
        let mut event = parse(line).unwrap();
        // 像 talk.rs 那样：DONE、没带 output 的，从转录读结果填进去再喂。
        if let Event::Tool(step) = &mut event
            && step.state == ToolState::Done
            && step.output.is_none()
        {
            step.output =
                crate::brain::result_in(Cursor::new(TRANSCRIPT), step.step_index).unwrap();
        }
        deltas.extend(turn.feed(event).deltas);
    }
    let result_of = |step| {
        crate::brain::result_in(Cursor::new(TRANSCRIPT), step)
            .unwrap()
            .unwrap()
    };
    let search = result_of(2);
    let page = result_of(4);
    assert!(search.starts_with("The search for"), "{search}");
    assert!(page.starts_with("Title: EditorConfig"), "{page}");
    let mut expected = vec![
        start(0, "search_web"),
        mark(0),
        json!({"text": {"index": 0, "text": r#"{"query":"EditorConfig specification"}"#}}),
        json!({"outcome": {"index": 0, "text": search}}),
        end(0),
        // 样本里还有 read_url_content：照同一条规矩也是一块。
        start(1, "read_url_content"),
        mark(1),
        json!({"text": {"index": 1, "text": r#"{"Url":"https://spec.editorconfig.org/"}"#}}),
        json!({"outcome": {"index": 1, "text": page}}),
        end(1),
        json!({"start": {"index": 2, "kind": "text"}}),
    ];
    for piece in [
        "NO-",
        "TOOL; the page title is \"EditorConfi",
        "g Specificat",
        "ion — EditorConfig Specification 0.17.2 docume",
        "ntation\".",
        "\n",
    ] {
        expected.push(json!({"text": {"index": 2, "text": piece}}));
    }
    expected.push(end(2));
    assert_eq!(deltas, expected);
}

#[test]
fn a_step_opens_once_however_many_active_lines_come_and_done_gives_the_result_then_ends() {
    let mut turn = Turn::new("a");
    let opened = turn.feed(tool(2, "search_web", "ACTIVE", &params())).deltas;
    assert_eq!(
        opened,
        vec![
            start(0, "search_web"),
            mark(0),
            json!({"text": {"index": 0, "text": r#"{"query":"rust"}"#}})
        ]
    );
    assert_eq!(
        turn.feed(tool(2, "search_web", "ACTIVE", &params())).deltas,
        Vec::<Value>::new(),
        "再来一条 ACTIVE 不重开"
    );
    let mut info = params();
    info["output"] = json!("1. Rust https://www.rust-lang.org");
    let done = turn.feed(tool(2, "search_web", "DONE", &info)).deltas;
    assert_eq!(
        done,
        vec![
            json!({"outcome": {"index": 0, "text": "1. Rust https://www.rust-lang.org"}}),
            end(0)
        ]
    );
    // 收了以后同一步再来的条不再开块。
    assert!(
        turn.feed(tool(2, "search_web", "DONE", &info))
            .deltas
            .is_empty()
    );
    // 下一步另开一块，编号接着数。
    assert_eq!(
        turn.feed(tool(3, "search_web", "ACTIVE", &params())).deltas[0],
        start(1, "search_web")
    );
}

#[test]
fn a_done_without_a_result_ends_the_block_without_an_outcome() {
    let mut turn = Turn::new("a");
    turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    assert_eq!(
        turn.feed(tool(2, "search_web", "DONE", &params())).deltas,
        vec![end(0)]
    );
    // 空字符串的结果也当没有。
    let mut empty = Turn::new("a");
    empty.feed(tool(2, "search_web", "ACTIVE", &params()));
    let mut info = params();
    info["output"] = json!("");
    assert_eq!(
        empty.feed(tool(2, "search_web", "DONE", &info)).deltas,
        vec![end(0)]
    );
}

#[test]
fn a_step_that_comes_as_done_or_error_alone_counts_as_the_first_sighting() {
    let mut done = Turn::new("a");
    let mut info = params();
    info["output"] = json!("found");
    assert_eq!(
        done.feed(tool(5, "search_web", "DONE", &info)).deltas,
        vec![
            start(0, "search_web"),
            mark(0),
            json!({"text": {"index": 0, "text": r#"{"query":"rust"}"#}}),
            json!({"outcome": {"index": 0, "text": "found"}}),
            end(0),
        ]
    );
    let mut error = Turn::new("a");
    let mut info = params();
    info["error"] = json!({"type": "tool_error", "message": "boom"});
    assert_eq!(
        error.feed(tool(5, "search_web", "ERROR", &info)).deltas,
        vec![
            start(0, "search_web"),
            mark(0),
            json!({"text": {"index": 0, "text": r#"{"query":"rust"}"#}}),
            json!({"outcome": {"index": 0, "text": "boom", "failed": true}}),
            end(0),
        ]
    );
}

#[test]
fn an_error_step_is_failed_with_the_streams_words_or_failed_when_there_are_none() {
    let mut turn = Turn::new("a");
    turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    let mut info = params();
    info["error"] = json!("quota for search used up");
    assert_eq!(
        turn.feed(tool(2, "search_web", "ERROR", &info)).deltas,
        vec![
            json!({"outcome": {"index": 0, "text": "quota for search used up", "failed": true}}),
            end(0)
        ]
    );
    let mut bare = Turn::new("a");
    bare.feed(tool(2, "search_web", "ACTIVE", &params()));
    assert_eq!(
        bare.feed(tool(2, "search_web", "ERROR", &params())).deltas,
        vec![
            json!({"outcome": {"index": 0, "text": "failed", "failed": true}}),
            end(0)
        ]
    );
}

#[test]
fn a_step_without_parameters_gives_no_text_and_the_name_is_kept_as_it_is() {
    let mut turn = Turn::new("a");
    let fed = parse(
        r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"mcp_other_Thing"}}"#,
    )
    .unwrap();
    assert_eq!(
        turn.feed(fed).deltas,
        vec![start(0, "mcp_other_Thing"), mark(0)]
    );
}

#[test]
fn a_long_result_is_cut_to_64_kib_on_a_char_boundary() {
    // 三字节的字 21846 个共 65538 字节：退到 65535。
    let long = "字".repeat(21846);
    let mut turn = Turn::new("a");
    turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    let mut info = params();
    info["output"] = json!(long);
    let deltas = turn.feed(tool(2, "search_web", "DONE", &info)).deltas;
    let sent = deltas[0]["outcome"]["text"].as_str().unwrap();
    assert_eq!(sent.len(), 65535);
    assert_eq!(sent, &long[..65535]);
    assert_eq!(cut("abc"), "abc");
    assert_eq!(cut(&"x".repeat(OUTCOME_LIMIT + 5)).len(), OUTCOME_LIMIT);
}

#[test]
fn an_open_text_block_is_closed_before_the_tool_block_starts() {
    let mut turn = Turn::new("a");
    let deltas = feed_all(
        &mut turn,
        vec![
            text(1, false, "Let me search. "),
            tool(2, "search_web", "ACTIVE", &params()),
        ],
    );
    assert_eq!(deltas[0], json!({"start": {"index": 0, "kind": "text"}}));
    assert_eq!(deltas[2], end(0));
    assert_eq!(deltas[3], start(1, "search_web"));
}

#[test]
fn an_unfinished_tool_block_is_closed_without_a_result_by_whatever_comes_next() {
    // 正文。
    let mut turn = Turn::new("a");
    turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    let deltas = turn.feed(text(3, false, "hi")).deltas;
    assert_eq!(deltas[0], end(0));
    assert_eq!(deltas[1], json!({"start": {"index": 1, "kind": "text"}}));
    // 下一件工具。
    let mut turn = Turn::new("a");
    turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    let deltas = turn
        .feed(tool(3, "other_tool", "ACTIVE", &json!({})))
        .deltas;
    assert_eq!(deltas, vec![end(0), start(1, "other_tool"), mark(1)]);
    // result。
    let mut turn = Turn::new("a");
    turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    let fed = turn.feed(parse(r#"{"event":"result","result":{"status":"SUCCESS"}}"#).unwrap());
    assert_eq!(fed.deltas, vec![end(0)]);
    assert!(fed.answer.is_some());
    // 失败的 result 也收。
    let mut turn = Turn::new("a");
    turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    let fed =
        turn.feed(parse(r#"{"event":"result","result":{"status":"ERROR","error":"x"}}"#).unwrap());
    assert_eq!(fed.deltas, vec![end(0)]);
}

#[test]
fn a_miyu_tool_step_closes_the_open_tool_block_before_the_hand_back() {
    let mut turn = Turn::new("a");
    turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    turn.tool_call(7, "bash", &json!({"command": "ls"}));
    let fed = turn.feed(tool(3, "mcp_miyu_bash", "ACTIVE", &json!({})));
    assert_eq!(
        fed.deltas,
        vec![
            end(0),
            json!({"start": {"index": 1, "kind": {"tool_call": {"name": "bash"}}}}),
            json!({"text": {"index": 1, "text": r#"{"command":"ls"}"#}}),
            json!({"private": {"index": 1, "data": {"mcp": 7}}}),
            json!({"end": {"index": 1}}),
        ]
    );
    assert_eq!(fed.handed, Some(7));
}

#[test]
fn hosted_blocks_are_not_the_miyu_tool_side_and_do_not_end_the_call() {
    let mut turn = Turn::new("a");
    let fed = turn.feed(tool(2, "search_web", "ACTIVE", &params()));
    assert_eq!(fed.answer, None);
    assert_eq!(fed.handed, None);
    assert_eq!(turn.waiting(), None);
    // 交过块了：流断了算交过增量。
    assert_eq!(turn.broken("x")["error"]["class"], "unclassified");
}

#[test]
fn steps_that_are_not_ordinary_states_or_have_no_name_are_ignored() {
    let mut turn = Turn::new("a");
    assert!(
        turn.feed(tool(2, "search_web", "PENDING", &params()))
            .deltas
            .is_empty()
    );
    assert!(
        turn.feed(tool(3, "", "ACTIVE", &params()))
            .deltas
            .is_empty()
    );
}
