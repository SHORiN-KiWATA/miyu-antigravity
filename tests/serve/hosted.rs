//! agy 自己跑的工具照实交成供应商侧工具块（设计第七节）、内容策略拦截（第八节）。真 `serve` + 假 agy（`tests/fake_agy.py`
//! 的 `plan:` 剧本，转录是假 agy 往它那个 `brain/<编号>/…/transcript_full.jsonl` 里写的，写法照真样本）。

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::support::{
    Collected, Core, assert_conversation_removed, hex, next_turn, request, wait_gone,
};
use crate::tool_rig::{stdin_text, text_blocks, tool_message, tool_request};

/// 假 agy 剧本里 `search_web` 在转录里写的字（去掉两行时间以后）。
const FOUND: &str = "The search for \"EditorConfig specification\" returned the following summary:\nA simple file format [1].";
const PARAMS: &str = r#"{"query":"EditorConfig specification"}"#;

fn hosted_start(index: u64) -> Value {
    json!({"start": {"index": index, "kind": {"hosted_tool": {"name": "search_web"}}}})
}

fn params(index: u64) -> Value {
    json!({"text": {"index": index, "text": PARAMS}})
}

fn end(index: u64) -> Value {
    json!({"end": {"index": index}})
}

fn outcome(index: u64, text: &str) -> Value {
    json!({"outcome": {"index": index, "text": text}})
}

fn text_block(index: u64, words: &[&str]) -> Vec<Value> {
    let mut deltas = vec![json!({"start": {"index": index, "kind": "text"}})];
    deltas.extend(
        words
            .iter()
            .map(|word| json!({"text": {"index": index, "text": word}})),
    );
    deltas.push(end(index));
    deltas
}

fn succeeded(got: &Collected) {
    assert!(
        got.response["result"].get("error").is_none(),
        "{}",
        got.response
    );
}

/// 一次调用，读到回应。
fn run(name: &str, mode: &str, text: &str) -> (Core, Collected) {
    let mut core = Core::start(name, mode);
    core.call("r1", "d1", &request("s", text));
    let got = core.collect("r1", "d1");
    (core, got)
}

/// 1. 搜索走完：供应商侧工具块（名字、参数、结果是转录里的字去掉两行时间）、后面的正文、`result`。
#[test]
fn a_search_is_a_hosted_tool_block_with_the_transcript_result_then_the_text() {
    let (_core, got) = run("h-search", "plan:search", "search for it");
    let mut expected = vec![hosted_start(0), params(0), outcome(0, FOUND), end(0)];
    expected.extend(text_block(1, &["Found it."]));
    assert_eq!(got.deltas, expected);
    succeeded(&got);
    assert_eq!(
        got.response["result"]["usage"],
        json!({"uncached": 10, "cache_read": 7, "cache_write": 0, "output": 3, "reasoning": 1})
    );
    assert!(
        !got.deltas
            .iter()
            .any(|delta| delta.get("private").is_some()),
        "供应商侧工具不带私有数据"
    );
}

/// 正文、供应商侧工具块、正文混在一次里：块先后和 `index` 一块接一块。
#[test]
fn text_a_hosted_block_and_more_text_keep_their_order_and_indexes() {
    let (_core, got) = run("h-between", "plan:search-between", "search for it");
    let mut expected = text_block(0, &["Let me search. "]);
    expected.extend([hosted_start(1), params(1), outcome(1, FOUND), end(1)]);
    expected.extend(text_block(2, &["Found it."]));
    assert_eq!(got.deltas, expected);
    succeeded(&got);
}

/// 2. 转录晚写 300 毫秒照样读得到。
#[test]
fn a_transcript_row_written_300_ms_late_is_still_read() {
    let (_core, got) = run("h-late", "plan:search-late", "search for it");
    assert_eq!(
        got.deltas[..4],
        [hosted_start(0), params(0), outcome(0, FOUND), end(0)]
    );
    succeeded(&got);
}

/// 2. 一直没写的到点不交 `outcome`、照样收块，这一次不出错。
#[test]
fn a_transcript_that_never_gets_the_row_gives_the_block_without_a_result_and_no_error() {
    let started = Instant::now();
    let (_core, got) = run("h-never", "plan:search-never", "search for it");
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "要等够 1 秒才放弃"
    );
    let mut expected = vec![hosted_start(0), params(0), end(0)];
    expected.extend(text_block(1, &["Found it."]));
    assert_eq!(got.deltas, expected);
    succeeded(&got);
}

/// 3. `DONE` 带 `output` 的用流里的（转录里另有不同的字）。
#[test]
fn a_done_with_output_uses_the_stream_and_not_the_transcript() {
    let (_core, got) = run("h-output", "plan:search-output", "search for it");
    assert_eq!(
        got.deltas[..4],
        [
            hosted_start(0),
            params(0),
            outcome(0, "from the stream"),
            end(0)
        ]
    );
    succeeded(&got);
}

/// 3. `ERROR` 的 `failed: true`，字是流里的出错原话。
#[test]
fn an_error_step_is_a_failed_outcome_with_the_streams_words() {
    let (_core, got) = run("h-error", "plan:search-error", "search for it");
    assert_eq!(
        got.deltas[..4],
        [
            hosted_start(0),
            params(0),
            json!({"outcome": {"index": 0, "text": "search backend failed", "failed": true}}),
            end(0)
        ]
    );
    succeeded(&got);
}

/// 只有 `DONE`（没见过 `ACTIVE`）也算头一次。
#[test]
fn a_step_that_comes_as_done_alone_is_a_block_too() {
    let (_core, got) = run("h-done-only", "plan:search-done-only", "search for it");
    assert_eq!(
        got.deltas[..4],
        [hosted_start(0), params(0), outcome(0, FOUND), end(0)]
    );
    succeeded(&got);
}

/// 一条 assistant 消息：照核心的记法拼回我们交上来的块（正文、供应商侧工具、工具调用）。
fn assistant(got: &Collected) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    for delta in &got.deltas {
        if let Some(start) = delta.get("start") {
            let kind = &start["kind"];
            blocks.push(if kind == "text" {
                json!({"type": "text", "text": ""})
            } else if let Some(hosted) = kind.get("hosted_tool") {
                json!({"type": "hosted_tool", "call_id": "h_1", "name": hosted["name"], "args": ""})
            } else {
                json!({"type": "tool_call", "call_id": "call_1", "name": kind["tool_call"]["name"], "args": ""})
            });
        } else if let Some(piece) = delta.get("text") {
            let last = blocks.last_mut().unwrap();
            let key = if last["type"] == "text" {
                "text"
            } else {
                "args"
            };
            let joined = format!(
                "{}{}",
                last[key].as_str().unwrap(),
                piece["text"].as_str().unwrap()
            );
            last[key] = json!(joined);
        } else if let Some(done) = delta.get("outcome") {
            blocks.last_mut().unwrap()["outcome"] = json!({"text": done["text"]});
        } else if let Some(private) = delta.get("private") {
            blocks.last_mut().unwrap()["private"] =
                json!({"driver": "antigravity", "data": private["data"]});
        }
    }
    json!({"role": "assistant", "blocks": blocks})
}

/// 4. 同一次里先搜索、再调 Miyu 的工具（交回）：块的 `index` 0、1，回答没有 `error`，带结果再来接得上。
#[test]
fn a_search_then_a_miyu_tool_call_hands_back_with_indexes_0_and_1_and_the_result_resumes() {
    let mut core = Core::start("h-then-call", "plan:search-then-call");
    let first = tool_request("s", "search then list");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    assert_eq!(
        one.deltas,
        vec![
            hosted_start(0),
            params(0),
            outcome(0, FOUND),
            end(0),
            json!({"start": {"index": 1, "kind": {"tool_call": {"name": "bash"}}}}),
            json!({"text": {"index": 1, "text": r#"{"command":"ls -l","n":2}"#}}),
            json!({"private": {"index": 1, "data": {"mcp": 1}}}),
            end(1),
        ]
    );
    succeeded(&one);
    let pid = core.records()[0]["pid"].as_i64().unwrap();

    let mut second = first.clone();
    let messages = second["messages"].as_array_mut().unwrap();
    messages.push(assistant(&one));
    messages.push(tool_message("call_1", text_blocks("file-a"), false));
    core.call("r2", "d2", &second);
    let two = core.collect("r2", "d2");
    assert_eq!(two.text(), "after");
    succeeded(&two);
    let records = core.records();
    assert_eq!(records.len(), 1, "接得上：没有新起 agy");
    assert_eq!(records[0]["pid"], pid);
    assert_eq!(
        records[0]["replies"],
        json!([{"content": [{"type": "text", "text": "file-a"}], "isError": false}])
    );
}

/// 5. 复用的进程（第二轮）里搜索：对话编号从 `Live` 来（第二轮没有 `init`），读得到。
#[test]
fn a_search_in_a_reused_process_reads_the_transcript_by_the_kept_conversation_id() {
    let mut core = Core::start("h-second", "plan:search-second");
    let first = request("s", "hello");
    core.call("r1", "d1", &first);
    assert_eq!(core.collect("r1", "d1").text(), "echo: hello");
    let pid = core.records()[0]["pid"].as_i64().unwrap();

    let second = next_turn(&first, "echo: hello", &["now search"]);
    core.call("r2", "d2", &second);
    let two = core.collect("r2", "d2");
    let mut expected = vec![hosted_start(0), params(0), outcome(0, FOUND), end(0)];
    expected.extend(text_block(1, &["Found it."]));
    assert_eq!(two.deltas, expected);
    succeeded(&two);
    let records = core.records();
    assert_eq!(records.len(), 1, "还是同一个 agy");
    assert_eq!(records[0]["pid"], pid);
    assert_eq!(stdin_text(&records[0], 1), "now search");
}

/// 6. 超过 64 KiB 的结果截了、没切开字。
#[test]
fn a_result_over_64_kib_is_cut_on_a_char_boundary() {
    let (_core, got) = run("h-big", "plan:search-big", "search for it");
    let sent = got.deltas[2]["outcome"]["text"].as_str().unwrap();
    assert_eq!(sent.len(), 65535, "65538 字节的结果退到字边界");
    assert_eq!(sent, "字".repeat(21845));
    assert!(got.line_lengths.iter().all(|length| *length < 1 << 20));
    succeeded(&got);
}

/// 7. 拦截文案：这一次是 `content_policy`、一条正文增量都没交、agy 进程收掉了。
#[test]
fn a_content_policy_notice_is_not_delivered_and_ends_the_call_with_content_policy() {
    for mode in ["plan:policy", "plan:policy-single"] {
        let (core, got) = run("h-policy", mode, "something");
        assert!(
            got.deltas.is_empty(),
            "{mode}: 一条增量都不交：{:?}",
            got.deltas
        );
        let error = &got.response["result"]["error"];
        assert_eq!(error["class"], "content_policy", "{mode}: {}", got.response);
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("could not be submitted"),
            "{error}"
        );
        // 这段对话作废：进程收掉、会话文件和工作目录都删了。
        let record = core.wait_record("conversation");
        wait_gone(record["pid"].as_i64().unwrap());
        assert_conversation_removed(&core.home, record["conversation"].as_str().unwrap());
        assert_eq!(core.work_dirs_left(), 0);
    }
}

/// 7. 前半一样、后面不一样的：字全交了，逐字节对。
#[test]
fn text_that_starts_like_the_notice_but_differs_is_delivered_in_full_and_byte_for_byte() {
    let (_core, got) = run("h-lookalike", "plan:policy-lookalike", "something");
    assert_eq!(
        got.deltas,
        text_block(0, &["The prompt could not be ", "found in the cache."])
    );
    assert_eq!(got.text(), "The prompt could not be found in the cache.");
    succeeded(&got);

    // 只写了开头一半就说完：也照常交。
    let (_core, half) = run("h-half", "plan:policy-half", "something");
    assert_eq!(half.deltas, text_block(0, &["The prompt could "]));
    succeeded(&half);
}

/// 8. 代理文件里白名单是 `search_web`；`init.agent` 核对照旧过（这一次成功、没有「代理没挂上」的出错）。
#[test]
fn the_agent_file_whitelists_search_web_and_the_agent_check_still_passes() {
    let system = "s"; // `run` 的请求里的系统提示词
    let (core, got) = run("h-agent", "plan:search", "search for it");
    succeeded(&got);
    let body = format!("{system}\n\n{}", miyu_antigravity::words::AGENT_NOTE);
    let digest = Sha256::digest(format!("search_web,view_file\0{body}").as_bytes());
    let name = format!("miyu-{}", &hex(&digest[..8])[..16]);
    let record = core.records().remove(0);
    let argv: Vec<&str> = record["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        argv[argv.iter().position(|arg| *arg == "--agent").unwrap() + 1],
        name
    );
    let file = core
        .home
        .join(".gemini/config/agents")
        .join(&name)
        .join("agent.md");
    let content = std::fs::read_to_string(file).unwrap();
    assert!(
        content.contains("\ntools: [search_web, view_file]\n"),
        "{content}"
    );
    assert!(!content.contains("tools: []"), "{content}");
    assert!(content.ends_with(&format!("---\n\n{body}")), "{content}");
}
