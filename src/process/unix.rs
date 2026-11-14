//! Unix 上起、收 agy：`process_group(0)` 让它自成进程组（它起的 MCP 服务这些子进程也在组里），收的时候对整组发信号。

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

/// `SIGTERM` 以后等多久再 `SIGKILL`。
pub const GRACE: Duration = Duration::from_secs(5);

/// 起 `command`，让它自成进程组。标准输入、输出、错误怎么接由调用的人在 `command` 上设好。
///
/// # Errors
///
/// 起不来（找不到程序、没有执行权限）：原样交。
pub fn spawn(mut command: Command) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    command.process_group(0).spawn()
}

/// 收掉 `child` 和它的整个进程组：先 `SIGTERM`，等至多 `grace`，再对整组 `SIGKILL`（领头的早退了、组里还剩的子进程也收），
/// 最后收尸。已经退了的照样走一遍，组里剩下的照样收。
pub fn terminate(child: &mut Child, grace: Duration) {
    let Ok(raw) = i32::try_from(child.id()) else {
        eprintln!("agy process id {} does not fit a pid", child.id());
        return;
    };
    let group = Pid::from_raw(raw);
    signal(group, Signal::SIGTERM);
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(_) => break,
            Err(error) => {
                eprintln!("waiting for agy (pid {raw}) failed: {error}");
                break;
            }
        }
    }
    signal(group, Signal::SIGKILL);
    if let Err(error) = child.wait() {
        eprintln!("reaping agy (pid {raw}) failed: {error}");
    }
}

/// 对进程组发信号；组已经没了（`ESRCH`）不算错，别的错记进日志。
fn signal(group: Pid, signal: Signal) {
    match killpg(group, signal) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => eprintln!("sending {signal} to agy's process group {group} failed: {error}"),
    }
}
