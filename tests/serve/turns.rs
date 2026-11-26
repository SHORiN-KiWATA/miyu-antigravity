//! 同一段对话的下一轮接在活着的 agy 上；认不上的新开（设计第四节第 1～3 条）。

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::support::{Core, alive, hex, next_turn, request, wait_group_gone};

/// 记录里收到的每一行。
fn lines(record: &Value) -> Vec<String> {
    record["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap().to_string())
        .collect()
}

/// 一行标准输入里的几个 text 块。
fn texts(line: &str) -> Vec<String> {
    let value: Value = serde_json::from_str(line).unwrap();
    value["message"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| {
            assert_eq!(block["type"], "text");
            block["text"].as_str().unwrap().to_string()
        })
        .collect()
}

fn sha256(text: &str) -> String {
    format!("sha256:{}", hex(&Sha256::digest(text.as_bytes())))
}

/// 回应里的一条 usage（假 agy 每轮一样）。
fn one_usage() -> Value {
    json!({"uncached": 10, "cache_read": 7, "cache_write": 0, "output": 3, "reasoning": 1, "context": 20})
}

#[test]
fn the_next_turn_goes_to_the_same_agy_and_only_the_new_words_are_written() {
    let mut core = Core::start("next-turn", "echo");
    let first = request("s", "the first question");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    assert_eq!(one.text(), "echo: rst question");

    let second = next_turn(&first, &one.text(), &["the second question"]);
    core.call("r2", "d2", &second);
    let two = core.collect("r2", "d2");
    assert_eq!(two.text(), "echo: ond question");

    let records = core.records();
    assert_eq!(records.len(), 1, "没有再起 agy");
    let lines = lines(&records[0]);
    assert_eq!(lines.len(), 2, "同一个进程收了两行");
    assert_eq!(
        lines[1],
        "{\"event\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"the second question\"}]}}\n",
        "第二行只有新的那句话"
    );
    assert_eq!(two.response["result"]["sent"], sha256(&lines[1]));
    // 每一次的块编号、用量各算各的。
    for got in [&one, &two] {
        assert_eq!(
            got.deltas[0],
            json!({"start": {"index": 0, "kind": "text"}})
        );
        assert_eq!(got.response["result"]["usage"], one_usage());
    }

    // 第三轮照样接上。
    let third = next_turn(&second, &two.text(), &["and a third one"]);
    core.call("r3", "d3", &third);
    assert_eq!(core.collect("r3", "d3").text(), "echo:  a third one");
    assert_eq!(core.records().len(), 1);
    assert!(alive(records[0]["pid"].as_i64().unwrap()));
}

#[test]
fn two_new_user_messages_are_two_text_blocks_in_one_line() {
    let mut core = Core::start("two-users", "echo");
    let first = request("s", "hello");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    let mut second = next_turn(&first, &one.text(), &["one", "two"]);
    second["tail"] = json!("<note/>");
    core.call("r2", "d2", &second);
    assert_eq!(core.collect("r2", "d2").text(), "echo: two\n\n<note/>");
    let records = core.records();
    assert_eq!(records.len(), 1);
    let lines = lines(&records[0]);
    assert_eq!(texts(&lines[1]), ["one", "two\n\n<note/>"]);
}

#[test]
fn the_same_request_again_starts_a_new_agy() {
    let mut core = Core::start("again", "echo");
    let first = request("s", "regenerate me");
    core.call("r1", "d1", &first);
    core.collect("r1", "d1");
    core.call("r2", "d2", &first);
    core.collect("r2", "d2");
    let records = core.records();
    assert_eq!(records.len(), 2, "同一份请求再来一次：新开");
    for record in &records {
        assert_eq!(texts(&lines(record)[0]), ["regenerate me"]);
    }
    // 同一段对话的旧的那个被新的换掉、收掉了。
    let pids: Vec<i64> = records
        .iter()
        .map(|record| record["pid"].as_i64().unwrap())
        .collect();
    let (gone, kept): (Vec<i64>, Vec<i64>) = pids.iter().partition(|pid| !alive(**pid));
    assert_eq!((gone.len(), kept.len()), (1, 1), "{pids:?}");
}

#[test]
fn a_changed_system_or_model_starts_a_new_agy_with_the_whole_history() {
    let mut core = Core::start("changed", "echo");
    let first = request("s", "first words");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");

    let mut other_system = next_turn(&first, &one.text(), &["second words"]);
    other_system["system"] = json!("another system prompt");
    core.call("r2", "d2", &other_system);
    core.collect("r2", "d2");
    assert_eq!(core.records().len(), 2, "换了 system：新开");

    let same_system = next_turn(&first, &one.text(), &["third words"]);
    core.call_model("r3", "d3", &same_system, "claude-opus-5-5-high");
    core.collect("r3", "d3");
    let records = core.records();
    assert_eq!(records.len(), 3, "换了模型：新开");
    let newest = records
        .iter()
        .find(|record| {
            record["argv"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg == "claude-opus-5-5-high")
        })
        .unwrap();
    let text = &texts(&lines(newest)[0])[0];
    assert!(
        text.contains(
            "<conversation-history>\nUser:\nfirst words\n\nAssistant:\necho: first words"
        ) && text.ends_with("third words"),
        "整份重放：{text}"
    );
}

#[test]
fn a_tool_message_in_the_next_turn_starts_a_new_agy() {
    let mut core = Core::start("tool", "echo");
    let first = request("s", "read the file");
    core.call("r1", "d1", &first);
    core.collect("r1", "d1");
    let original = core.records()[0]["pid"].as_i64().unwrap();
    let mut second = first.clone();
    let messages = second["messages"].as_array_mut().unwrap();
    messages.push(json!({"role": "assistant", "blocks": [
        {"type": "text", "text": "reading"},
        {"type": "tool_call", "id": "c1", "name": "read", "args": "{\"path\":\"a.txt\"}"}
    ]}));
    messages.push(
        json!({"role": "tool", "call": "c1", "blocks": [{"type": "text", "text": "file body"}]}),
    );
    messages.push(json!({"role": "user", "blocks": [{"type": "text", "text": "thanks"}]}));
    core.call("r2", "d2", &second);
    assert_eq!(core.collect("r2", "d2").text(), "echo: ory>\n\nthanks");
    let records = core.records();
    assert_eq!(records.len(), 2, "夹着工具结果：新开");
    assert_eq!(
        lines(&core.record_of(original)).len(),
        1,
        "原来的那个没收到第二行"
    );
    let replayed = records
        .iter()
        .find(|record| record["pid"] != original)
        .unwrap();
    let text = &texts(&lines(replayed)[0])[0];
    assert!(
        text.contains("[called tool read with {\"path\":\"a.txt\"}]")
            && text.contains("[tool result]\nfile body"),
        "整份重放里有那次工具调用和结果：{text}"
    );
}

#[test]
fn a_busy_conversation_is_not_shared() {
    let mut core = Core::start("busy", "echo");
    let first = request("s", "start here");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    let original = core.records()[0]["pid"].as_i64().unwrap();

    // 两个「下一轮」同时来：一个接上，另一个碰上它正忙着，新开。
    let left = next_turn(&first, &one.text(), &["the left branch"]);
    let right = next_turn(&first, &one.text(), &["the right branch"]);
    core.call("r2", "d2", &left);
    core.call("r3", "d3", &right);
    let (got, _) = core.collect_all(&[("r2", "d2"), ("r3", "d3")]);
    assert_eq!(got["d2"].text(), "echo:  left branch");
    assert_eq!(got["d3"].text(), "echo: right branch");
    for collected in got.values() {
        assert_eq!(collected.response["result"]["usage"], one_usage());
    }

    let records = core.records();
    assert_eq!(records.len(), 2);
    let reused = core.record_of(original);
    let reused_lines = lines(&reused);
    assert_eq!(reused_lines.len(), 2, "原来的那个接了一个");
    let other = records
        .iter()
        .find(|record| record["pid"] != original)
        .unwrap();
    let other_lines = lines(other);
    assert_eq!(other_lines.len(), 1, "另一个新开");
    let (on_reused, on_new) = (&texts(&reused_lines[1])[0], &texts(&other_lines[0])[0]);
    assert!(
        (on_reused == "the left branch" && on_new.ends_with("the right branch"))
            || (on_reused == "the right branch" && on_new.ends_with("the left branch")),
        "{on_reused:?} / {on_new:?}"
    );
    assert!(core.finish().success());
    wait_group_gone(original);
}
