# SIG4A

SIG4A là repo mới cho ứng dụng single 4 all. Phần Rust được giữ nguyên từ repo cũ và nằm trong workspace `rust/`.

## Rust workspace

Workspace hiện ghim toolchain stable và target Windows MSVC trong [`rust/rust-toolchain.toml`](rust/rust-toolchain.toml). Cần cài Rust qua `rustup`; dashboard cần WebView2 trên Windows.

Chạy các lệnh từ thư mục `rust/`:

```powershell
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`ui.html` và `assets/icon.ico` ở repo root là các asset được Rust dashboard và build script tham chiếu.
