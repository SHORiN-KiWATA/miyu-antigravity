//! 扩展被 `SIGKILL` 以后留下的东西，下一个扩展启动时清掉（设计第八节「清理」）：套接字文件、工作目录、agy 那边的会话文件
//! （靠 `kept/<编号>` 的记号知道是哪段）。真 `serve` + 假 agy：假 agy 标准输入一关就退，和真的一样，所以硬杀以后它自己没了、
//! 文件留着。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::support::{Core, WAIT, assert_conversation_removed, request, wait_gone};

/// agy 那边一段对话的文件：`conversations/<编号>.db*` 和 `brain/<编号>/`。
pub fn conversation_files(home: &Path, conversation: &str) -> Vec<PathBuf> {
    let base = home.join(".gemini/antigravity-cli");
    let mut files: Vec<PathBuf> = [".db", ".db-shm", ".db-wal"]
        .iter()
        .map(|suffix| {
            base.join("conversations")
                .join(format!("{conversation}{suffix}"))
        })
        .collect();
    files.push(base.join("brain").join(conversation));
    files
}

/// 假造一段对话的文件（假 agy 自己建的那种）。
fn plant_conversation(home: &Path, conversation: &str) {
    let files = conversation_files(home, conversation);
    let (databases, brain) = files.split_at(3);
    for file in databases {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "x").unwrap();
    }
    std::fs::create_dir_all(&brain[0]).unwrap();
}

/// 工作目录的父目录里名字以 `<pid>-` 开头的目录。
fn work_dirs_of(state: &Path, pid: u32) -> Vec<PathBuf> {
    std::fs::read_dir(state.join("work"))
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(&format!("{pid}-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 等到 `path` 有了。
fn wait_exists(path: &Path) {
    let deadline = Instant::now() + WAIT;
    while !path.exists() {
        assert!(Instant::now() < deadline, "{} 一直没有", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 等到 `path` 没了。
fn wait_missing(path: &Path) {
    let deadline = Instant::now() + WAIT;
    while path.exists() {
        assert!(Instant::now() < deadline, "{} 一直还在", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_killed_extensions_leftovers_are_swept_by_the_next_one() {
    let mut core = Core::start("left-killed", "echo");
    core.call("r1", "d1", &request("s", "hi"));
    core.collect("r1", "d1");
    let record = core.records().into_iter().next().unwrap();
    let (agy, conversation) = (
        record["pid"].as_i64().unwrap(),
        record["conversation"].as_str().unwrap().to_string(),
    );
    let dead = core.pid();
    let socket = core.runtime.join(format!("antigravity-{dead}.sock"));
    let mark = core.state.join("kept").join(&conversation);
    assert_eq!(std::fs::read_to_string(&mark).unwrap(), format!("{dead}\n"));

    core.kill_hard();
    wait_gone(agy); // 标准输入关了，假 agy 自己退。
    let files = conversation_files(&core.home, &conversation);
    assert!(socket.exists(), "硬杀来不及删套接字");
    assert_eq!(work_dirs_of(&core.state, dead).len(), 1, "工作目录留着");
    assert!(mark.exists());
    for file in &files {
        assert!(file.exists(), "{} 留着", file.display());
    }

    // 同一套目录再起一个：过了启动（能答 driver.models）就都清了。
    let mut next = core.restart();
    next.wait_started();
    assert!(!socket.exists(), "套接字");
    assert!(work_dirs_of(&next.state, dead).is_empty(), "工作目录");
    assert!(!mark.exists(), "记号");
    for file in &files {
        assert!(!file.exists(), "{} 没了", file.display());
    }
    assert!(
        next.runtime
            .join(format!("antigravity-{}.sock", next.pid()))
            .exists(),
        "新的自己的套接字在"
    );
}

#[test]
fn leftovers_that_belong_to_a_living_process_or_have_other_names_stay() {
    let mut core = Core::spawn("left-alive", "echo");
    // 测试进程自己活着：它的进程号当成「还在」的另一个进程。
    let alive = std::process::id();
    let socket = core.runtime.join(format!("antigravity-{alive}.sock"));
    let odd_socket = core.runtime.join("antigravity-notapid.sock");
    let work = core.state.join("work").join(format!("{alive}-1-1"));
    let odd_work = core.state.join("work/plain");
    let kept = core.state.join("kept");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&odd_work).unwrap();
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::write(&socket, "").unwrap();
    std::fs::write(&odd_socket, "").unwrap();
    std::fs::write(kept.join("alive-conv-1"), format!("{alive}\n")).unwrap();
    std::fs::write(kept.join(".half-written.tmp"), "").unwrap();
    plant_conversation(&core.home, "alive-conv-1");

    core.answer_hello(
        &json!({"protocol": 1, "language": "en", "config": {"antigravity.binary": core.binary.to_str().unwrap()}}),
        None,
    );
    core.wait_started();
    for path in [
        &socket,
        &odd_socket,
        &work,
        &odd_work,
        &kept.join("alive-conv-1"),
        &kept.join(".half-written.tmp"),
    ] {
        assert!(path.exists(), "{} 留着", path.display());
    }
    for file in conversation_files(&core.home, "alive-conv-1") {
        assert!(file.exists(), "{} 留着", file.display());
    }
}

#[test]
fn a_clean_exit_leaves_no_mark_behind() {
    let mut core = Core::start("left-clean", "echo");
    core.call("r1", "d1", &request("s", "hi"));
    core.collect("r1", "d1");
    let record = core.records().into_iter().next().unwrap();
    let conversation = record["conversation"].as_str().unwrap();
    let mark = core.state.join("kept").join(conversation);
    assert!(mark.exists(), "说完留着的那段有记号");
    assert!(core.finish().success());
    assert_conversation_removed(&core.home, conversation);
    assert!(!mark.exists(), "会话文件删了、记号也删");
}

#[test]
fn the_mark_is_there_as_soon_as_agy_says_its_conversation_and_goes_with_it() {
    let mut core = Core::start("left-early", "hang");
    core.call("r1", "d1", &request("s", "hi"));
    let record = core.wait_record("child");
    let conversation = record["conversation"].as_str().unwrap();
    // 这一轮还没完：记号已经写了。
    let mark = core.state.join("kept").join(conversation);
    wait_exists(&mark);
    assert_eq!(
        std::fs::read_to_string(&mark).unwrap(),
        format!("{}\n", core.pid())
    );
    core.send(&json!({"jsonrpc": "2.0", "method": "driver.cancel", "params": {"call": "d1"}}));
    core.collect("r1", "d1");
    assert_conversation_removed(&core.home, conversation);
    wait_missing(&mark);
}
