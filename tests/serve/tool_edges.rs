//! 带工具的边角：两边只到一边的时限、交回以后别的调用被取消、扩展退出时挂着的收干净。

use serde_json::json;

use crate::support::{Core, alive, wait_gone, wait_group_gone};
use crate::tool_rig::{
    cancel, mcp_pid, sockets, text_blocks, tool_request, wait_records, with_result,
};

fn pair_wait(name: &str, plan: &str) -> Core {
    Core::start_with(
        name,
        plan,
        &[("MIYU_ANTIGRAVITY_PAIR_WAIT_MS", "300")],
        &json!({}),
    )
}

/// 9a：流那边到了、MCP 没到：到点出错 `unclassified`、说明提 `mcp_config.json`、进程收掉。
#[test]
fn the_stream_without_the_tool_server_fails_when_the_wait_ends_and_the_agy_is_stopped() {
    let mut core = pair_wait("t-stream", "plan:stream-only");
    core.call("r1", "d1", &tool_request("s", "go"));
    let one = core.collect("r1", "d1");
    let error = &one.response["result"]["error"];
    assert_eq!(error["class"], "unclassified", "{}", one.response);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("mcp_config.json"),
        "{error}"
    );
    assert_eq!(one.text(), "Let me look. ", "前面的正文照交");
    let record = core.records().remove(0);
    wait_group_gone(record["pid"].as_i64().unwrap());
    wait_gone(mcp_pid(&record));
    assert_eq!(core.work_dirs_left(), 0);
}

/// 9b：MCP 到了、流没到：到点照交；带结果来以后接着说完。
#[test]
fn the_tool_server_without_the_stream_hands_the_call_back_when_the_wait_ends() {
    let mut core = pair_wait("t-mcp", "plan:mcp-only");
    let first = tool_request("s", "go");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    assert!(
        one.response["result"].get("error").is_none(),
        "{}",
        one.response
    );
    assert_eq!(
        one.deltas[3],
        json!({"start": {"index": 1, "kind": {"tool_call": {"name": "bash"}}}})
    );
    assert_eq!(
        one.deltas[5],
        json!({"private": {"index": 1, "data": {"mcp": 1}}})
    );
    let pid = core.records()[0]["pid"].as_i64().unwrap();
    assert!(alive(pid));

    let second = with_result(&first, &one, "call_1", text_blocks("done it"), false);
    core.call("r2", "d2", &second);
    assert_eq!(core.collect("r2", "d2").text(), "done");
    assert_eq!(core.records().len(), 1, "同一个 agy");
}

/// 两边谁先到都行：先调 MCP、后发流里的工具步。
#[test]
fn the_tool_server_first_and_the_stream_second_hands_the_call_back_without_waiting_out() {
    let mut core = Core::start("t-first", "plan:mcp-first");
    let first = tool_request("s", "go");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    assert!(
        one.response["result"].get("error").is_none(),
        "{}",
        one.response
    );
    assert_eq!(one.deltas.len(), 7, "{:?}", one.deltas);
    assert_eq!(
        one.deltas[5],
        json!({"private": {"index": 1, "data": {"mcp": 1}}})
    );
    let second = with_result(&first, &one, "call_1", text_blocks("ok"), false);
    core.call("r2", "d2", &second);
    assert_eq!(core.collect("r2", "d2").text(), "done");
}

/// 10：交回以后 `driver.cancel` 另一次正在跑的调用，不影响挂着的；扩展收到 shutdown 时挂着的 agy、MCP 服务全收掉，套接字文件删了。
#[test]
fn cancelling_another_call_leaves_the_pending_agy_and_shutdown_stops_everything() {
    let mut core = Core::start("t-cancel", "plan:two-then-hold");
    let first = tool_request("s1", "go");
    core.call("r1", "d1", &first);
    let one = core.collect("r1", "d1");
    let record = core.records().remove(0);
    let (pid, mcp) = (record["pid"].as_i64().unwrap(), mcp_pid(&record));
    assert_eq!(sockets(&core), 1);

    // 另一段对话在跑（假 agy 挂住不动），取消它。
    core.call("r2", "d2", &tool_request("s2", "other"));
    let records = wait_records(&core, 2);
    let other = records.iter().find(|record| record["pid"] != pid).unwrap();
    let other_pid = other["pid"].as_i64().unwrap();
    cancel(&mut core, "d2");
    let two = core.collect("r2", "d2");
    assert!(
        two.response["result"]["error"].is_object(),
        "{}",
        two.response
    );
    wait_group_gone(other_pid);
    assert!(alive(pid), "挂着的那段不受影响");

    // 它还接得上自己的结果，又交回第二件。
    let second = with_result(&first, &one, "call_1", text_blocks("one"), false);
    core.call("r3", "d3", &second);
    let three = core.collect("r3", "d3");
    assert_eq!(
        three.deltas[2],
        json!({"private": {"index": 0, "data": {"mcp": 2}}})
    );
    assert!(alive(pid));

    // 退出：挂着的 agy、它的 MCP 服务都收掉，套接字文件删了。
    assert!(core.finish().success());
    wait_group_gone(pid);
    wait_gone(mcp);
    assert_eq!(sockets(&core), 0, "套接字文件删了");
    assert_eq!(core.work_dirs_left(), 0);
}
