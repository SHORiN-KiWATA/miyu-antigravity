use super::*;

fn list(home: &Value) -> String {
    json!({"token": "t", "op": "list", "home": home}).to_string()
}

#[test]
fn a_request_may_carry_an_absolute_home_and_old_ones_do_not() {
    assert_eq!(
        parse(&list(&json!("/tmp/fake-home"))),
        Ok(Request::List {
            token: "t".into(),
            home: Some(PathBuf::from("/tmp/fake-home")),
        })
    );
    assert_eq!(
        parse(r#"{"token": "t", "op": "list"}"#),
        Ok(Request::List {
            token: "t".into(),
            home: None,
        })
    );
    let call =
        json!({"token": "t", "op": "call", "name": "bash", "arguments": {"a": 1}, "home": "/h"});
    assert_eq!(
        parse(&call.to_string()),
        Ok(Request::Call {
            token: "t".into(),
            name: "bash".into(),
            arguments: json!({"a": 1}),
            home: Some(PathBuf::from("/h")),
        })
    );
}

#[test]
fn a_home_that_is_not_a_string_is_empty_or_relative_counts_as_absent_without_an_error() {
    for odd in [
        json!(5),
        json!(null),
        json!(["/a"]),
        json!({"a": "/b"}),
        json!(true),
        json!(""),
        json!("relative/dir"),
        json!("./x"),
        json!("~/x"),
    ] {
        assert_eq!(
            parse(&list(&odd)),
            Ok(Request::List {
                token: "t".into(),
                home: None,
            }),
            "{odd}"
        );
    }
}
