//! 这个包自己的两个配置项（清单 `[settings.binary]`、`[settings.home]`，设计第九节）：握手的 `config` 交全的，
//! `extension.config` 交变了的。空的字、`null` 当没设。

use std::path::PathBuf;

use serde_json::{Map, Value};

/// `antigravity.binary` 的键。
pub const BINARY: &str = "antigravity.binary";
/// `antigravity.home` 的键。
pub const HOME: &str = "antigravity.home";

/// 现在的配置。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// agy 在哪；没有的在 `PATH` 里找 `agy`。
    pub binary: Option<PathBuf>,
    /// 用哪个家目录跑 agy；没有的用扩展自己的 `HOME`。
    pub home: Option<PathBuf>,
}

impl Config {
    /// 照 `keys` 改：只看这两个键，别的不理。
    ///
    /// # Errors
    ///
    /// 这两个键的值不是字也不是 `null`：说是哪个键，什么都不改。
    pub fn apply(&mut self, keys: &Map<String, Value>) -> Result<(), String> {
        let mut next = self.clone();
        for (key, slot) in [(BINARY, &mut next.binary), (HOME, &mut next.home)] {
            match keys.get(key) {
                None => {}
                Some(Value::Null) => *slot = None,
                Some(Value::String(text)) if text.is_empty() => *slot = None,
                Some(Value::String(text)) => *slot = Some(PathBuf::from(text)),
                Some(other) => return Err(format!("{key} must be text, got {other}")),
            }
        }
        *self = next;
        Ok(())
    }

    /// 起 agy 用的程序。
    #[must_use]
    pub fn program(&self) -> PathBuf {
        self.binary.clone().unwrap_or_else(|| PathBuf::from("agy"))
    }

    /// agy 的家目录：配置的，没配的是扩展自己的 `HOME`；都没有的是 `None`。
    #[must_use]
    pub fn agy_home(&self) -> Option<PathBuf> {
        self.home.clone().or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(PathBuf::from)
        })
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
            .apply(&keys(
                &json!({BINARY: "/bin/agy", HOME: "/acct", "other.key": 3}),
            ))
            .unwrap();
        assert_eq!(config.program(), PathBuf::from("/bin/agy"));
        assert_eq!(config.home, Some(PathBuf::from("/acct")));
        assert_eq!(config.agy_home(), Some(PathBuf::from("/acct")));

        config.apply(&keys(&json!({BINARY: ""}))).unwrap();
        assert_eq!(config.binary, None, "只改交了的键");
        assert_eq!(config.home, Some(PathBuf::from("/acct")));
        assert_eq!(config.program(), PathBuf::from("agy"));
        config.apply(&keys(&json!({HOME: null}))).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn non_text_is_an_error_and_changes_nothing() {
        let mut config = Config::default();
        let error = config
            .apply(&keys(&json!({BINARY: "/bin/agy", HOME: 5})))
            .unwrap_err();
        assert!(error.contains(HOME), "{error}");
        assert_eq!(config, Config::default());
    }
}
