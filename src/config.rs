//! 这个包自己的配置项（清单 `[settings.binary]`、`[settings.keep_seconds]`，设计第九节）：握手的 `config` 交全的，`extension.config` 交变了的。空的字、
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

/// 现在的配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// agy 在哪；没有的在 `PATH` 里找 `agy`。
    pub binary: Option<PathBuf>,
    /// 闲着的 agy 进程留多久。
    pub keep: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            binary: None,
            keep: DEFAULT_KEEP,
        }
    }
}

impl Config {
    /// 照 `keys` 改：只看这个包的键，别的不理。
    ///
    /// # Errors
    ///
    /// `antigravity.binary` 不是字也不是 `null`、`antigravity.keep_seconds` 不是不小于 0 的整数也不是 `null`：说是哪个键，
    /// 什么都不改。
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
}
