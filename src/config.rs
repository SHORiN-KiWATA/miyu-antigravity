//! 这个包自己的配置项（清单 `[settings.binary]`、`[settings.keep_seconds]`、`[settings.tool_wait_seconds]`，设计第九、十节）：握手的 `config` 交全的，`extension.config` 交变了的。空的字、
//! `null` 当没设。用哪个账号不归本包管：agy 照扩展自己的 `HOME` 跑（设计第九节、第十一节第 4 条）。

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};

/// `antigravity.binary` 的键。
pub const BINARY: &str = "antigravity.binary";

/// `antigravity.keep_seconds` 的键。
pub const KEEP_SECONDS: &str = "antigravity.keep_seconds";
/// `antigravity.keep_seconds` 的默认值：闲着的 agy 进程留 10 分钟。
pub const DEFAULT_KEEP: Duration = Duration::from_mins(10);

/// `antigravity.tool_wait_seconds` 的键。
pub const TOOL_WAIT_SECONDS: &str = "antigravity.tool_wait_seconds";
/// `antigravity.tool_wait_seconds` 的默认值：一个工具调用等 1 小时（设计第十节）。
pub const DEFAULT_TOOL_WAIT: Duration = Duration::from_hours(1);
/// `antigravity.tool_wait_seconds` 至少 60 秒（清单里的 `min`）。
pub const MIN_TOOL_WAIT_SECONDS: u64 = 60;
/// `antigravity.tool_wait_seconds` 至多 86400 秒（清单里的 `max`）。
pub const MAX_TOOL_WAIT_SECONDS: u64 = 86_400;

/// 现在的配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// agy 在哪；没有的在 `PATH` 里找 `agy`。
    pub binary: Option<PathBuf>,
    /// 闲着的 agy 进程留多久。
    pub keep: Duration,
    /// 一个挂着的工具调用等多久（设计第十节；谁来用它是 AG-3 下半的事）。
    pub tool_wait: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            binary: None,
            keep: DEFAULT_KEEP,
            tool_wait: DEFAULT_TOOL_WAIT,
        }
    }
}

impl Config {
    /// 照 `keys` 改：只看这个包的键，别的不理。
    ///
    /// # Errors
    ///
    /// `antigravity.binary` 不是字也不是 `null`、`antigravity.keep_seconds` 不是不小于 0 的整数也不是 `null`、
    /// `antigravity.tool_wait_seconds` 不是 60 到 86400 的整数也不是 `null`：说是哪个键，什么都不改。
    pub fn apply(&mut self, keys: &Map<String, Value>) -> Result<(), String> {
        let mut next = self.clone();
        match keys.get(BINARY) {
            None => {}
            Some(Value::Null) => next.binary = None,
            Some(Value::String(text)) if text.is_empty() => next.binary = None,
            Some(Value::String(text)) => next.binary = Some(PathBuf::from(text)),
            Some(other) => return Err(format!("{BINARY} must be text, got {other}")),
        }
        match keys.get(KEEP_SECONDS) {
            None => {}
            Some(Value::Null) => next.keep = DEFAULT_KEEP,
            Some(value) => match value.as_u64() {
                Some(seconds) => next.keep = Duration::from_secs(seconds),
                None => {
                    return Err(format!(
                        "{KEEP_SECONDS} must be a whole number of seconds, got {value}"
                    ));
                }
            },
        }
        match keys.get(TOOL_WAIT_SECONDS) {
            None => {}
            Some(Value::Null) => next.tool_wait = DEFAULT_TOOL_WAIT,
            Some(value) => match value
                .as_u64()
                .filter(|seconds| (MIN_TOOL_WAIT_SECONDS..=MAX_TOOL_WAIT_SECONDS).contains(seconds))
            {
                Some(seconds) => next.tool_wait = Duration::from_secs(seconds),
                None => {
                    return Err(format!(
                        "{TOOL_WAIT_SECONDS} must be a whole number of seconds from {MIN_TOOL_WAIT_SECONDS} to {MAX_TOOL_WAIT_SECONDS}, got {value}"
                    ));
                }
            },
        }
        *self = next;
        Ok(())
    }

    /// 起 agy 用的程序。
    #[must_use]
    pub fn program(&self) -> PathBuf {
        self.binary.clone().unwrap_or_else(|| PathBuf::from("agy"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn keys(value: &Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn strings_set_and_empty_or_null_clear() {
        let mut config = Config::default();
        config
            .apply(&keys(&json!({BINARY: "/bin/agy", "other.key": 3})))
            .unwrap();
        assert_eq!(config.program(), PathBuf::from("/bin/agy"));

        config.apply(&keys(&json!({"other.key": "x"}))).unwrap();
        assert_eq!(
            config.binary,
            Some(PathBuf::from("/bin/agy")),
            "只改交了的键"
        );
        config.apply(&keys(&json!({BINARY: ""}))).unwrap();
        assert_eq!(config.binary, None);
        assert_eq!(config.program(), PathBuf::from("agy"));
        config.apply(&keys(&json!({BINARY: "/bin/agy"}))).unwrap();
        config.apply(&keys(&json!({BINARY: null}))).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn non_text_is_an_error_and_changes_nothing() {
        let mut config = Config::default();
        let error = config.apply(&keys(&json!({BINARY: 5}))).unwrap_err();
        assert!(error.contains(BINARY), "{error}");
        for bad in [json!(-1), json!(1.5), json!("600")] {
            let error = config
                .apply(&keys(&json!({BINARY: "/bin/agy", KEEP_SECONDS: bad})))
                .unwrap_err();
            assert!(error.contains(KEEP_SECONDS), "{error}");
        }
        assert_eq!(config, Config::default());
    }

    #[test]
    fn keep_seconds_is_whole_seconds_and_null_is_the_default() {
        let mut config = Config::default();
        assert_eq!(config.keep, Duration::from_mins(10));
        config.apply(&keys(&json!({KEEP_SECONDS: 30}))).unwrap();
        assert_eq!(config.keep, Duration::from_secs(30));
        config.apply(&keys(&json!({BINARY: "/bin/agy"}))).unwrap();
        assert_eq!(config.keep, Duration::from_secs(30), "只改交了的键");
        config.apply(&keys(&json!({KEEP_SECONDS: null}))).unwrap();
        assert_eq!(config.keep, DEFAULT_KEEP);
    }

    #[test]
    fn tool_wait_is_whole_seconds_within_the_range_and_null_is_the_default() {
        let mut config = Config::default();
        assert_eq!(config.tool_wait, Duration::from_hours(1));
        for seconds in [60, 3600, 86_400] {
            config
                .apply(&keys(&json!({TOOL_WAIT_SECONDS: seconds})))
                .unwrap();
            assert_eq!(config.tool_wait, Duration::from_secs(seconds));
        }
        config.apply(&keys(&json!({BINARY: "/bin/agy"}))).unwrap();
        assert_eq!(config.tool_wait, Duration::from_hours(24), "只改交了的键");
        config
            .apply(&keys(&json!({TOOL_WAIT_SECONDS: null})))
            .unwrap();
        assert_eq!(config.tool_wait, DEFAULT_TOOL_WAIT);
    }

    #[test]
    fn tool_wait_out_of_range_or_not_an_integer_is_an_error_and_changes_nothing() {
        let mut config = Config::default();
        config
            .apply(&keys(&json!({TOOL_WAIT_SECONDS: 120})))
            .unwrap();
        for bad in [
            json!(59),
            json!(86_401),
            json!(0),
            json!(-1),
            json!(60.5),
            json!("600"),
        ] {
            let error = config
                .apply(&keys(&json!({KEEP_SECONDS: 5, TOOL_WAIT_SECONDS: bad})))
                .unwrap_err();
            assert!(error.contains(TOOL_WAIT_SECONDS), "{error}");
            assert_eq!(config.tool_wait, Duration::from_mins(2));
            assert_eq!(config.keep, DEFAULT_KEEP, "出错什么都不改");
        }
    }
}
