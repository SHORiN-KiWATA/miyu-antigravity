use miyu_antigravity::words;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::support::{Core, alive, assert_conversation_removed, request, wait_group_gone};

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .map(|nibble| char::from_digit(u32::from(nibble), 16).expect("a nibble is a hex digit"))
        .collect()
}

/// 系统提示词对应的代理名：`miyu-` 加「白名单 NUL system」的 SHA-256 前 16 位。
fn agent_name(system: &str) -> String {
    let input = format!("search_web\0{system}\n\n{}", words::AGENT_NOTE);
    format!("miyu-{}", &hex(&Sha256::digest(input.as_bytes()))[..16])
}

#[test]
fn handshake_models_unknown_methods_and_clean_exit() {
    let mut core = Core::start("models", "sample:real-pong.jsonl");
    core.send(&json!({"jsonrpc": "2.0", "id": "m1", "method": "driver.models", "params": {"driver": "antigravity/antigravity"}}));
    let answer = core.recv();
    assert_eq!(answer["id"], "m1");
    assert_eq!(
        answer["result"],
        json!({"models": [{"id": "gemini-3.8-flash-low"}, {"id": "claude-opus-5-5-high"}]})
    );

    core.send(&json!({"jsonrpc": "2.0", "id": 7, "method": "driver.encode", "params": {}}));
    let refused = core.recv();
    assert_eq!(refused["id"], 7);
    assert_eq!(refused["error"]["code"], -32601);
    // 不认识的通知不回。
    core.send(&json!({"jsonrpc": "2.0", "method": "something.new", "params": {}}));
    core.send(&json!({"jsonrpc": "2.0", "id": "m2", "method": "driver.models", "params": {}}));
    assert_eq!(core.recv()["id"], "m2");
    assert!(core.finish().success(), "标准输入关了就照常退出");
}

#[test]
fn models_that_cannot_be_listed_are_a_jsonrpc_error() {
    let mut core = Core::start("models-fail", "models-fail");
    core.send(&json!({"jsonrpc": "2.0", "id": "m1", "method": "driver.models", "params": {}}));
    let answer = core.recv();
    assert_eq!(answer["error"]["code"], -32603);
    let message = answer["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("agy models") && message.contains("not logged into"),
        "{message}"
    );
}

#[test]
fn pong_streams_text_and_usage_and_cleans_up() {
    let mut core = Core::start("pong", "sample:real-pong.jsonl");
    let system = "You are terse.\nAnswer in one word.";
    core.call("r1", "d1", &request(system, "ping"));
    let got = core.collect("r1", "d1");
    assert_eq!(
        got.deltas,
        vec![
            json!({"start": {"index": 0, "kind": "text"}}),
            json!({"text": {"index": 0, "text": "pong"}}),
            json!({"text": {"index": 0, "text": "\n"}}),
            json!({"end": {"index": 0}}),
        ]
    );
    let result = &got.response["result"];
    assert_eq!(
        result["usage"],
        json!({"uncached": 2679, "cache_read": 0, "cache_write": 0, "output": 1, "reasoning": 0})
    );

    // 假 agy 收到的命令行、工作目录、HOME、标准输入那一行。
    let records = core.records();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    let agent = agent_name(system);
    let work = record["cwd"].as_str().unwrap();
    assert!(
        std::path::Path::new(work).starts_with(core.state.join("work")),
        "工作目录在状态目录的 work/ 下：{work}"
    );
    assert_eq!(
        record["argv"],
        json!([
            "--print=",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--dangerously-skip-permissions",
            "--model",
            "gemini-3.8-flash-low",
            "--agent",
            agent,
            "--add-dir",
            work,
            "--print-timeout",
            "86400s"
        ])
    );
    assert_eq!(record["home"], core.home.to_str().unwrap());
    let stdin = record["stdin"].as_str().unwrap();
    assert_eq!(
        stdin,
        "{\"event\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"ping\"}]}}\n"
    );
    assert_eq!(
        result["sent"],
        format!("sha256:{}", hex(&Sha256::digest(stdin.as_bytes())))
    );

    // 代理文件写在扩展的 HOME 下（agy 也照它跑，见上面的 record["home"]）。
    let agent_file = core
        .home
        .join(".gemini/config/agents")
        .join(&agent)
        .join("agent.md");
    assert_eq!(
        std::fs::read_to_string(agent_file).unwrap(),
        format!(
            "---\nname: {agent}\ndescription: Miyu system prompt (written by the Miyu antigravity package; renamed whenever the prompt changes)\nmainAgent: true\nsubagent: false\ntools: [search_web]\n---\n\n{system}\n\n{}",
            words::AGENT_NOTE
        )
    );
    assert!(
        !core.state.join(".gemini").exists(),
        "agy 的东西只在 HOME 下"
    );

    // AG-2：说完以后 agy 还在、登记着等下一轮，会话文件、工作目录都留着。
    let pid = record["pid"].as_i64().unwrap();
    let conversation = record["conversation"].as_str().unwrap();
    assert!(alive(pid), "说完以后 agy 还在");
    assert!(
        core.home
            .join(".gemini/antigravity-cli/conversations")
            .join(format!("{conversation}.db"))
            .exists()
    );
    assert_eq!(core.work_dirs_left(), 1);
    // 扩展退出时收掉：整组没了，会话文件、工作目录都删了。
    assert!(core.finish().success());
    wait_group_gone(pid);
    assert_conversation_removed(&core.home, conversation);
    assert_eq!(core.work_dirs_left(), 0);
}

#[test]
fn search_read_streams_only_the_answer_text_and_sums_usage() {
    let mut core = Core::start("search", "sample:real-search-read.jsonl");
    core.call("r1", "d1", &request("s", "read the spec"));
    let got = core.collect("r1", "d1");
    // AG-4：工具步交成供应商侧工具块（块 0、1，各带参数的字），正文是块 2。
    let kinds: Vec<&Value> = got
        .deltas
        .iter()
        .filter_map(|delta| delta.pointer("/start/kind"))
        .collect();
    assert_eq!(
        kinds,
        [
            &json!({"hosted_tool": {"name": "search_web"}}),
            &json!({"hosted_tool": {"name": "read_url_content"}}),
            &json!("text"),
        ],
        "没有字的步不开块"
    );
    let answer: String = got
        .deltas
        .iter()
        .filter_map(|delta| delta.get("text").filter(|text| text["index"] == 2))
        .filter_map(|text| text["text"].as_str())
        .collect();
    assert_eq!(
        answer,
        "NO-TOOL; the page title is \"EditorConfig Specification — EditorConfig Specification 0.17.2 documentation\".\n"
    );
    assert_eq!(
        got.response["result"]["usage"],
        json!({"uncached": 7553, "cache_read": 5966, "cache_write": 0, "output": 110, "reasoning": 0})
    );
}

#[test]
fn history_is_transcribed_into_one_line() {
    let mut core = Core::start("history", "sample:real-pong.jsonl");
    let mut req = request("s", "first");
    req["messages"] = json!([
        {"role": "user", "blocks": [{"type": "text", "text": "first"}]},
        {"role": "assistant", "blocks": [{"type": "text", "text": "one"}]},
        {"role": "user", "blocks": [{"type": "text", "text": "second"}]}
    ]);
    req["tail"] = json!("<note/>");
    core.call("r1", "d1", &req);
    core.collect("r1", "d1");
    let stdin = core.records()[0]["stdin"].as_str().unwrap().to_string();
    let line: Value = serde_json::from_str(&stdin).unwrap();
    let text = line["message"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("<conversation-history>\nUser:\nfirst\n\nAssistant:\none\n\n</conversation-history>\n\nsecond\n\n<note/>"), "{text}");
}

#[test]
fn two_calls_at_once_each_get_their_own_deltas() {
    let mut core = Core::start("pair", "echo");
    core.call("r1", "d1", &request("s", "the first question"));
    core.call("r2", "d2", &request("s", "the second question"));
    let (got, order) = core.collect_all(&[("r1", "d1"), ("r2", "d2")]);
    // 同时在跑：第二次的头一条增量在第一次回答之前就来了（假 agy 一次要 0.6 秒以上）。
    let position = |what: &str| order.iter().position(|seen| seen == what).unwrap();
    assert!(position("delta:d2") < position("answer:d1"), "{order:?}");
    assert_eq!(got["d1"].text(), "echo: rst question");
    assert_eq!(got["d2"].text(), "echo: ond question");
    for (call, collected) in &got {
        assert_eq!(
            collected.deltas.first().unwrap()["start"]["index"],
            0,
            "{call} 的块从 0 数"
        );
        assert_eq!(
            collected.response["result"]["usage"],
            json!({"uncached": 10, "cache_read": 7, "cache_write": 0, "output": 3, "reasoning": 1})
        );
    }
    let records = core.records();
    assert_eq!(records.len(), 2, "两个 agy 进程");
    assert_ne!(records[0]["cwd"], records[1]["cwd"], "各有各的工作目录");
    // AG-2：两段对话都留着；扩展退出时一起收掉。
    assert_eq!(core.work_dirs_left(), 2);
    assert!(core.finish().success());
    for record in &records {
        wait_group_gone(record["pid"].as_i64().unwrap());
        assert_conversation_removed(&core.home, record["conversation"].as_str().unwrap());
    }
    assert_eq!(core.work_dirs_left(), 0);
}
