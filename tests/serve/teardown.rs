//! 作废和回收（设计第八节）：出错、取消、流断了的那段对话作废；闲着的 agy 自己退了、闲置到点、超过 4 段、扩展退出时收掉；
//! 旧的代理目录回收。

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::support::{
    Core, alive, assert_conversation_removed, next_turn, request, wait_gone, wait_group_gone,
};

/// 巡检间隔调小（毫秒），只给测试用的环境变量。
const FAST_SWEEP: (&str, &str) = ("MIYU_ANTIGRAVITY_SWEEP_MS", "50");

/// 第一轮说完；交第一轮的请求、回答的字、那个 agy 的记录。
fn first_turn(core: &mut Core, words: &str) -> (Value, String, Value) {
    let first = request("s", words);
    core.call("r1", "d1", &first);
    let got = core.collect("r1", "d1");
    assert!(
        got.response["result"].get("error").is_none(),
        "{}",
        got.response
    );
    let record = core.records().into_iter().next().unwrap();
    (first, got.text(), record)
}

/// 记录里的进程号、对话编号。
fn ids(record: &Value) -> (i64, String) {
    (
        record["pid"].as_i64().unwrap(),
        record["conversation"].as_str().unwrap().to_string(),
    )
}

/// 第二轮出了岔子以后：那个 agy（和它的子进程）没了、会话文件删了、工作目录删了；同一份请求重来照新开，答得上。
fn assert_dropped_and_retry_is_fresh(core: &mut Core, record: &Value, second: &Value) {
    let (pid, conversation) = ids(record);
    wait_gone(pid);
    assert_conversation_removed(&core.home, &conversation);
    assert_eq!(core.work_dirs_left(), 0);
    core.call("r9", "d9", second);
    let again = core.collect("r9", "d9");
    assert!(
        again.response["result"].get("error").is_none(),
        "{}",
        again.response
    );
    let records = core.records();
    assert_eq!(records.len(), 2, "重来照新开");
    let fresh = records.iter().find(|other| other["pid"] != pid).unwrap();
    assert_eq!(fresh["lines"].as_array().unwrap().len(), 1);
}

#[test]
fn an_error_in_the_next_turn_drops_the_conversation() {
    let mut core = Core::start("fail-second", "fail-second");
    let (first, said, record) = first_turn(&mut core, "hi");
    let second = next_turn(&first, &said, &["again"]);
    core.call("r2", "d2", &second);
    let error = core.collect("r2", "d2").response["result"]["error"].clone();
    assert_eq!(error["message"], "agy failed (ERROR): second turn broke");
    assert_dropped_and_retry_is_fresh(&mut core, &record, &second);
}

#[test]
fn cancel_in_the_next_turn_drops_the_conversation() {
    let mut core = Core::start("hang-second", "hang-second");
    let (first, said, record) = first_turn(&mut core, "hi");
    let second = next_turn(&first, &said, &["again"]);
    core.call("r2", "d2", &second);
    let hanging = core.wait_record("child");
    assert_eq!(hanging["pid"], record["pid"], "第二轮接在同一个进程上");
    core.send(&json!({"jsonrpc": "2.0", "method": "driver.cancel", "params": {"call": "d2"}}));
    let got = core.collect("r2", "d2");
    assert_eq!(
        got.response["result"]["error"]["message"],
        "the call was cancelled"
    );
    wait_gone(hanging["child"].as_i64().unwrap());
    assert_dropped_and_retry_is_fresh(&mut core, &record, &second);
}

#[test]
fn a_broken_stream_in_the_next_turn_drops_the_conversation() {
    let mut core = Core::start("die-second", "die-second");
    let (first, said, record) = first_turn(&mut core, "hi");
    let second = next_turn(&first, &said, &["again"]);
    core.call("r2", "d2", &second);
    let got = core.collect("r2", "d2");
    assert_eq!(got.text(), "partial");
    assert_eq!(got.response["result"]["error"]["class"], "unclassified");
    assert_dropped_and_retry_is_fresh(&mut core, &record, &second);
}

#[test]
fn a_kept_agy_that_dies_silently_is_replaced_without_an_error() {
    let mut core = Core::start("quit-second", "quit-second");
    let (first, said, record) = first_turn(&mut core, "hi");
    let second = next_turn(&first, &said, &["again"]);
    core.call("r2", "d2", &second);
    let got = core.collect("r2", "d2");
    assert!(
        got.response["result"].get("error").is_none(),
        "{}",
        got.response
    );
    let (pid, conversation) = ids(&record);
    assert_eq!(
        core.record_of(pid)["lines"].as_array().unwrap().len(),
        2,
        "先试了留着的那个"
    );
    wait_gone(pid);
    assert_conversation_removed(&core.home, &conversation);
    assert_eq!(core.records().len(), 2, "改走新开");
}

#[test]
fn an_idle_agy_that_exited_is_noticed_on_the_next_turn() {
    let mut core = Core::start("idle-exit", "idle-exit");
    let (first, said, record) = first_turn(&mut core, "hi");
    core.wait_record("exited");
    let second = next_turn(&first, &said, &["again"]);
    core.call("r2", "d2", &second);
    let got = core.collect("r2", "d2");
    assert!(
        got.response["result"].get("error").is_none(),
        "{}",
        got.response
    );
    let (pid, conversation) = ids(&record);
    wait_gone(pid);
    assert_conversation_removed(&core.home, &conversation);
    assert_eq!(core.records().len(), 2, "照新开");
}

#[test]
fn an_idle_agy_that_exited_is_cleaned_up_by_the_sweeper() {
    let mut core = Core::start_with("idle-sweep", "idle-exit", &[FAST_SWEEP], &json!({}));
    let (_, _, record) = first_turn(&mut core, "hi");
    let (pid, conversation) = ids(&record);
    // 没有下一轮：巡检发现它退了、收尸、删文件。
    wait_gone(pid);
    let deadline = Instant::now() + Duration::from_secs(10);
    while core.work_dirs_left() > 0 {
        assert!(Instant::now() < deadline, "工作目录没删");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_conversation_removed(&core.home, &conversation);
}

#[test]
fn an_idle_agy_is_stopped_after_keep_seconds() {
    let mut core = Core::start_with(
        "keep",
        "echo",
        &[FAST_SWEEP],
        &json!({"antigravity.keep_seconds": 1}),
    );
    let started = Instant::now();
    let (_, _, record) = first_turn(&mut core, "hi");
    let (pid, conversation) = ids(&record);
    assert!(alive(pid), "刚说完还留着");
    wait_group_gone(pid);
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "留够了 keep_seconds"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while core.work_dirs_left() > 0 {
        assert!(Instant::now() < deadline, "工作目录没删");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_conversation_removed(&core.home, &conversation);
}

#[test]
fn more_than_four_conversations_push_out_the_least_recently_used() {
    let mut core = Core::start("evict", "sample:real-pong.jsonl");
    let mut pids = Vec::new();
    let mut requests = Vec::new();
    for index in 0..5 {
        let first = request("s", &format!("conversation {index}"));
        let (id, call) = (format!("r{index}"), format!("d{index}"));
        core.call(&id, &call, &first);
        core.collect(&id, &call);
        let record = core
            .records()
            .into_iter()
            .find(|record| !pids.contains(&record["pid"].as_i64().unwrap()))
            .unwrap();
        pids.push(record["pid"].as_i64().unwrap());
        requests.push(first);
    }
    wait_group_gone(pids[0]);
    for pid in &pids[1..] {
        assert!(alive(*pid), "{pid} 留着");
    }
    // 第二段还接得上（没再起进程）。
    core.call("r9", "d9", &next_turn(&requests[1], "pong\n", &["more"]));
    assert_eq!(core.collect("r9", "d9").text(), "pong\n");
    assert_eq!(core.records().len(), 5);
    assert_eq!(
        core.record_of(pids[1])["lines"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn quitting_stops_every_kept_agy() {
    let mut core = Core::start("quit-all", "sample:real-pong.jsonl");
    for index in 0..3 {
        let (id, call) = (format!("r{index}"), format!("d{index}"));
        core.call(&id, &call, &request("s", &format!("conversation {index}")));
        core.collect(&id, &call);
    }
    let records = core.records();
    assert_eq!(records.len(), 3);
    assert!(core.finish().success());
    for record in &records {
        let (pid, conversation) = ids(record);
        wait_group_gone(pid);
        assert_conversation_removed(&core.home, &conversation);
    }
    assert_eq!(core.work_dirs_left(), 0);
}

/// 把 `home` 下代理 `name` 的 `agent.md` 写好、修改时刻设成 `at`。
fn plant_agent(home: &Path, name: &str, at: SystemTime) {
    let file = home
        .join(".gemini/config/agents")
        .join(name)
        .join("agent.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

#[test]
fn old_miyu_agents_are_swept_when_an_agent_is_written() {
    let mut core = Core::start("agents", "sample:real-pong.jsonl");
    let old = SystemTime::now() - Duration::from_hours(2);
    // 一段活着的对话，它的代理也很久没改过。
    core.call("r1", "d1", &request("kept system", "hi"));
    core.collect("r1", "d1");
    let agents = core.home.join(".gemini/config/agents");
    let in_use: Vec<String> = std::fs::read_dir(&agents)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(in_use.len(), 1);
    let in_use = in_use[0].clone();
    plant_agent(&core.home, &in_use, old);
    plant_agent(&core.home, "miyu-0000000000000000", old);
    plant_agent(&core.home, "miyu-1111111111111111", SystemTime::now());
    plant_agent(&core.home, "someone-else", old);

    core.call("r2", "d2", &request("another system", "hi"));
    core.collect("r2", "d2");
    let mut left: Vec<String> = std::fs::read_dir(&agents)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert!(
        !left.contains(&"miyu-0000000000000000".to_string()),
        "旧的删了：{left:?}"
    );
    for name in [in_use.as_str(), "miyu-1111111111111111", "someone-else"] {
        assert!(
            left.iter().any(|left| left == name),
            "{name} 留着：{left:?}"
        );
    }
    assert_eq!(left.len(), 4, "加上新写的那个：{left:?}");
}
