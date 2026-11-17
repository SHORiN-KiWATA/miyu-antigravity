//! agy 实际用的家目录（设计第七、八节）。
//!
//! `antigravity.binary` 可以指到一个包装脚本，它可能给 agy 换一个 `HOME`（假家目录里的 `.gemini/config` 是软链回真家目录的，
//! 所以代理文件和 MCP 登记照常生效）；agy 的对话数据（`.gemini/antigravity-cli/brain/<编号>/…`、`conversations/<编号>.db*`）
//! 却落在它实际用的那个家目录下。agy 拉起的 MCP 服务继承 agy 的环境，它的 `HOME` 就是那个目录：每次经套接字找扩展时报上来
//! （[`crate::bridge`]），总台（[`crate::hub`]）记在这段对话的口令上（[`Reported`]），读转录、删会话文件、启动清扫的记号都照它。
//! 没报过的（请求不带工具、没起工具服务）照旧用扩展自己的 `HOME`。
//!
//! 两样东西：
//!
//! - [`Reported`]：和总台共享的格子。`Wiring` 丢掉（口令没了）以后 [`crate::live::Live`] 收尾时还要拿得到，所以不能到时候再去问总台。
//! - [`AgyHome`]：一个 [`crate::live::Live`] 用的：自己的家目录、状态目录、已经记了号的对话编号；管记号的写（头一行进程号，报来了
//!   和自己的不同再加第二行 agy 的家目录；写记号的时候还没报到、之后才报到的，补写一次，别丢了进程号那一行）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::{leftovers, workspace};

/// agy 的 MCP 服务报来的家目录，和总台共享。
#[derive(Default)]
pub struct Reported {
    home: Mutex<Option<PathBuf>>,
    /// 日志里带的 agy 进程号（起进程以后才知道，0 是还不知道）。
    pid: AtomicU32,
    /// 报来了新的值以后要做的事（补写记号）。
    watcher: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl std::fmt::Debug for Reported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reported")
            .field("home", &self.get())
            .finish_non_exhaustive()
    }
}

impl Reported {
    /// 报来的家目录；没报过的是 `None`。
    #[must_use]
    pub fn get(&self) -> Option<PathBuf> {
        self.home
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 记下 agy 的进程号（日志用）。
    pub fn set_pid(&self, pid: u32) {
        self.pid.store(pid, Ordering::SeqCst);
    }

    /// 报来了新的值时调 `watcher`（在报的那条连接的线程里调）。
    pub fn watch(&self, watcher: impl Fn() + Send + 'static) {
        *self.watcher.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(watcher));
    }

    /// 总台收到一次报来的 `home`（已经验过是绝对路径）：和记着的一样的不理；换了值的记下，头一次和扩展自己的 `HOME` 不一样时
    /// 记一行日志（同一个值不重复记），再叫 [`Reported::watch`] 那件事。
    pub fn report(&self, home: &Path) {
        {
            let mut slot = self.home.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.as_deref() == Some(home) {
                return;
            }
            *slot = Some(home.to_path_buf());
        }
        if workspace::home().as_deref() != Some(home) {
            let pid = self.pid.load(Ordering::SeqCst);
            let who = if pid == 0 {
                "agy".to_string()
            } else {
                format!("agy (pid {pid})")
            };
            eprintln!(
                "{who}: its tool server runs with HOME {}, not the extension's own; reading its transcripts and removing its conversation files there",
                home.display()
            );
        }
        if let Some(watcher) = self
            .watcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            watcher();
        }
    }
}

/// 一个 agy 进程的家目录：报来的用报来的，没报过的是扩展自己的。
pub struct AgyHome {
    own: PathBuf,
    state: PathBuf,
    pid: u32,
    reported: Arc<Reported>,
    /// 已经记了号的对话编号。
    marked: Mutex<Option<String>>,
}

impl AgyHome {
    /// `own` 是扩展自己的家目录，`state` 是状态目录（记号放这里），`pid` 是 agy 的进程号（日志用），`reported` 是和总台共享的格子。
    #[must_use]
    pub fn new(own: PathBuf, state: PathBuf, pid: u32, reported: &Arc<Reported>) -> Arc<AgyHome> {
        let home = Arc::new(AgyHome {
            own,
            state,
            pid,
            reported: Arc::clone(reported),
            marked: Mutex::new(None),
        });
        reported.set_pid(pid);
        let weak: Weak<AgyHome> = Arc::downgrade(&home);
        reported.watch(move || {
            if let Some(home) = weak.upgrade() {
                home.refresh();
            }
        });
        home
    }

    /// agy 实际用的家目录。
    #[must_use]
    pub fn current(&self) -> PathBuf {
        self.reported.get().unwrap_or_else(|| self.own.clone())
    }

    /// 记号第二行要写的：报来的、而且和扩展自己的不一样的。
    fn extra(&self) -> Option<PathBuf> {
        self.reported.get().filter(|home| *home != self.own)
    }

    /// 刚得知对话编号 `conversation`：在状态目录里记号（[`leftovers::mark_with_home`]）。编号不合
    /// [`workspace::valid_conversation`] 的不记；已经记过的不重复写；换了编号的把旧的记号删掉；写不了的记一行日志。
    pub fn note_conversation(&self, conversation: &str) {
        let mut marked = self.marked.lock().unwrap_or_else(PoisonError::into_inner);
        if marked.as_deref() == Some(conversation) || !workspace::valid_conversation(conversation) {
            return;
        }
        match leftovers::mark_with_home(&self.state, conversation, self.extra().as_deref()) {
            Ok(()) => {
                if let Some(old) = marked.replace(conversation.to_string())
                    && let Err(error) = leftovers::unmark(&self.state, &old)
                {
                    eprintln!(
                        "agy (pid {}): could not remove the mark of {old}: {error}",
                        self.pid
                    );
                }
            }
            Err(error) => eprintln!(
                "agy (pid {}): could not mark its conversation {conversation}: {error}",
                self.pid
            ),
        }
    }

    /// 家目录报来了新的值：已经记了号的补写一次（照 [`leftovers::mark_with_home`] 先写暂存再改名）。
    fn refresh(&self) {
        let marked = self.marked.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(conversation) = marked.as_deref()
            && let Err(error) =
                leftovers::mark_with_home(&self.state, conversation, self.extra().as_deref())
        {
            eprintln!(
                "agy (pid {}): could not update the mark of {conversation}: {error}",
                self.pid
            );
        }
    }

    /// 收尾：会话文件删干净了（`removed`）才把记号删掉；没删干净的留着，下次启动的清扫再试。
    pub fn finish(&self, removed: bool) {
        let marked = self
            .marked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if removed
            && let Some(marked) = marked
            && let Err(error) = leftovers::unmark(&self.state, &marked)
        {
            eprintln!(
                "agy (pid {}): could not remove the mark of {marked}: {error}",
                self.pid
            );
        }
    }
}

#[cfg(test)]
mod tests;
