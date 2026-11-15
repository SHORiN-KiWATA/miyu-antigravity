//! miyu-antigravity 的命令行入口：分派到库里的 `serve`、`mcp`、`unregister`（说明见库的头注释）。

use std::process::ExitCode;

use miyu_antigravity::mcp::{self, Bridge};
use miyu_antigravity::{rpc, unregister};

const USAGE: &str = "usage: miyu-antigravity serve        (started by the Miyu core; provides the model driver antigravity/antigravity)\n       miyu-antigravity mcp          (started by agy; the MCP server that hands tool calls to Miyu)\n       miyu-antigravity unregister   (removes the miyu entry and miyu-* agents from the agy configuration)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => rpc::serve(std::io::stdin().lock(), &rpc::Out::stdout()),
        Some("mcp") => mcp::run(
            std::io::stdin().lock(),
            &rpc::Out::stdout(),
            Bridge::from_env(),
        ),
        Some("unregister") => unregister::run(),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
