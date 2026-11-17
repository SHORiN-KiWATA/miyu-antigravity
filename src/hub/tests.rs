use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::bridge::client::exchange;

const WAIT: Duration = Duration::from_secs(30);

/// 总台加真的套接字服务端（扮 MCP 服务的客户端用 [`exchange`] 连）。临时目录用短路径：套接字路径有长度上限。
struct Rig {
    hub: Arc<Hub>,
    server: Option<Server>,
    dir: PathBuf,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!("ag-hub-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hub = Arc::new(Hub::default());
        let server = Server::start(&dir.join("a.sock"), Arc::clone(&hub)).unwrap();
        Rig {
            hub,
            server: Some(server),
            dir,
        }
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("a.sock")
    }

    fn register(&self) -> Wiring {
        self.hub
            .register(vec![tool("bash")], &self.socket())
            .unwrap()
    }

    /// 在另一个线程里发一次 `call`，交线程（它的结果是 [`exchange`] 的）。
    fn call(
        &self,
        token: &str,
        name: &str,
        arguments: &Value,
    ) -> JoinHandle<Result<Value, String>> {
        let (socket, request) = (self.socket(), bridge::call_request(token, name, arguments));
        std::thread::spawn(move || exchange(&socket, &request))
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server = None;
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn tool(name: &str) -> Tool {
    Tool {
        name: name.into(),
        description: format!("does {name}"),
        input_schema: json!({"type": "object"}),
    }
}

/// 把 `wiring` 的路由指到一条新的通道。
fn listen(wiring: &Wiring) -> Receiver<Signal> {
    let (sender, receiver) = channel();
    *wiring.route().lock().unwrap() = Some(sender);
    receiver
}

fn next_call(receiver: &Receiver<Signal>) -> McpCall {
    match receiver.recv_timeout(WAIT).unwrap() {
        Signal::ToolCall(call) => call,
        other => panic!("想要一件工具调用，来的是 {other:?}"),
    }
}

#[test]
fn list_gives_the_registered_table_and_unknown_tokens_get_nothing() {
    let rig = Rig::new("list");
    let wiring = rig.register();
    assert_eq!(rig.hub.list(wiring.token()), Some(vec![tool("bash")]));
    assert_eq!(rig.hub.list("nope"), None);
    let reply = exchange(&rig.socket(), &bridge::list_request(wiring.token())).unwrap();
    assert_eq!(reply["tools"][0]["name"], "bash");
    let error = exchange(&rig.socket(), &bridge::list_request("nope")).unwrap_err();
    assert!(error.contains("unknown token"), "{error}");
}

#[test]
fn tokens_are_distinct_and_the_socket_path_is_kept() {
    let rig = Rig::new("tokens");
    let (one, two) = (rig.register(), rig.register());
    assert_ne!(one.token(), two.token());
    assert_eq!(one.socket(), rig.socket());
    assert_eq!(rig.hub.len(), 2);
}

#[test]
fn a_call_goes_to_the_listening_route_numbered_from_one_and_the_reply_comes_back() {
    let rig = Rig::new("call");
    let wiring = rig.register();
    let receiver = listen(&wiring);

    let first = rig.call(wiring.token(), "bash", &json!({"command": "ls", "n": 1}));
    let call = next_call(&receiver);
    assert_eq!((call.n, call.name.as_str()), (1, "bash"));
    assert_eq!(call.arguments, json!({"command": "ls", "n": 1}));
    call.reply
        .send(&[json!({"type": "text", "text": "one"})], false)
        .unwrap();
    assert_eq!(
        first.join().unwrap().unwrap(),
        json!({"content": [{"type": "text", "text": "one"}], "isError": false})
    );

    let second = rig.call(wiring.token(), "read", &json!({}));
    let call = next_call(&receiver);
    assert_eq!((call.n, call.name.as_str()), (2, "read"));
    call.reply.send(&[], true).unwrap();
    assert_eq!(second.join().unwrap().unwrap()["isError"], true);
    assert!(!wiring.stray().load(Ordering::SeqCst));
}

#[test]
fn a_call_with_nobody_listening_marks_the_process_stray_and_closes_the_connection() {
    let rig = Rig::new("stray");
    let wiring = rig.register();
    let result = rig.call(wiring.token(), "bash", &json!({})).join().unwrap();
    assert!(result.unwrap_err().contains("closed the connection"));
    assert!(wiring.stray().load(Ordering::SeqCst));

    // 听的那一头已经没了（接收端丢了、路由还指着）：同样算乱说话。
    let other = rig.register();
    drop(listen(&other));
    let result = rig.call(other.token(), "bash", &json!({})).join().unwrap();
    assert!(result.is_err());
    assert!(other.stray().load(Ordering::SeqCst));
}

#[test]
fn a_call_with_an_unknown_token_is_refused() {
    let rig = Rig::new("refuse");
    let wiring = rig.register();
    let receiver = listen(&wiring);
    let error = rig
        .call("wrong", "bash", &json!({}))
        .join()
        .unwrap()
        .unwrap_err();
    assert!(error.contains("unknown token"), "{error}");
    assert!(receiver.try_recv().is_err(), "没有送给别人的路由");
}

#[test]
fn numbers_are_per_process() {
    let rig = Rig::new("numbers");
    let (one, two) = (rig.register(), rig.register());
    let (first, second) = (listen(&one), listen(&two));
    let calls = [
        rig.call(one.token(), "bash", &json!({})),
        rig.call(two.token(), "bash", &json!({})),
    ];
    assert_eq!(next_call(&first).n, 1);
    assert_eq!(next_call(&second).n, 1);
    drop(calls);
}

#[test]
fn dropping_the_wiring_takes_the_token_out() {
    let rig = Rig::new("drop");
    let wiring = rig.register();
    let token = wiring.token().to_string();
    assert_eq!(rig.hub.len(), 1);
    drop(wiring);
    assert!(rig.hub.is_empty());
    assert_eq!(rig.hub.list(&token), None);
}

/// 直接往套接字写一行原文、读回一行。
fn raw(socket: &Path, line: &str) -> Value {
    use std::io::{BufRead, BufReader, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
    writeln!(stream, "{line}").unwrap();
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).unwrap();
    serde_json::from_str(&reply).unwrap()
}

#[test]
fn an_old_tool_server_without_a_home_still_gets_the_table_and_reports_nothing() {
    let rig = Rig::new("nohome");
    let wiring = rig.register();
    let line = json!({"token": wiring.token(), "op": "list"}).to_string();
    assert_eq!(raw(&rig.socket(), &line)["tools"][0]["name"], "bash");
    assert_eq!(wiring.reported().get(), None);
}

#[test]
fn a_reported_home_is_kept_per_token_from_list_and_call_and_odd_ones_are_ignored() {
    let rig = Rig::new("home");
    let (one, two) = (rig.register(), rig.register());
    let line = json!({"token": one.token(), "op": "list", "home": "/fake/one"}).to_string();
    assert_eq!(raw(&rig.socket(), &line)["tools"][0]["name"], "bash");
    assert_eq!(one.reported().get(), Some(PathBuf::from("/fake/one")));
    assert_eq!(two.reported().get(), None, "别的口令不受影响");

    // `call` 也能报；不合格的（相对路径、不是字、空的）当没带、不改已经记着的。
    let receiver = listen(&one);
    for odd in [json!("rel"), json!(3), json!("")] {
        let line = json!({"token": one.token(), "op": "list", "home": odd}).to_string();
        assert_eq!(raw(&rig.socket(), &line)["tools"][0]["name"], "bash");
    }
    assert_eq!(one.reported().get(), Some(PathBuf::from("/fake/one")));
    let socket = rig.socket();
    let line = json!({"token": two.token(), "op": "call", "name": "bash", "arguments": {}, "home": "/fake/two"}).to_string();
    let listening = listen(&two);
    let asked = std::thread::spawn(move || raw(&socket, &line));
    let call = next_call(&listening);
    assert_eq!(two.reported().get(), Some(PathBuf::from("/fake/two")));
    call.reply.send(&[], false).unwrap();
    assert_eq!(asked.join().unwrap()["isError"], false);
    drop(receiver);
}

#[test]
fn a_home_reported_with_an_unknown_token_is_dropped() {
    let rig = Rig::new("homeunknown");
    let wiring = rig.register();
    let line = json!({"token": "nope", "op": "list", "home": "/evil"}).to_string();
    assert!(raw(&rig.socket(), &line).get("error").is_some());
    assert_eq!(wiring.reported().get(), None);
}

#[test]
fn the_reported_cell_outlives_the_wiring() {
    let rig = Rig::new("outlive");
    let wiring = rig.register();
    let line = json!({"token": wiring.token(), "op": "list", "home": "/fake/h"}).to_string();
    raw(&rig.socket(), &line);
    let cell = wiring.reported();
    drop(wiring);
    assert_eq!(cell.get(), Some(PathBuf::from("/fake/h")));
}
