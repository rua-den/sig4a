#![windows_subsystem = "windows"]
use std::{env, path::PathBuf, process::{self, Command, Stdio}};
fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let quick = args.first().is_some_and(|arg| arg == "--quick-tool");
    let dir = PathBuf::from(env::var_os("SIG4A_PROOF_DIR").unwrap());
    let source = PathBuf::from(env::var_os("SIG4A_PROOF_SOURCE").unwrap());
    let dashboard = env::current_exe().unwrap().with_file_name("dashboard-product.exe");
    let name = if quick { "quick" } else { "dashboard" };
    let code = Command::new(dashboard).args(&args).arg("--e2e")
        .arg(source.join(format!("{name}.js"))).arg(dir.join(format!("{name}.result")))
        .stdin(Stdio::inherit()).status().unwrap().code().unwrap_or(1);
    process::exit(code);
}
