//! `mcp` 把自己环境里的 `HOME`（agy 实际用的家目录）随每次 `tools/list`、`tools/call` 报给扩展（AG-6）。

use serde_json::json;

use crate::support::Rig;

#[test]
fn list_and_call_carry_the_home_of_the_tool_server() {
    let rig = Rig::new("m-home");
    let mut agy = rig.agy_with_home("/fake/agy-home");
    agy.ask(1, "tools/list", &json!({}));
    assert_eq!(
        *rig.homes.lock().unwrap(),
        [std::path::PathBuf::from("/fake/agy-home")]
    );
    agy.request(2, "tools/call", &json!({"name": "bash", "arguments": {}}));
    let call = rig.next_call();
    assert_eq!(rig.homes.lock().unwrap().len(), 2, "call 也带");
    call.reply.send(&[], false).unwrap();
    assert_eq!(agy.recv()["id"], 2);
}

#[test]
fn an_empty_or_relative_home_is_not_reported_and_the_tools_still_come() {
    for (name, home) in [("m-home-empty", ""), ("m-home-rel", "relative/dir")] {
        let rig = Rig::new(name);
        let mut agy = rig.agy_with_home(home);
        let reply = agy.ask(1, "tools/list", &json!({}));
        assert_eq!(reply["result"]["tools"][0]["name"], "bash");
        assert!(rig.homes.lock().unwrap().is_empty(), "{home:?}");
    }
}

/// 老的 MCP 服务不带这一格：直接对套接字发一条不带 `home` 的 `list`，照常回工具表，也没人报家目录。
#[test]
fn an_old_tool_server_that_sends_no_home_gets_the_table() {
    use std::io::{BufRead, BufReader, Write};
    let rig = Rig::new("m-home-old");
    let mut stream = std::os::unix::net::UnixStream::connect(&rig.path).unwrap();
    writeln!(
        stream,
        "{}",
        json!({"token": crate::support::TOKEN, "op": "list"})
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["tools"][0]["name"], "bash");
    assert!(rig.homes.lock().unwrap().is_empty());
}
