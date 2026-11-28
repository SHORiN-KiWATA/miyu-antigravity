use serde_json::{Value, json};

use super::*;

fn user(text: &str) -> Value {
    json!({"role": "user", "blocks": [{"type": "text", "text": text}]})
}

fn assistant(text: &str) -> Value {
    json!({"role": "assistant", "blocks": [{"type": "text", "text": text}]})
}

fn request(messages: &[Value]) -> Value {
    json!({"tools": [], "system": "s", "messages": messages, "stable": 0})
}

#[test]
fn a_single_user_message_goes_as_its_own_text() {
    let got = transcribe(&request(&[user("hello there")]), BUDGET).unwrap();
    assert_eq!(got, "hello there");
}

#[test]
fn history_is_wrapped_and_the_last_turn_follows_it() {
    let got = transcribe(
        &request(&[
            user("q1"),
            assistant("a1"),
            user("q2"),
            user("runtime note"),
        ]),
        BUDGET,
    )
    .unwrap();
    assert_eq!(
        got,
        format!(
            "{}User:\nq1\n\nAssistant:\na1\n\n</conversation-history>\n\nq2\n\nruntime note",
            words::HISTORY_HEAD
        )
    );
}

/// 历史里不写工具（施工 AG-14）：她调的工具、工具的结果、供应商侧工具都不进转写；只调了工具、没说话的回复整条不写。以前照写，
/// 模型照这个样子编了一段字当回复（`[called tool …]`、`[tool result]`）。
#[test]
fn tool_calls_results_and_reasoning() {
    let messages = vec![
        user("list files"),
        json!({"role": "assistant", "blocks": [
            {"type": "reasoning", "text": "secret thoughts"},
            {"type": "text", "text": "Let me look."},
            {"type": "tool_call", "call_id": "c_1", "name": "ls", "args": "{\"path\":\".\"}"}
        ]}),
        json!({"role": "tool", "call_id": "c_1", "error": false, "blocks": [{"type": "text", "text": "a.txt"}]}),
        json!({"role": "tool", "call_id": "c_2", "error": true, "blocks": [{"type": "text", "text": "denied"}]}),
        json!({"role": "assistant", "blocks": [
            {"type": "hosted_tool", "call_id": "c_3", "name": "search_web", "args": "{\"query\":\"rust\"}",
             "outcome": {"text": "1. Rust https://www.rust-lang.org"}},
            {"type": "hosted_tool", "call_id": "c_4", "name": "search_web", "args": "{}",
             "outcome": {"text": "boom", "failed": true}},
            {"type": "hosted_tool", "call_id": "c_5", "name": "search_web", "args": "{}"}
        ]}),
        assistant("Found them."),
        user("thanks"),
    ];
    let got = transcribe(&request(&messages), BUDGET).unwrap();
    assert!(!got.contains("secret thoughts"), "思考不写");
    let body = got
        .strip_prefix(words::HISTORY_HEAD)
        .unwrap()
        .strip_suffix("</conversation-history>\n\nthanks")
        .unwrap();
    assert_eq!(
        body,
        "User:\nlist files\n\nAssistant:\nLet me look.\n\nAssistant:\nFound them.\n\n"
    );
    for marker in words::REPLAY_MARKERS
        .iter()
        .filter(|marker| marker.starts_with('['))
    {
        assert!(!got.contains(marker), "不写 {marker}");
    }
}

#[test]
fn images_files_and_paths_become_placeholders() {
    let blob = format!("sha256:{}", "a".repeat(64));
    let other = format!("sha256:{}", "b".repeat(64));
    let mut req = request(&[json!({"role": "user", "blocks": [
        {"type": "text", "text": "look"},
        {"type": "image", "blob": blob, "name": "sunset.png", "media_type": "image/png", "width": 1, "height": 1},
        {"type": "image", "blob": other, "media_type": "image/png", "width": 1, "height": 1},
        {"type": "file", "blob": other, "name": "report.pdf", "media_type": "application/pdf"},
        {"type": "path", "name": "disk.iso", "path": "/home/a/disk.iso", "size": 9},
        {"type": "future_kind", "x": 1}
    ]})]);
    req["described"] = json!({ blob.clone(): "A red sky over the sea." });
    let got = transcribe(&req, BUDGET).unwrap();
    assert_eq!(
        got,
        "look\n\
         [image \"sunset.png\", described: A red sky over the sea.]\n\
         [image omitted: this model reads text only]\n\
         [file \"report.pdf\" omitted: this model reads text only]\n\
         [file \"disk.iso\" attached by path /home/a/disk.iso; its content is not included]"
    );
}

#[test]
fn tail_goes_last() {
    let mut req = request(&[user("hi")]);
    req["tail"] = json!("<reminder/>");
    assert_eq!(transcribe(&req, BUDGET).unwrap(), "hi\n\n<reminder/>");
    let mut req = request(&[user("q"), assistant("a"), user("q2")]);
    req["tail"] = json!("<reminder/>");
    assert!(
        transcribe(&req, BUDGET)
            .unwrap()
            .ends_with("</conversation-history>\n\nq2\n\n<reminder/>")
    );
}

#[test]
fn a_trailing_tool_result_asks_to_continue() {
    let messages = vec![
        user("q"),
        json!({"role": "assistant", "blocks": [{"type": "tool_call", "call_id": "c_1", "name": "ls", "args": "{}"}]}),
        json!({"role": "tool", "call_id": "c_1", "error": false, "blocks": [{"type": "text", "text": "x"}]}),
    ];
    let got = transcribe(&request(&messages), BUDGET).unwrap();
    // 这一轮做到一半（以工具的结果结束）：做过的写在这一轮的旁注里，不写成对话（施工 AG-14）。
    assert_eq!(
        got,
        format!(
            "{}User:\nq\n\n</conversation-history>\n\n<this-turn>\n\
             What you already did in this turn, after the user's last message:\n\
             You called the tool ls with {{}}.\nThe tool returned: x\n</this-turn>\n\n{}",
            words::HISTORY_HEAD,
            words::CONTINUE
        )
    );
}

#[test]
fn budget_drops_the_oldest_turns_and_never_the_last_one() {
    let mut messages = Vec::new();
    for index in 0..40 {
        messages.push(user(&format!("q{index} {}", "问".repeat(300))));
        messages.push(assistant(&format!("a{index} {}", "答".repeat(300))));
    }
    messages.push(user("the real question"));
    let req = request(&messages);
    let whole = transcribe(&req, BUDGET).unwrap();
    let budget = 20_000;
    assert!(whole.len() > budget, "夹具要先超预算：{}", whole.len());
    assert!(!whole.contains(words::OMITTED));

    let got = transcribe(&req, budget).unwrap();
    assert!(got.len() <= budget, "{} > {budget}", got.len());
    assert!(got.ends_with("</conversation-history>\n\nthe real question"));
    let body = got
        .strip_prefix(words::HISTORY_HEAD)
        .unwrap()
        .strip_prefix(words::OMITTED)
        .expect("写了省略");
    assert!(body.starts_with("User:\nq"), "从一轮的开头起：{body:.40}");
    assert!(!got.contains("q0 "), "最老的丢了");
    assert!(got.contains("q39 ") && got.contains("a39 "), "最新的在");

    // 最后一轮自己就超了：照发，历史全丢、只留省略。
    let tight = transcribe(&req, 10).unwrap();
    assert_eq!(
        tight,
        format!(
            "{}{}</conversation-history>\n\nthe real question",
            words::HISTORY_HEAD,
            words::OMITTED
        )
    );
}

#[test]
fn budget_exactly_fits_keeps_everything() {
    let req = request(&[user("q"), assistant("a"), user("q2")]);
    let whole = transcribe(&req, BUDGET).unwrap();
    assert_eq!(transcribe(&req, whole.len()).unwrap(), whole);
    let less = transcribe(&req, whole.len() - 1).unwrap();
    assert!(less.contains(words::OMITTED));
    assert!(less.len() <= whole.len() - 1 + words::OMITTED.len());
}

#[test]
fn bad_shapes_are_errors() {
    assert!(transcribe(&json!({}), BUDGET).is_err());
    assert!(transcribe(&request(&[json!({"role": "system", "blocks": []})]), BUDGET).is_err());
    assert!(transcribe(&request(&[json!({"role": "user"})]), BUDGET).is_err());
}

#[test]
fn a_new_turn_is_one_text_per_new_message_with_the_tail_last() {
    let mut req = request(&[
        user("q1"),
        assistant("a1"),
        user("q2"),
        json!({"role": "user", "blocks": [{"type": "text", "text": "line"}, {"type": "path", "name": "a.txt", "path": "/w/a.txt"}]}),
    ]);
    assert_eq!(
        new_turn(&req, 2).unwrap(),
        [
            "q2".to_string(),
            format!("line\n{}", words::path_attached("a.txt", "/w/a.txt"))
        ]
    );
    req["tail"] = json!("<note/>");
    assert_eq!(
        new_turn(&req, 3).unwrap(),
        [format!(
            "line\n{}\n\n<note/>",
            words::path_attached("a.txt", "/w/a.txt")
        )]
    );
}

#[test]
fn a_new_turn_without_text_asks_to_continue() {
    let req = request(&[
        user("q1"),
        assistant("a1"),
        json!({"role": "user", "blocks": [{"type": "text", "text": ""}]}),
    ]);
    assert_eq!(new_turn(&req, 2).unwrap(), [words::CONTINUE]);
    assert!(new_turn(&json!({"messages": 3}), 0).is_err());
}

/// 她以前漏进群里的那几条（照以前的转写编的工具调用）留在历史里：重放时标记起的那一截不写，只剩标记前面的话，整条都是的不写
/// （施工 AG-14）。
#[test]
fn old_replies_that_copied_the_transcript_are_cut_in_the_history() {
    let messages = vec![
        user("q1"),
        assistant(
            "[called tool qq_quote with {\"msg\":\"1\"}]\n\n[tool result]\nYour next message will quote it.\n",
        ),
        user("q2"),
        assistant("在窥屏呢\n\n[tool result]\nok"),
        user("q3"),
    ];
    let got = transcribe(&request(&messages), BUDGET).unwrap();
    assert!(!got.contains("[called tool"), "{got}");
    assert!(!got.contains("[tool result"), "{got}");
    assert!(
        got.contains("User:\nq1\n\nUser:\nq2\n\nAssistant:\n在窥屏呢\n\n"),
        "{got}"
    );
}

/// 核心换成的供应商侧工具（施工 AG-16）：0.1.9 以前驱动交的块不带私有数据，核心不认得是这家的，交回来是一块
/// `<hosted-tool name=…>…</hosted-tool>` 的正文。这样的块当工具，历史里不写、这一轮的旁注里也不写；她照着写进回复的（没有收尾
/// 的那种），从标记起截掉。2026-11-28 主人群里她把这段字当回复发了两条。
#[test]
fn hosted_tools_the_core_retold_as_text_are_tools_not_lines() {
    let retold = "<hosted-tool name=\"search_web\">\n{\"query\":\"FlClash\"}\n<result>\nThe search for FlClash <\\/b>\n</result>\n</hosted-tool>";
    let messages = vec![
        user("q1"),
        json!({"role": "assistant", "blocks": [
            {"type": "text", "text": retold},
            {"type": "text", "text": "查到了"}
        ]}),
        user("q2"),
        json!({"role": "assistant", "blocks": [{"type": "text", "text": retold}]}),
        user("q3"),
        assistant("好的<hosted-tool name=\"mcp_miyu_kb_search\">\n{\"kb\":shorinwiki}\n"),
        user("q4"),
        json!({"role": "assistant", "blocks": [
            {"type": "text", "text": retold},
            {"type": "tool_call", "call_id": "c_1", "name": "kb_search", "args": "{}"}
        ]}),
        json!({"role": "tool", "blocks": [{"type": "text", "text": "found"}]}),
    ];
    let got = transcribe(&request(&messages), BUDGET).unwrap();
    assert!(!got.contains("<hosted-tool"), "{got}");
    assert!(!got.contains("<result>"), "{got}");
    assert!(
        got.contains(
            "User:\nq1\n\nAssistant:\n查到了\n\nUser:\nq2\n\nUser:\nq3\n\nAssistant:\n好的\n\nUser:\nq4\n\n"
        ),
        "{got}"
    );
}
