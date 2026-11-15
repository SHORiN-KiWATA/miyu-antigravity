//! miyu-antigravity 的命令行入口：分派到库里的 `serve`（说明见库的头注释）。

use std::process::ExitCode;

use miyu_antigravity::rpc;

const USAGE: &str = "usage: miyu-antigravity serve   (started by the Miyu core; provides the model driver antigravity/antigravity)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => rpc::serve(std::io::stdin().lock(), &rpc::Out::stdout()),
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
