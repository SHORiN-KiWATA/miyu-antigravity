//! 攒正文的开头、认内容策略拦截（设计第八节）。

use serde_json::{Value, json};

use super::*;
use crate::stream::parse;

/// 旧版 Miyu 的测试里录的 Google 拦截文案。
const BLOCK: &str = "The prompt could not be submitted. The prompt contains sensitive words that violate Google's [Generative AI Prohibited Use policy](https://policies.google.com/terms/generative-ai/use-policy).";

fn text(step: u64, done: bool, text: &str) -> Event {
    Event::Response {
        step_index: step,
        done,
        text: Some(text.to_string()),
        usage: None,
    }
}

fn result(json: &str) -> Event {
    parse(json).unwrap()
}

fn success() -> Event {
    result(r#"{"event":"result","result":{"status":"SUCCESS"}}"#)
}

fn feed_all(turn: &mut Turn, events: Vec<Event>) -> Vec<Value> {
    events
        .into_iter()
        .flat_map(|event| turn.feed(event).deltas)
        .collect()
}

fn opened(index: u64) -> Value {
    json!({"start": {"index": index, "kind": "text"}})
}

fn piece(index: u64, text: &str) -> Value {
    json!({"text": {"index": index, "text": text}})
}

#[test]
fn the_block_notice_is_not_delivered_and_the_turn_ends_with_content_policy() {
    let mut turn = Turn::new("a");
    let deltas = feed_all(
        &mut turn,
        vec![
            text(1, false, &BLOCK[..20]),
            text(1, false, &BLOCK[20..70]),
            text(1, false, &BLOCK[70..]),
            text(1, true, ""),
        ],
    );
    assert!(deltas.is_empty(), "一条增量都不交：{deltas:?}");
    let fed = turn.feed(success());
    assert!(fed.deltas.is_empty());
    let error = &fed.answer.unwrap()["error"];
    assert_eq!(error["class"], "content_policy");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("The prompt could not be submitted"),
        "{message}"
    );
}

#[test]
fn the_notice_in_one_piece_or_with_other_case_and_leading_blanks_is_recognised_too() {
    for body in [
        BLOCK,
        "\n  THE PROMPT COULD NOT BE SUBMITTED. Try rephrasing the prompt.",
    ] {
        let mut turn = Turn::new("a");
        assert!(feed_all(&mut turn, vec![text(1, true, body)]).is_empty());
        let answer = turn.feed(success()).answer.unwrap();
        assert_eq!(answer["error"]["class"], "content_policy", "{body:?}");
    }
}

#[test]
fn a_notice_ends_the_call_whatever_the_result_status_says() {
    let mut turn = Turn::new("a");
    feed_all(&mut turn, vec![text(1, true, BLOCK)]);
    let fed = turn.feed(result(
        r#"{"event":"result","result":{"status":"ERROR","error":"sticky old error"}}"#,
    ));
    assert_eq!(fed.answer.unwrap()["error"]["class"], "content_policy");
}

#[test]
fn a_notice_that_only_comes_in_the_result_response_counts_when_nothing_was_delivered() {
    let mut turn = Turn::new("a");
    let line =
        json!({"event": "result", "result": {"status": "SUCCESS", "response": BLOCK}}).to_string();
    assert_eq!(
        turn.feed(result(&line)).answer.unwrap()["error"]["class"],
        "content_policy"
    );
    // 已经交过字的不算。
    let mut turn = Turn::new("a");
    feed_all(&mut turn, vec![text(1, true, "fine")]);
    assert!(
        turn.feed(result(&line))
            .answer
            .unwrap()
            .get("error")
            .is_none()
    );
}

#[test]
fn a_broken_stream_after_the_notice_is_content_policy_too() {
    let mut turn = Turn::new("a");
    feed_all(&mut turn, vec![text(1, false, BLOCK)]);
    assert!(turn.flush().is_empty());
    assert_eq!(
        turn.broken("agy exited")["error"]["class"],
        "content_policy"
    );
}

#[test]
fn text_that_starts_like_the_notice_but_goes_elsewhere_is_delivered_piece_by_piece_unchanged() {
    let mut turn = Turn::new("a");
    // 前半一样：攒着，不交。
    assert!(
        turn.feed(text(1, false, "The prompt could not be "))
            .deltas
            .is_empty()
    );
    // 后面不一样：攒着的一条一条交出去，再交这一条。
    let deltas = turn.feed(text(1, false, "found in the cache.")).deltas;
    assert_eq!(
        deltas,
        vec![
            opened(0),
            piece(0, "The prompt could not be "),
            piece(0, "found in the cache.")
        ]
    );
    let rest = feed_all(&mut turn, vec![text(1, false, " More."), text(1, true, "")]);
    assert_eq!(rest, vec![piece(0, " More."), json!({"end": {"index": 0}})]);
    assert!(turn.feed(success()).answer.unwrap().get("error").is_none());
}

#[test]
fn a_blank_start_is_held_until_the_text_shows_what_it_is() {
    let mut turn = Turn::new("a");
    assert!(turn.feed(text(1, false, " ")).deltas.is_empty());
    assert_eq!(
        turn.feed(text(1, false, "pong")).deltas,
        vec![opened(0), piece(0, " "), piece(0, "pong")]
    );
}

#[test]
fn a_held_start_is_delivered_when_the_step_is_done() {
    let mut turn = Turn::new("a");
    turn.feed(text(1, false, "The prompt could "));
    assert_eq!(
        turn.feed(text(1, true, "")).deltas,
        vec![
            opened(0),
            piece(0, "The prompt could "),
            json!({"end": {"index": 0}})
        ]
    );
}

#[test]
fn a_held_start_is_delivered_when_a_result_or_another_step_comes() {
    // result（成功）：交出去、收块、回用量。
    let mut turn = Turn::new("a");
    turn.feed(text(1, false, "The prompt"));
    let fed = turn.feed(success());
    assert_eq!(
        fed.deltas,
        vec![
            opened(0),
            piece(0, "The prompt"),
            json!({"end": {"index": 0}})
        ]
    );
    assert!(fed.answer.unwrap().get("error").is_none());
    // result（失败）：交出去，照交过增量归类。
    let mut turn = Turn::new("a");
    turn.feed(text(1, false, "The prompt"));
    let fed = turn.feed(result(
        r#"{"event":"result","result":{"status":"ERROR","error":"boom"}}"#,
    ));
    assert_eq!(fed.deltas, vec![opened(0), piece(0, "The prompt")]);
    assert_eq!(fed.answer.unwrap()["error"]["class"], "unclassified");
    // 别的正文步。
    let mut turn = Turn::new("a");
    turn.feed(text(1, false, "The"));
    let deltas = turn.feed(text(3, false, "next")).deltas;
    assert_eq!(
        deltas,
        vec![
            opened(0),
            piece(0, "The"),
            json!({"end": {"index": 0}}),
            opened(1),
            piece(1, "next")
        ]
    );
}

#[test]
fn a_held_start_is_delivered_before_a_tool_block_and_before_the_hand_back() {
    let tool = |name: &str| {
        parse(
            &json!({"event": "step_update", "step_update": {
                "step_index": 2, "state": "ACTIVE", "step_type": "tool", "tool_name": name,
                "tool_info": {"parameters": {"a": 1}}
            }})
            .to_string(),
        )
        .unwrap()
    };
    let mut turn = Turn::new("a");
    turn.feed(text(1, false, "The prompt"));
    let deltas = turn.feed(tool("search_web")).deltas;
    assert_eq!(
        deltas[..5],
        [
            opened(0),
            piece(0, "The prompt"),
            json!({"end": {"index": 0}}),
            json!({"start": {"index": 1, "kind": {"hosted_tool": {"name": "search_web"}}}}),
            json!({"private": {"index": 1, "data": {"hosted": true}}}),
        ]
    );
    let mut turn = Turn::new("a");
    turn.feed(text(1, false, "The prompt"));
    turn.tool_call(3, "bash", &json!({}));
    let fed = turn.feed(tool("mcp_miyu_bash"));
    assert_eq!(
        fed.deltas[..3],
        [
            opened(0),
            piece(0, "The prompt"),
            json!({"end": {"index": 0}})
        ]
    );
    assert_eq!(fed.handed, Some(3));
}

#[test]
fn flush_hands_over_what_is_held_and_a_broken_stream_counts_it_as_delivered() {
    let mut turn = Turn::new("a");
    turn.feed(text(1, false, "The prompt could "));
    assert_eq!(turn.flush(), vec![opened(0), piece(0, "The prompt could ")]);
    assert!(turn.flush().is_empty(), "交一次就够");
    assert_eq!(turn.broken("agy exited")["error"]["class"], "unclassified");
    // 什么都没攒的：什么都不交，还没交过增量的是 retryable。
    let mut empty = Turn::new("a");
    assert!(empty.flush().is_empty());
    assert_eq!(empty.broken("agy exited")["error"]["class"], "retryable");
}

#[test]
fn only_the_first_text_of_a_turn_is_held() {
    let mut turn = Turn::new("a");
    let deltas = feed_all(
        &mut turn,
        vec![
            text(1, true, "fine."),
            text(3, false, "The prompt could "),
            text(3, true, ""),
        ],
    );
    // 第二段正文照老样子立刻交，不攒。
    assert_eq!(
        deltas,
        vec![
            opened(0),
            piece(0, "fine."),
            json!({"end": {"index": 0}}),
            opened(1),
            piece(1, "The prompt could "),
            json!({"end": {"index": 1}}),
        ]
    );
    let mut later = Turn::new("a");
    later.feed(text(1, true, "fine."));
    feed_all(&mut later, vec![text(3, true, BLOCK)]);
    assert!(later.feed(success()).answer.unwrap().get("error").is_none());
}

#[test]
fn text_that_clearly_is_not_the_notice_is_delivered_at_once() {
    let mut turn = Turn::new("a");
    assert_eq!(
        turn.feed(text(1, false, "pong")).deltas,
        vec![opened(0), piece(0, "pong")]
    );
}
