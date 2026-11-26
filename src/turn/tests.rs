use serde_json::{Value, json};

use super::*;
use crate::stream::parse;

const PONG: &str = include_str!("../../tests/samples/real-pong.jsonl");
const SEARCH: &str = include_str!("../../tests/samples/real-search-read.jsonl");

/// 把样本喂给一轮（代理名换成 `probe-a`，样本录的就是它），交全部增量和回答。
fn run(sample: &str) -> (Vec<Value>, Option<Value>, Turn) {
    let mut turn = Turn::new("probe-a");
    let mut deltas = Vec::new();
    let mut answer = None;
    for line in sample.lines() {
        assert!(answer.is_none(), "回答以后不再读");
        let fed = turn.feed(parse(line).unwrap());
        deltas.extend(fed.deltas);
        answer = fed.answer;
    }
    (deltas, answer, turn)
}

#[test]
fn pong_is_one_text_block_and_its_usage() {
    let (deltas, answer, turn) = run(PONG);
    assert_eq!(
        deltas,
        vec![
            json!({"start": {"index": 0, "kind": "text"}}),
            json!({"text": {"index": 0, "text": "pong"}}),
            json!({"text": {"index": 0, "text": "\n"}}),
            json!({"end": {"index": 0}}),
        ]
    );
    assert_eq!(
        answer,
        Some(
            json!({"usage": {"uncached": 2679, "cache_read": 0, "cache_write": 0, "output": 1, "reasoning": 0, "context": 2680}})
        )
    );
    assert_eq!(
        turn.conversation(),
        Some("ff223be4-3376-4486-b661-7414044371c2")
    );
    assert_eq!(
        turn.broken("x")["error"]["class"],
        "unclassified",
        "交过增量了"
    );
}

#[test]
fn search_read_reports_the_tools_as_hosted_blocks_and_skips_empty_steps_and_reports_the_last_context()
 {
    let (deltas, answer, _) = run(SEARCH);
    // 两件工具各是一块供应商侧工具（没有结果：这里没人去读转录），正文在它们后面，块一块接一块。
    let mut expected = vec![
        json!({"start": {"index": 0, "kind": {"hosted_tool": {"name": "search_web"}}}}),
        json!({"text": {"index": 0, "text": r#"{"query":"EditorConfig specification"}"#}}),
        json!({"end": {"index": 0}}),
        json!({"start": {"index": 1, "kind": {"hosted_tool": {"name": "read_url_content"}}}}),
        json!({"text": {"index": 1, "text": r#"{"Url":"https://spec.editorconfig.org/"}"#}}),
        json!({"end": {"index": 1}}),
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
    expected.push(json!({"end": {"index": 2}}));
    assert_eq!(deltas, expected, "步 1、3 没有字，不开块");
    // 记账照三个 DONE 加起来；上下文大小另报最后一次请求的（施工 AG-13，核心 ACP-1）：agy 跑自带工具时一轮里来回请求
    // 几次，每次都带整段上下文，核心拿加起来的当上下文大小就虚高（2026-11-23 主人的群会话因此停在 too_large）。
    assert_eq!(
        answer,
        Some(json!({"usage": {
            "uncached": 2741 + 2482 + 2330,
            "cache_read": 2758 + 3208,
            "cache_write": 0,
            "output": 38 + 47 + 25,
            "reasoning": 0,
            "context": 2330 + 3208 + 25
        }}))
    );
}

fn response(step: u64, done: bool, text: Option<&str>) -> Event {
    Event::Response {
        step_index: step,
        done,
        text: text.map(str::to_string),
        usage: None,
    }
}

#[test]
fn a_new_response_step_closes_the_open_block_and_opens_another() {
    let mut turn = Turn::new("a");
    let mut deltas = Vec::new();
    for event in [
        response(1, false, Some("one")),
        response(3, false, Some("two")),
        response(3, true, None),
    ] {
        deltas.extend(turn.feed(event).deltas);
    }
    let result = turn.feed(parse(r#"{"event":"result","result":{"status":"SUCCESS"}}"#).unwrap());
    assert_eq!(
        deltas,
        vec![
            json!({"start": {"index": 0, "kind": "text"}}),
            json!({"text": {"index": 0, "text": "one"}}),
            json!({"end": {"index": 0}}),
            json!({"start": {"index": 1, "kind": "text"}}),
            json!({"text": {"index": 1, "text": "two"}}),
            json!({"end": {"index": 1}}),
        ]
    );
    assert_eq!(result.answer, Some(json!({})), "没有用量的不报 usage");
}

#[test]
fn an_open_block_is_closed_before_a_successful_answer() {
    let mut turn = Turn::new("a");
    turn.feed(response(1, false, Some("half")));
    let fed = turn.feed(parse(r#"{"event":"result","result":{"status":"SUCCESS"}}"#).unwrap());
    assert_eq!(fed.deltas, vec![json!({"end": {"index": 0}})]);
}

#[test]
fn long_text_is_cut_into_pieces_on_char_boundaries() {
    let mut turn = Turn::new("a");
    let text = "字".repeat(TEXT_PIECE); // 三倍于上限的字节
    let fed = turn.feed(response(1, false, Some(&text)));
    let pieces: Vec<&str> = fed.deltas[1..]
        .iter()
        .map(|delta| delta["text"]["text"].as_str().unwrap())
        .collect();
    assert!(pieces.len() >= 3);
    assert!(pieces.iter().all(|piece| piece.len() <= TEXT_PIECE));
    assert_eq!(pieces.concat(), text);
}

#[test]
fn a_wrong_agent_ends_the_call() {
    let mut turn = Turn::new("miyu-0011223344556677");
    let fed = turn.feed(
        parse(r#"{"event":"init","conversation_id":"c-1","init":{"agent":"probe-a"}}"#).unwrap(),
    );
    let answer = fed.answer.unwrap();
    assert_eq!(answer["error"]["class"], "unclassified");
    let message = answer["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("\"probe-a\"") && message.contains("miyu-0011223344556677"),
        "{message}"
    );
    assert_eq!(
        turn.conversation(),
        Some("c-1"),
        "照样记下编号，好删它的文件"
    );
    let none = Turn::new("x").feed(parse(r#"{"event":"init","init":{}}"#).unwrap());
    assert!(
        none.answer.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no agent")
    );
}

#[test]
fn failed_results_are_classified() {
    // 手写，照设计第八节和旧版 stream.rs：result 出错的形状。
    let fail = |error: &str, before: &[Event]| {
        let mut turn = Turn::new("a");
        for event in before {
            turn.feed(event.clone());
        }
        let line =
            json!({"event": "result", "result": {"status": "ERROR", "error": error}}).to_string();
        turn.feed(parse(&line).unwrap()).answer.unwrap()["error"].clone()
    };
    assert_eq!(
        fail("You are not logged into Antigravity", &[])["class"],
        "auth"
    );
    assert_eq!(fail("RESOURCE_EXHAUSTED", &[])["class"], "quota");
    assert_eq!(fail("rate limit hit", &[])["class"], "rate_limited");
    assert_eq!(fail("boom", &[])["class"], "retryable");
    let after_text = fail("boom", &[response(1, false, Some("hi"))]);
    assert_eq!(after_text["class"], "unclassified");
    assert_eq!(after_text["message"], "agy failed (ERROR): boom");
    // error_message 步的字也算进归类和原话。
    let said = fail(
        "",
        &[Event::ErrorMessage {
            text: "Quota exceeded".into(),
        }],
    );
    assert_eq!(said["class"], "quota");
    assert_eq!(said["message"], "agy failed (ERROR): Quota exceeded");
}

#[test]
fn a_broken_stream_is_retryable_until_text_went_out() {
    let mut turn = Turn::new("a");
    let early = turn.broken("agy exited without a result");
    assert_eq!(early["error"]["class"], "retryable");
    assert_eq!(early["error"]["message"], "agy exited without a result");
    turn.feed(response(1, false, Some("hi")));
    assert_eq!(turn.broken("x")["error"]["class"], "unclassified");
    let mut quota = Turn::new("a");
    quota.feed(Event::ErrorMessage {
        text: "You are not logged in".into(),
    });
    let answer = quota.broken("agy exited without a result");
    assert_eq!(answer["error"]["class"], "auth");
    assert_eq!(
        answer["error"]["message"],
        "agy exited without a result: You are not logged in"
    );
}

#[test]
fn pieces_keep_short_text_whole() {
    assert_eq!(pieces("abc", 10), ["abc"]);
    assert_eq!(pieces("abcdef", 3), ["abc", "def"]);
    assert_eq!(pieces("a字b", 2), ["a", "字", "b"]);
}
