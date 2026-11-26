#![windows_subsystem = "windows"]
use std::{env, path::PathBuf, process::{self, Command, Stdio}};
fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let dir = PathBuf::from(env::var_os("SIG4A_PROOF_DIR").unwrap());
    let script = PathBuf::from(env::var_os("SIG4A_PROOF_SCRIPT").unwrap());
    let dashboard = env::current_exe().unwrap().with_file_name("product-real").join("claude-usage-dashboard.exe");
    let status = Command::new(dashboard).args(args).arg("--e2e").arg(script)
        .arg(dir.join("view.result")).stdin(Stdio::inherit()).status().unwrap();
    process::exit(status.code().unwrap_or(1));
}
