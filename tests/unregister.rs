//! 集成测试：`miyu-antigravity unregister`，`HOME` 指到临时目录，真的 `~/.gemini` 碰不到。

#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

fn home(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "unregister-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_miyu-antigravity"))
        .arg("unregister")
        .env("HOME", home)
        .output()
        .unwrap()
}

fn config_file(home: &Path) -> PathBuf {
    home.join(".gemini/config/mcp_config.json")
}

fn write_config(home: &Path, text: &str) {
    let file = config_file(home);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

fn make_agent(home: &Path, name: &str) {
    let dir = home.join(".gemini/config/agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("agent.md"), "x").unwrap();
}

#[test]
fn removes_the_entry_and_only_miyu_agents() {
    let home = home("ok");
    write_config(
        &home,
        r#"{"keep": 1, "mcpServers": {"other": {"command": "o"}, "miyu": {"command": "m"}}}"#,
    );
    for name in [
        "miyu-0011223344556677",
        "miyu-old",
        "mine",
        "not-miyu-x",
        "Miyu-up",
    ] {
        make_agent(&home, name);
    }
    std::fs::write(home.join(".gemini/config/agents/miyu-file"), "a file").unwrap();

    let output = run(&home);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("removed the \"miyu\" entry"), "{stdout}");
    assert!(
        stdout.contains("removed 2 miyu-* agent directories"),
        "{stdout}"
    );

    let value: Value =
        serde_json::from_str(&std::fs::read_to_string(config_file(&home)).unwrap()).unwrap();
    assert_eq!(
        value,
        json!({"keep": 1, "mcpServers": {"other": {"command": "o"}}})
    );
    let mut left: Vec<String> = std::fs::read_dir(home.join(".gemini/config/agents"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["Miyu-up", "mine", "miyu-file", "not-miyu-x"]);

    // 再来一遍：本来就没有，照样成功。
    let again = run(&home);
    assert_eq!(again.status.code(), Some(0));
    assert!(
        String::from_utf8(again.stdout)
            .unwrap()
            .contains("nothing to remove")
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn nothing_there_is_a_success_and_creates_nothing() {
    let home = home("empty");
    let output = run(&home);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(!home.join(".gemini").exists(), "什么都不建");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn an_unreadable_config_exits_one_and_stays_as_it_was_but_agents_are_still_cleaned() {
    let home = home("bad");
    write_config(&home, "{ this is not json");
    make_agent(&home, "miyu-aaaa");
    make_agent(&home, "keepme");
    let output = run(&home);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&config_file(&home).display().to_string()),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(config_file(&home)).unwrap(),
        "{ this is not json"
    );
    assert!(!home.join(".gemini/config/agents/miyu-aaaa").exists());
    assert!(home.join(".gemini/config/agents/keepme").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_config_whose_servers_is_not_an_object_is_left_alone() {
    let home = home("servers");
    write_config(&home, r#"{"mcpServers": "oops"}"#);
    let output = run(&home);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(config_file(&home)).unwrap(),
        r#"{"mcpServers": "oops"}"#
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn without_home_it_exits_one() {
    let output = Command::new(env!("CARGO_BIN_EXE_miyu-antigravity"))
        .arg("unregister")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}
