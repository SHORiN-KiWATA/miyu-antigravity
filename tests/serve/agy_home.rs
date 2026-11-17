//! 包装脚本给 agy 换了 `HOME` 的情况（AG-6）：agy 的对话数据落在假家目录下，agy 拉起的工具服务的 `HOME` 就是那个目录，经套接字
//! 报给扩展，扩展读转录、删会话文件、启动清扫都照它。假 agy 用环境变量 `FAKE_AGY_HOME` 扮这个（`tests/fake_agy.py` 开头的说明）：
//! 它的 MCP 服务子进程的 `HOME`、它写的会话文件和转录都在那个目录，配置和代理文件仍从原来的 `HOME` 读。

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::leftovers::conversation_files;
use crate::support::{Collected, Core, assert_conversation_removed, request, scratch, wait_gone};
use crate::tool_rig::tool_request;

/// 假 agy 剧本里 `search_web` 在转录里写的字（去掉两行时间以后）。
const FOUND: &str = "The search for \"EditorConfig specification\" returned the following summary:\nA simple file format [1].";

/// 假家目录：丢掉时删。
struct FakeHome(PathBuf);

impl FakeHome {
    fn new(name: &str) -> FakeHome {
        FakeHome(scratch(name))
    }

    fn text(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for FakeHome {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// 起扩展（假 agy 照 `plan:search` 演、家目录换成 `fake`，`None` 不换）、带工具搜一次；交扩展、收到的、假 agy 的进程号和对话编号。
fn search(name: &str, fake: Option<&FakeHome>) -> (Core, Collected, i64, String) {
    let env: Vec<(&str, &str)> = fake
        .map(|fake| ("FAKE_AGY_HOME", fake.text()))
        .into_iter()
        .collect();
    let mut core = Core::start_with(name, "plan:search", &env, &json!({}));
    core.call("r1", "d1", &tool_request("s", "search for it"));
    let got = core.collect("r1", "d1");
    let record = core.records().into_iter().next().unwrap();
    let (agy, conversation) = (
        record["pid"].as_i64().unwrap(),
        record["conversation"].as_str().unwrap().to_string(),
    );
    (core, got, agy, conversation)
}

/// 扩展自己的 `HOME` 下没有 agy 的对话数据（`.gemini/antigravity-cli` 不存在）。
fn assert_no_agy_data_under(home: &Path) {
    let root = home.join(".gemini/antigravity-cli");
    assert!(!root.exists(), "{} 不该有", root.display());
}

/// 1. 换了家目录的假 agy 搜索一次：`hosted_tool` 块的 `outcome` 是假家目录下那份转录里的字。
#[test]
fn a_search_reads_the_transcript_under_the_home_the_tool_server_reported() {
    let fake = FakeHome::new("ah-search");
    let (core, got, _, _) = search("ah-search-core", Some(&fake));
    assert!(
        got.response["result"].get("error").is_none(),
        "{}",
        got.response
    );
    assert_eq!(
        got.deltas[..4],
        [
            json!({"start": {"index": 0, "kind": {"hosted_tool": {"name": "search_web"}}}}),
            json!({"text": {"index": 0, "text": r#"{"query":"EditorConfig specification"}"#}}),
            json!({"outcome": {"index": 0, "text": FOUND}}),
            json!({"end": {"index": 0}}),
        ]
    );
    assert_no_agy_data_under(&core.home);
}

/// 2. 同一种情况下对话收掉：假家目录下的会话文件没了；扩展自己的 `HOME` 下本来就没有。
#[test]
fn closing_the_extension_removes_the_conversation_under_the_reported_home() {
    let fake = FakeHome::new("ah-close");
    let (mut core, _, agy, conversation) = search("ah-close-core", Some(&fake));
    let files = conversation_files(&fake.0, &conversation);
    assert!(files.iter().all(|file| file.exists()), "收之前在");
    assert_no_agy_data_under(&core.home);
    assert!(core.finish().success());
    wait_gone(agy);
    assert_conversation_removed(&fake.0, &conversation);
    assert_no_agy_data_under(&core.home);
    assert!(
        !core.state.join("kept").join(&conversation).exists(),
        "删干净了记号也删"
    );
}

/// 3. 扩展被 `SIGKILL`、再起一个新的扩展：开头的清扫照记号把假家目录下那段对话删了。
#[test]
fn a_killed_extensions_conversation_under_the_reported_home_is_swept_by_the_next_one() {
    let fake = FakeHome::new("ah-kill");
    let (mut core, _, agy, conversation) = search("ah-kill-core", Some(&fake));
    let dead = core.pid();
    let mark = core.state.join("kept").join(&conversation);
    assert_eq!(
        std::fs::read_to_string(&mark).unwrap(),
        format!("{dead}\n{}\n", fake.0.display()),
        "记号第二行是假家目录"
    );

    core.kill_hard();
    wait_gone(agy);
    let files = conversation_files(&fake.0, &conversation);
    assert!(files.iter().all(|file| file.exists()), "硬杀以后留着");

    let mut next = core.restart();
    next.wait_started();
    assert!(!mark.exists(), "记号");
    for file in &files {
        assert!(!file.exists(), "{} 没了", file.display());
    }
    assert_no_agy_data_under(&next.home);
}

/// 4. 工具服务的 `HOME` 就是扩展自己的：什么都不变（会话文件在扩展自己的家目录下、记号只有进程号一行）。
#[test]
fn a_tool_server_with_the_extensions_own_home_changes_nothing() {
    let (mut core, got, agy, conversation) = search("ah-same", None);
    assert_eq!(
        got.deltas[2],
        json!({"outcome": {"index": 0, "text": FOUND}})
    );
    let mark = core.state.join("kept").join(&conversation);
    assert_eq!(
        std::fs::read_to_string(&mark).unwrap(),
        format!("{}\n", core.pid())
    );
    assert!(core.finish().success());
    wait_gone(agy);
    assert_conversation_removed(&core.home, &conversation);
}

/// 4. 请求不带工具、`mcp_config.json` 里也没有 `miyu` 条目：agy 不拉工具服务，没人报，照旧用扩展自己的 `HOME`。
#[test]
fn without_tools_and_without_a_miyu_entry_nothing_is_reported_and_the_own_home_is_used() {
    let mut core = Core::start_with("ah-notools-core", "echo", &[], &json!({}));
    core.call("r1", "d1", &request("s", "hi"));
    let got = core.collect("r1", "d1");
    assert!(
        got.response["result"].get("error").is_none(),
        "{}",
        got.response
    );
    let record = core.records().into_iter().next().unwrap();
    let conversation = record["conversation"].as_str().unwrap().to_string();
    assert!(record.get("mcp_pid").is_none());
    assert!(core.finish().success());
    assert_conversation_removed(&core.home, &conversation);
}

/// 把 `miyu` 条目事先写进 `mcp_config.json`（以前哪次带工具的调用登记过的样子）；`others` 是同一个文件里别的条目。
fn seed_miyu_entry(core: &Core, others: bool) -> PathBuf {
    let file = core.home.join(".gemini/config/mcp_config.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut servers =
        json!({"miyu": {"command": env!("CARGO_BIN_EXE_miyu-antigravity"), "args": ["mcp"]}});
    if others {
        servers["other"] = json!({"command": "true", "args": []});
    }
    std::fs::write(&file, json!({"mcpServers": servers}).to_string()).unwrap();
    file
}

/// a. 换了家目录的假 agy、文件里事先有 `miyu` 条目、请求不带工具：工具服务照样被拉起、来 `list` 报了家目录——记号有第二行，
/// 关扩展以后假家目录下的会话文件没了；文件没被改。
#[test]
fn a_call_without_tools_still_learns_the_home_when_the_miyu_entry_is_already_registered() {
    let fake = FakeHome::new("ah-nt-a");
    let mut core = Core::start_with(
        "ah-nt-a-core",
        "echo",
        &[("FAKE_AGY_HOME", fake.text())],
        &json!({}),
    );
    let file = seed_miyu_entry(&core, false);
    let before = std::fs::read_to_string(&file).unwrap();
    core.call("r1", "d1", &request("s", "hi"));
    let got = core.collect("r1", "d1");
    assert!(
        got.response["result"].get("error").is_none(),
        "{}",
        got.response
    );
    let record = core.records().into_iter().next().unwrap();
    let (agy, conversation) = (
        record["pid"].as_i64().unwrap(),
        record["conversation"].as_str().unwrap().to_string(),
    );
    assert!(record.get("mcp_pid").is_some(), "工具服务被拉起了");
    assert_eq!(
        std::fs::read_to_string(core.state.join("kept").join(&conversation)).unwrap(),
        format!("{}\n{}\n", core.pid(), fake.0.display())
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before, "文件原样");
    assert!(core.finish().success());
    wait_gone(agy);
    assert_conversation_removed(&fake.0, &conversation);
    assert_no_agy_data_under(&core.home);
}

/// b. 同上但没人拉工具服务（文件不存在、或者文件里没有 `miyu` 条目）：没人报，记号一行，文件没被建出来、也没被改。
#[test]
fn a_call_without_tools_and_without_a_miyu_entry_reports_nothing_and_writes_no_file() {
    for (name, other_only) in [("ah-nt-b1", false), ("ah-nt-b2", true)] {
        let fake = FakeHome::new(name);
        let mut core = Core::start_with(
            &format!("{name}-core"),
            "echo",
            &[("FAKE_AGY_HOME", fake.text())],
            &json!({}),
        );
        let file = core.home.join(".gemini/config/mcp_config.json");
        let before = other_only.then(|| {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            let text =
                json!({"mcpServers": {"other": {"command": "true", "args": []}}}).to_string();
            std::fs::write(&file, &text).unwrap();
            text
        });
        core.call("r1", "d1", &request("s", "hi"));
        let got = core.collect("r1", "d1");
        assert!(
            got.response["result"].get("error").is_none(),
            "{}",
            got.response
        );
        let record = core.records().into_iter().next().unwrap();
        let conversation = record["conversation"].as_str().unwrap();
        assert!(record.get("mcp_pid").is_none(), "没人拉工具服务");
        assert_eq!(
            record["bridge_env"],
            json!(["MIYU_ANTIGRAVITY_BRIDGE", "MIYU_ANTIGRAVITY_TOKEN"])
        );
        assert_eq!(
            std::fs::read_to_string(core.state.join("kept").join(conversation)).unwrap(),
            format!("{}\n", core.pid()),
            "记号一行"
        );
        match before {
            Some(text) => assert_eq!(std::fs::read_to_string(&file).unwrap(), text),
            None => assert!(!file.exists(), "文件没被建出来"),
        }
    }
}

/// c. 不带工具的请求里假 agy 调了 `x`（全局配置里别的对话登记的工具）：工具服务拿到出错的结果（`isError`），扩展不崩，这一轮
/// 照常结束。
#[test]
fn a_tool_call_on_a_conversation_without_tools_is_answered_as_an_error_and_the_turn_goes_on() {
    let mut core = Core::start("ah-nt-c", "plan:call-unlisted");
    seed_miyu_entry(&core, false);
    core.call("r1", "d1", &request("s", "hi"));
    let got = core.collect("r1", "d1");
    assert!(
        got.response["result"].get("error").is_none(),
        "{}",
        got.response
    );
    assert_eq!(got.text(), "Let me look. done");
    assert!(
        got.deltas
            .iter()
            .all(|delta| delta.pointer("/start/kind/tool_call").is_none()),
        "不交回工具调用"
    );
    let record = core.records().into_iter().next().unwrap();
    assert_eq!(
        record["calls"],
        json!([{"name": "x", "arguments": {"a": 1}}])
    );
    assert_eq!(record["replies"][0]["isError"], true, "{record}");
    assert!(
        record["replies"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not available"),
        "{record}"
    );
    core.call("r2", "d2", &request("s2", "again"));
    core.collect("r2", "d2");
    assert!(core.finish().success(), "扩展没崩");
}
