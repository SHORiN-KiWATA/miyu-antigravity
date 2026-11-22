//! 核心交的图、文件存进 `MIYU_STATE_DIR/media`，转写里写上路径，agy 用 `view_file` 看（施工 AG-9）。

use serde_json::{Value, json};

use crate::support::{Core, request};

/// 一张图、一份 PDF 跟着人说的话来：存成按哈希起名的文件，交给 agy 的那一行写着两个路径；没交内容的那一份照旧写看不到。
#[test]
fn images_and_files_are_saved_and_their_paths_are_written() {
    let mut core = Core::start("media", "echo");
    let png = format!("sha256:{}", "a".repeat(64));
    let pdf = format!("sha256:{}", "b".repeat(64));
    let gone = format!("sha256:{}", "c".repeat(64));
    let mut asked = request("s", "what is in these?");
    asked["messages"][0]["blocks"] = json!([
        {"type": "text", "text": "what is in these?"},
        {"type": "image", "blob": png, "name": "shot.png", "media_type": "image/png"},
        {"type": "file", "blob": pdf, "name": "报告.pdf", "media_type": "application/pdf"},
        {"type": "file", "blob": gone, "name": "old.pdf", "media_type": "application/pdf"},
    ]);
    core.send(&json!({
        "jsonrpc": "2.0",
        "id": "r1",
        "method": "driver.call",
        "params": {
            "call": "d1",
            "driver": "antigravity/antigravity",
            "request": asked,
            "fingerprint": crate::support::fingerprint(&asked),
            "params": {"model": "gemini-3.8-flash-low"},
            "blobs": {
                png.clone(): {"media_type": "image/png", "data": "iVBORw=="},
                pdf.clone(): {"media_type": "application/pdf", "data": "JVBERi0="},
            }
        }
    }));
    let got = core.collect("r1", "d1");
    assert!(got.response.get("result").is_some(), "{}", got.response);

    let media = core.state.join("media");
    let png_path = media.join(format!("{}.png", "a".repeat(64)));
    let pdf_path = media.join(format!("{}.pdf", "b".repeat(64)));
    assert_eq!(std::fs::read(&png_path).unwrap(), b"\x89PNG");
    assert_eq!(std::fs::read(&pdf_path).unwrap(), b"%PDF-");

    let record = core.records().remove(0);
    let line: Value = serde_json::from_str(record["lines"][0].as_str().unwrap()).unwrap();
    let text = line["message"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains(&format!(
            "[image \"shot.png\" saved at {}; open it with view_file to see it]",
            png_path.display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "[file \"报告.pdf\" saved at {}; open it with view_file to read it]",
            pdf_path.display()
        )),
        "{text}"
    );
    assert!(
        text.contains("[file \"old.pdf\" omitted: this model reads text only]"),
        "{text}"
    );
}
