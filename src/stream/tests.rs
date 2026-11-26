use super::*;

const PONG: &str = include_str!("../../tests/samples/real-pong.jsonl");
const SEARCH: &str = include_str!("../../tests/samples/real-search-read.jsonl");
const TRANSCRIPT: &str = include_str!("../../tests/samples/real-search-read.transcript.jsonl");

fn events(sample: &str) -> Vec<Event> {
    sample.lines().map(|line| parse(line).unwrap()).collect()
}

fn usage(input: u64, output: u64, cache_read: u64) -> Usage {
    Usage {
        input,
        output,
        thinking: 0,
        cache_read,
    }
}

#[test]
fn real_pong_reads_as_init_user_two_response_lines_and_result() {
    let got = events(PONG);
    assert_eq!(got.len(), 5);
    assert_eq!(
        got[0],
        Event::Init {
            conversation_id: Some("ff223be4-3376-4486-b661-7414044371c2".into()),
            agent: Some("probe-a".into()),
        }
    );
    assert_eq!(got[1], Event::UserInput);
    assert_eq!(
        got[2],
        Event::Response {
            step_index: 1,
            done: false,
            text: Some("pong".into()),
            usage: None,
        }
    );
    assert_eq!(
        got[3],
        Event::Response {
            step_index: 1,
            done: true,
            text: Some("\n".into()),
            usage: Some(usage(2679, 1, 0)),
        }
    );
    let Event::Result(outcome) = &got[4] else {
        panic!("最后一条是 result：{:?}", got[4]);
    };
    assert!(outcome.succeeded());
    assert_eq!(outcome.response, "pong\n");
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.usage, Some(usage(2679, 1, 0)));
    assert_eq!(
        outcome.conversation_id.as_deref(),
        Some("ff223be4-3376-4486-b661-7414044371c2")
    );
}

#[test]
fn real_search_read_has_two_tool_steps_and_five_text_pieces() {
    let got = events(SEARCH);
    assert_eq!(got.len(), 15);
    let tools: Vec<&ToolStep> = got
        .iter()
        .filter_map(|event| match event {
            Event::Tool(step) => Some(step),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 4, "两个工具步，各一条 ACTIVE、一条 DONE");
    let expected = [
        (
            2,
            ToolState::Active,
            "search_web",
            r#"{"query":"EditorConfig specification"}"#,
        ),
        (
            2,
            ToolState::Done,
            "search_web",
            r#"{"query":"EditorConfig specification"}"#,
        ),
        (
            4,
            ToolState::Active,
            "read_url_content",
            r#"{"Url":"https://spec.editorconfig.org/"}"#,
        ),
        (
            4,
            ToolState::Done,
            "read_url_content",
            r#"{"Url":"https://spec.editorconfig.org/"}"#,
        ),
    ];
    for (step, (index, state, name, parameters)) in tools.iter().zip(expected) {
        assert_eq!(step.step_index, index);
        assert_eq!(step.state, state);
        assert_eq!(step.name, name);
        assert_eq!(step.parameters.as_deref(), Some(parameters));
        assert_eq!(step.output, None, "原生工具的 DONE 不带结果");
        assert_eq!(step.error, None);
    }
    assert_eq!(
        tools[0].describe(),
        r#"tool step 2 search_web ACTIVE parameters={"query":"EditorConfig specification"}"#
    );

    let pieces: Vec<&str> = got
        .iter()
        .filter_map(|event| match event {
            Event::Response {
                step_index: 5,
                done: false,
                text: Some(text),
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(pieces.len(), 5);
    assert_eq!(
        pieces.concat(),
        "NO-TOOL; the page title is \"EditorConfig Specification — EditorConfig Specification 0.17.2 documentation\"."
    );

    let done: Vec<(u64, Option<&str>, Option<Usage>)> = got
        .iter()
        .filter_map(|event| match event {
            Event::Response {
                step_index,
                done: true,
                text,
                usage,
            } => Some((*step_index, text.as_deref(), *usage)),
            _ => None,
        })
        .collect();
    assert_eq!(
        done,
        vec![
            (1, None, Some(usage(2741, 38, 0))),
            (3, None, Some(usage(2482, 47, 2758))),
            (5, Some("\n"), Some(usage(2330, 25, 3208))),
        ]
    );
    let Some(Event::Result(outcome)) = got.last() else {
        panic!("最后一条是 result");
    };
    assert_eq!(
        outcome.usage,
        Some(usage(7553, 110, 5966)),
        "result 的是整段累计"
    );
}

#[test]
fn transcript_lines_are_not_stream_events() {
    // 转录（AG-4 才读）是另一种格式，没有 `event`：流的解析器不把它错认成事件。
    for line in TRANSCRIPT.lines() {
        assert_eq!(parse(line).unwrap(), Event::Ignored);
    }
}

#[test]
fn tool_error_step_reads_type_and_message() {
    // 照设计第二节「工具步骤」和旧版 stream.rs 手写：真样本里没有 ERROR。
    let line = r#"{"event":"step_update","step_update":{"conversation_id":"c","step_index":3,"state":"ERROR","step_type":"tool","tool_name":"mcp_miyu_read","tool_info":{"name":"mcp_miyu_read","parameters":{"path":"a","limit":2},"error":{"type":"TIMEOUT","message":"MCP tool call to server \"miyu\" timed out after 3m0s"}}}}"#;
    assert_eq!(
        parse(line).unwrap(),
        Event::Tool(ToolStep {
            step_index: 3,
            state: ToolState::Error,
            name: "mcp_miyu_read".into(),
            parameters: Some(r#"{"path":"a","limit":2}"#.into()),
            output: None,
            error: Some(ToolError {
                kind: Some("TIMEOUT".into()),
                message: "MCP tool call to server \"miyu\" timed out after 3m0s".into(),
            }),
        })
    );
    let Event::Tool(step) = parse(line).unwrap() else {
        panic!("是工具步");
    };
    assert!(step.describe().ends_with(
        "ERROR parameters={\"path\":\"a\",\"limit\":2} error=TIMEOUT: MCP tool call to server \"miyu\" timed out after 3m0s"
    ));
}

#[test]
fn mcp_tool_done_carries_output() {
    // 手写：MCP 工具的 DONE 带 tool_info.output（设计第二节）。
    let line = r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"tool","tool_info":{"name":"mcp_miyu_ls","parameters":{},"output":"a.txt\nb.txt"}}}"#;
    let Event::Tool(step) = parse(line).unwrap() else {
        panic!("是工具步");
    };
    assert_eq!(
        step.name, "mcp_miyu_ls",
        "没有 tool_name 的照 tool_info.name"
    );
    assert_eq!(step.state, ToolState::Done);
    assert_eq!(step.output.as_deref(), Some("a.txt\nb.txt"));
}

#[test]
fn error_message_step_and_failed_result() {
    // 手写，照旧版 stream.rs：error_message 步的字在 text_delta；result 的 error 可以是字也可以是对象。
    let message = r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"error_message","text_delta":"RESOURCE_EXHAUSTED: quota"}}"#;
    assert_eq!(
        parse(message).unwrap(),
        Event::ErrorMessage {
            text: "RESOURCE_EXHAUSTED: quota".into()
        }
    );
    let failed = r#"{"event":"result","result":{"status":"ERROR","response":"","error":"You are not logged into Antigravity"}}"#;
    let Event::Result(outcome) = parse(failed).unwrap() else {
        panic!("是 result");
    };
    assert!(!outcome.succeeded());
    assert_eq!(
        outcome.error.as_deref(),
        Some("You are not logged into Antigravity")
    );
    assert_eq!(outcome.usage, None);
    let object = r#"{"event":"result","result":{"status":"ERROR","error":{"code":429}}}"#;
    let Event::Result(outcome) = parse(object).unwrap() else {
        panic!("是 result");
    };
    assert_eq!(outcome.error.as_deref(), Some(r#"{"code":429}"#));
}

#[test]
fn unknown_events_and_step_types_are_ignored() {
    assert_eq!(parse(r#"{"event":"heartbeat"}"#).unwrap(), Event::Ignored);
    assert_eq!(parse(r#"{"no_event":1}"#).unwrap(), Event::Ignored);
    assert_eq!(parse("[1,2]").unwrap(), Event::Ignored);
    assert_eq!(
        parse(r#"{"event":"step_update","step_update":{"step_index":9,"state":"DONE","step_type":"planner_thought"}}"#)
            .unwrap(),
        Event::Ignored
    );
}

#[test]
fn a_line_that_is_not_json_is_an_error() {
    let error = parse("Fetching available models...").unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("agy wrote a line that is not JSON: ")
    );
    assert!(parse(r#"{"event":"init""#).is_err(), "半截的 JSON");
}

#[test]
fn usage_adds_up_and_the_context_is_input_plus_output() {
    // 施工 AG-13：记账照几次加起来；上下文大小是一次请求的输入（含命中缓存的）加输出。
    let a = Usage {
        input: 1,
        output: 2,
        thinking: 3,
        cache_read: 4,
    };
    assert_eq!(
        a.plus(a),
        Usage {
            input: 2,
            output: 4,
            thinking: 6,
            cache_read: 8
        }
    );
    assert_eq!(a.context(), 1 + 4 + 2);
}
