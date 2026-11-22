//! `driver.models`：跑 `agy models`（至多 [`TIMEOUT`]），每行 `slug<TAB>显示名` 交 `{"models": [{"id": slug}, …]}`（设计第二节
//! 「模型」、第九节）。窗口、最大输出 agy 不报，不交。

use std::io::Read;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::agy;
use crate::config::Config;
use crate::process;

/// `agy models` 至多跑多久。
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// 从 `agy models` 的输出读模型名：带 TAB 的行取 TAB 前面那一截；不带的（`Fetching available models...`）、空的跳过。
#[must_use]
pub fn parse(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(slug, _)| slug.trim().to_string())
        .filter(|slug| !slug.is_empty())
        .collect()
}

/// 回答 `driver.models` 的 `result`。
#[must_use]
pub fn answer(slugs: &[String]) -> Value {
    // 都看得了图（施工 AG-9）：图存成文件、agy 用 view_file 看；报上去核心就不先找看图模型转述。
    json!({"models": slugs.iter().map(|slug| json!({"id": slug, "vision": true})).collect::<Vec<_>>()})
}

/// 照 `config` 跑 `agy models`，至多 `timeout`。
///
/// # Errors
///
/// 起不来、超时、退出码不是 0、一个模型都没读到：英文的一句原因（给人看）。
pub fn list(config: &Config, timeout: Duration) -> Result<Vec<String>, String> {
    let mut command = agy::command(config, agy::MODELS_ARGS);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child =
        process::spawn(command).map_err(|error| crate::call::spawn_failure(config, &error))?;
    let (sender, receiver) = mpsc::channel();
    drain(child.stdout.take(), 0, sender.clone());
    drain(child.stderr.take(), 1, sender);

    let deadline = std::time::Instant::now() + timeout;
    let mut outputs = [String::new(), String::new()];
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match receiver.recv_timeout(left) {
            Ok((index, Ok(text))) => outputs[index] = text,
            Ok((_, Err(error))) => {
                process::terminate(&mut child, process::GRACE);
                return Err(format!(
                    "reading the output of `agy models` failed: {error}"
                ));
            }
            Err(_) => {
                process::terminate(&mut child, process::GRACE);
                return Err(format!(
                    "`agy models` did not finish within {} seconds",
                    timeout.as_secs()
                ));
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("waiting for `agy models` failed: {error}"))?;
    let [stdout, stderr] = outputs;
    if !status.success() {
        return Err(format!(
            "`agy models` failed ({status}): {}",
            stderr.trim().chars().take(400).collect::<String>()
        ));
    }
    let slugs = parse(&stdout);
    if slugs.is_empty() {
        return Err("`agy models` listed no models".into());
    }
    Ok(slugs)
}

/// 在另一个线程里把 `stream` 读完，连同第几路 `index` 交给 `sender`。
fn drain(
    stream: Option<impl Read + Send + 'static>,
    index: usize,
    sender: mpsc::Sender<(usize, std::io::Result<String>)>,
) {
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = match stream {
            Some(mut stream) => stream.read_to_string(&mut text).map(|_| text),
            None => Ok(text),
        };
        // 等的那头超时先走了：没人要这份输出，发不出去就算了。
        sender.send((index, read)).ok();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_takes_the_slug_before_the_tab() {
        let output = "Fetching available models...\n\
                      gemini-3.8-flash-low\tGemini 3.8 Flash (Low)\n\
                      \n\
                      claude-opus-5-5-high\tClaude Opus 5.5 (High)\r\n\
                      no tab here\n";
        assert_eq!(
            parse(output),
            ["gemini-3.8-flash-low", "claude-opus-5-5-high"]
        );
        assert!(parse("Fetching available models...\n").is_empty());
    }

    #[test]
    fn answer_lists_ids() {
        assert_eq!(
            answer(&["a".into(), "b".into()]),
            json!({"models": [{"id": "a", "vision": true}, {"id": "b", "vision": true}]})
        );
    }
}
