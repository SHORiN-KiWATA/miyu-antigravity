use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcp-config-{name}-{}-{}",
        std::process::id(),
        workspace::unique()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn entry(tools: &[&str]) -> Registration {
    Registration {
        program: PathBuf::from("/opt/miyu/miyu-antigravity"),
        timeout_seconds: 3660,
        tools: tools.iter().map(ToString::to_string).collect(),
    }
}

fn read(file: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

#[test]
fn a_missing_file_is_created_with_the_entry() {
    let dir = scratch("create");
    let file = path(&dir);
    assert!(register(&file, &entry(&["bash", "read"])).unwrap());
    assert_eq!(
        read(&file),
        json!({"mcpServers": {"miyu": {
            "command": "/opt/miyu/miyu-antigravity",
            "args": ["mcp"],
            "timeoutSeconds": 3660,
            "tools": {"bash": {"eager": true}, "read": {"eager": true}}
        }}})
    );
    assert!(
        !std::fs::read_to_string(&file).unwrap().contains("\"env\""),
        "不写 env"
    );
    let left: Vec<_> = std::fs::read_dir(file.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left, ["mcp_config.json"], "暂存文件改名后没有留下");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn other_servers_other_top_level_keys_and_key_order_are_kept() {
    let old = r#"{
  "zeta": {"keep": [1, 2, 3]},
  "mcpServers": {
    "other": {"command": "x", "args": ["a"], "env": {"K": "V"}, "extra": null},
    "miyu": {"command": "/old/miyu", "args": ["mcp"], "env": {"GUARD": "1"}, "tools": {"old_tool": {"eager": false}}},
    "last": {"url": "http://localhost:1"}
  },
  "alpha": true
}"#;
    let new = register_text(Some(old), &entry(&["new_tool", "old_tool"]))
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&new).unwrap();
    let top: Vec<_> = value.as_object().unwrap().keys().collect();
    assert_eq!(top, ["zeta", "mcpServers", "alpha"]);
    let servers: Vec<_> = value["mcpServers"].as_object().unwrap().keys().collect();
    assert_eq!(servers, ["other", "miyu", "last"], "miyu 留在原来的位置");
    assert_eq!(value["zeta"], json!({"keep": [1, 2, 3]}));
    assert_eq!(value["alpha"], json!(true));
    let before: Value = serde_json::from_str(old).unwrap();
    assert_eq!(value["mcpServers"]["other"], before["mcpServers"]["other"]);
    assert_eq!(value["mcpServers"]["last"], before["mcpServers"]["last"]);
    let miyu = &value["mcpServers"]["miyu"];
    assert_eq!(miyu["command"], "/opt/miyu/miyu-antigravity");
    assert_eq!(miyu["args"], json!(["mcp"]));
    assert_eq!(miyu["timeoutSeconds"], 3660);
    assert!(miyu.get("env").is_none(), "旧条目的 env 被接管掉");
    let tools: Vec<_> = miyu["tools"].as_object().unwrap().keys().collect();
    assert_eq!(tools, ["old_tool", "new_tool"], "旧的在前、新的并上");
    assert_eq!(miyu["tools"]["old_tool"], json!({"eager": true}));
}

#[test]
fn tools_only_ever_grow() {
    let first = register_text(None, &entry(&["a", "b"])).unwrap().unwrap();
    let second = register_text(Some(&first), &entry(&["c"]))
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&second).unwrap();
    let tools: Vec<_> = value["mcpServers"]["miyu"]["tools"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(tools, ["a", "b", "c"]);
}

#[test]
fn a_file_without_mcp_servers_gets_one_and_keeps_the_rest() {
    let new = register_text(Some(r#"{"theme": "dark"}"#), &entry(&["t"]))
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&new).unwrap();
    assert_eq!(value["theme"], "dark");
    assert_eq!(
        value["mcpServers"]["miyu"]["tools"],
        json!({"t": {"eager": true}})
    );
}

#[test]
fn an_unchanged_registration_does_not_touch_the_file() {
    let dir = scratch("unchanged");
    let file = path(&dir);
    assert!(register(&file, &entry(&["a"])).unwrap());
    let before = std::fs::metadata(&file).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    assert!(!register(&file, &entry(&["a"])).unwrap(), "没变不写");
    assert!(!register(&file, &entry(&[])).unwrap(), "名单没增也不写");
    let after = std::fs::metadata(&file).unwrap();
    assert_eq!(before.ino(), after.ino(), "没有改名顶替");
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert!(register(&file, &entry(&["b"])).unwrap(), "多了名字就写");
    assert_eq!(
        register_text(
            Some(&std::fs::read_to_string(&file).unwrap()),
            &entry(&["b"])
        ),
        Ok(None)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_file_that_cannot_be_understood_is_never_overwritten() {
    let dir = scratch("bad");
    for (name, content, reason) in [
        ("text", "this is not json", "not valid JSON"),
        ("array", "[1, 2]", "not a JSON object"),
        ("string", "\"hi\"", "not a JSON object"),
        (
            "servers_string",
            r#"{"mcpServers": "oops"}"#,
            "\"mcpServers\" is not a JSON object",
        ),
        ("empty", "", "not valid JSON"),
    ] {
        let file = dir.join(format!("{name}.json"));
        std::fs::write(&file, content).unwrap();
        for result in [register(&file, &entry(&["a"])), unregister(&file)] {
            let error = result.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{name}");
            let message = error.to_string();
            assert!(message.contains(&file.display().to_string()), "{message}");
            assert!(message.contains(reason), "{message}");
        }
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            content,
            "{name} 原样"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unregister_removes_only_the_miyu_entry() {
    let dir = scratch("remove");
    let file = path(&dir);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        r#"{"b": 1, "mcpServers": {"x": {"command": "x"}, "miyu": {"command": "m"}, "y": {"command": "y"}}, "a": 2}"#,
    )
    .unwrap();
    assert!(unregister(&file).unwrap());
    let value = read(&file);
    assert_eq!(
        value,
        json!({"b": 1, "mcpServers": {"x": {"command": "x"}, "y": {"command": "y"}}, "a": 2})
    );
    let servers: Vec<_> = value["mcpServers"].as_object().unwrap().keys().collect();
    assert_eq!(servers, ["x", "y"]);
    assert!(!unregister(&file).unwrap(), "本来就没有，不写");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unregister_of_a_missing_file_or_a_file_without_servers_writes_nothing() {
    let dir = scratch("remove-missing");
    let file = path(&dir);
    assert!(!unregister(&file).unwrap());
    assert!(!file.exists(), "没有的文件不建");
    assert_eq!(unregister_text(Some("{}")), Ok(None));
    assert_eq!(unregister_text(Some(r#"{"mcpServers": {}}"#)), Ok(None));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_registrations_lose_no_update() {
    let dir = scratch("concurrent");
    let file = path(&dir);
    let threads: Vec<_> = (0..16)
        .map(|n| {
            let file = file.clone();
            std::thread::spawn(move || {
                register(&file, &entry(&[&format!("tool_{n}")])).unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let value = read(&file);
    let tools = value["mcpServers"]["miyu"]["tools"].as_object().unwrap();
    assert_eq!(tools.len(), 16, "{tools:?}");
    for n in 0..16 {
        assert!(tools.contains_key(&format!("tool_{n}")));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_existing_file_mode_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("mode");
    let file = path(&dir);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(register(&file, &entry(&["a"])).unwrap());
    assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
    std::fs::remove_dir_all(dir).unwrap();
}
