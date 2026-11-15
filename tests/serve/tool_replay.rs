//! 挂着的工具调用对不上的几种情形：旧 agy 收掉、整份重放（设计第四节第 2 条的最后几点、第五节）。

use serde_json::{Value, json};

use crate::support::{Core, alive, assert_conversation_removed, wait_gone, wait_group_gone};
use crate::tool_rig::{
    mcp_pid, record_other_than, stdin_text, text_blocks, tool_request, wait_records, with_result,
};

const CALLED: &str = r#"[called tool bash with {"command":"ls -l","n":2}]"#;
const CALLED_LS: &str = r#"[called tool bash with {"command":"ls"}]"#;

/// 第一次调用交回一件工具调用，挂着；交出请求、交回的、agy 进程号、会话编号、MCP 服务进程号。
fn hang(core: &mut Core) -> (Value, crate::support::Collected, i64, String, i64) {
    let first = tool_request("s", "list the files");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    let record = core.records().remove(0);
    let pid = record["pid"].as_i64().unwrap();
    let conversation = record["conversation"].as_str().unwrap().to_string();
    assert!(alive(pid));
    (first, one, pid, conversation, mcp_pid(&record))
}

/// 旧 agy（连同它的 MCP 服务）收掉了，会话文件删了。
fn assert_old_gone(core: &Core, pid: i64, conversation: &str, mcp: i64) {
    wait_group_gone(pid);
    wait_gone(mcp);
    assert_conversation_removed(&core.home, conversation);
}

/// 新起的 agy 收到的是整份重放，里面有那件工具调用和它的结果。
fn assert_replayed(core: &Core, old: i64, called: &str, result: &str) -> Value {
    let record = record_other_than(core, old);
    let text = stdin_text(&record, 0);
    assert!(text.contains("<conversation-history>"), "{text}");
    assert!(text.contains(called), "{text}");
    assert!(text.contains(&format!("[tool result]\n{result}")), "{text}");
    record
}

/// 4：tool 后面又跟了一条 user（人在她干活时排进来的话）。
#[test]
fn a_user_message_after_the_tool_result_discards_the_pending_agy_and_replays() {
    let mut core = Core::start("t-user", "plan:replay");
    let (first, one, pid, conversation, mcp) = hang(&mut core);
    let mut second = with_result(&first, &one, "call_1", text_blocks("file-a"), false);
    second["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "user", "blocks": [{"type": "text", "text": "also, be quick"}]}));
    core.call("r2", "d2", &second);
    assert_eq!(core.collect("r2", "d2").text(), "replayed");
    assert_old_gone(&core, pid, &conversation, mcp);
    let record = assert_replayed(&core, pid, CALLED, "file-a");
    assert!(stdin_text(&record, 0).ends_with("also, be quick"));
    assert_eq!(core.work_dirs_left(), 1, "旧的工作目录删了，只剩新的");
}

/// 5：assistant 里的私有数据的 n 不对、`call_id` 对不上、没有私有数据、两个工具调用块：都作废重放。结尾是 tool 结果，补一句让它接着做。
#[test]
fn a_wrong_number_or_call_id_discards_the_pending_agy_and_replays() {
    type Tamper = fn(&mut Value);
    let cases: [(&str, Tamper); 4] = [
        ("wrong mcp number", |request| {
            request["messages"][1]["blocks"][1]["private"]["data"] = json!({"mcp": 7});
        }),
        ("wrong call id", |request| {
            request["messages"][2]["call_id"] = json!("call_9");
        }),
        ("no private data", |request| {
            request["messages"][1]["blocks"][1]
                .as_object_mut()
                .unwrap()
                .remove("private");
        }),
        ("two tool calls", |request| {
            let blocks = request["messages"][1]["blocks"].as_array_mut().unwrap();
            let again = blocks[1].clone();
            blocks.push(again);
        }),
    ];
    for (index, (name, tamper)) in cases.into_iter().enumerate() {
        let mut core = Core::start(&format!("t-bad{index}"), "plan:replay");
        let (first, one, pid, conversation, mcp) = hang(&mut core);
        let mut second = with_result(&first, &one, "call_1", text_blocks("file-a"), false);
        tamper(&mut second);
        core.call("r2", "d2", &second);
        let two = core.collect("r2", "d2");
        assert_eq!(two.text(), "replayed", "{name}");
        assert_old_gone(&core, pid, &conversation, mcp);
        let record = record_other_than(&core, pid);
        let text = stdin_text(&record, 0);
        assert!(text.contains("[tool result]\nfile-a"), "{name}: {text}");
        assert!(
            text.ends_with("Continue from where the conversation above left off."),
            "{name}: {text}"
        );
    }
}

/// 6：同一次请求原样再来一遍：挂着的被收掉、新开。
#[test]
fn the_same_request_again_discards_the_pending_agy_and_starts_a_new_one() {
    let mut core = Core::start("t-same", "plan:replay");
    let (first, _, pid, conversation, mcp) = hang(&mut core);
    core.call("r2", "d2", &first);
    let two = core.collect("r2", "d2");
    assert_eq!(two.text(), "replayed");
    assert_old_gone(&core, pid, &conversation, mcp);
    let new = record_other_than(&core, pid);
    assert_ne!(new["pid"], pid);
    assert!(alive(new["pid"].as_i64().unwrap()));
}

/// 7：挂着超过上限（环境变量改短）：巡检收掉；之后带结果来，新开重放。
#[test]
fn a_pending_agy_is_reaped_after_the_tool_wait_and_the_result_then_replays() {
    let mut core = Core::start_with(
        "t-wait",
        "plan:replay",
        &[
            ("MIYU_ANTIGRAVITY_TOOL_WAIT_MS", "300"),
            ("MIYU_ANTIGRAVITY_SWEEP_MS", "50"),
        ],
        &json!({}),
    );
    let (first, one, pid, conversation, mcp) = hang(&mut core);
    assert_old_gone(&core, pid, &conversation, mcp);
    let second = with_result(&first, &one, "call_1", text_blocks("file-a"), false);
    core.call("r2", "d2", &second);
    assert_eq!(core.collect("r2", "d2").text(), "replayed");
    assert_replayed(&core, pid, CALLED, "file-a");
}

/// 8：挂着时 agy 自己说话了（ERROR 那一步、正文、result）：带结果来时作废、新开。
#[test]
fn an_agy_that_spoke_while_pending_is_discarded_when_the_result_comes() {
    let mut core = Core::start("t-spoke", "plan:speaks");
    let first = tool_request("s", "list the files");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    let record = core.records().remove(0);
    let (pid, conversation, mcp) = (
        record["pid"].as_i64().unwrap(),
        record["conversation"].as_str().unwrap().to_string(),
        mcp_pid(&record),
    );
    // 让它说话；它塞满管道才写 spoke，所以那几行扩展都读过了。
    std::fs::write(core.record.join("speak"), "").unwrap();
    core.wait_record("spoke");
    let second = with_result(&first, &one, "call_1", text_blocks("file-a"), false);
    core.call("r2", "d2", &second);
    assert_eq!(core.collect("r2", "d2").text(), "replayed");
    assert_old_gone(&core, pid, &conversation, mcp);
    assert_replayed(&core, pid, CALLED_LS, "file-a");
}

/// 挂着的那段自己还在时，别的消息（一模一样前缀的别的对话）不受影响；挂着的不被没关系的请求动。
#[test]
fn an_unrelated_request_leaves_the_pending_agy_alone_and_it_still_takes_its_result() {
    let mut core = Core::start("t-other", "plan:one");
    let first = tool_request("s", "list the files");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    let pid = core.records()[0]["pid"].as_i64().unwrap();

    // 另一段对话（系统提示词不同）：它自己起一个 agy，挂着的不动。
    let other = tool_request("another system", "something else");
    core.call("r2", "d2", &other);
    let two = core.collect("r2", "d2");
    assert_eq!(two.deltas.last().unwrap(), &json!({"end": {"index": 1}}));
    assert!(alive(pid), "挂着的那段还在");
    assert_eq!(wait_records(&core, 2).len(), 2);

    let third = with_result(&first, &one, "call_1", text_blocks("ok"), false);
    core.call("r3", "d3", &third);
    assert_eq!(core.collect("r3", "d3").text(), "Listing received.");
    assert!(alive(pid));
}
