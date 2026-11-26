# Change report

## 2.3.0 — 2026-11-26

Bản nền cho SIG4A sau khi chuyển sang repo mới:

- Nhập nguyên workspace Rust hiện có từ `claude-usage/usage4all/rust/` vào `rust/`, giữ nguyên cấu trúc và mã nguồn.
- Đưa `ui.html` và `assets/icon.ico` vào repo root để dashboard và build script tiếp tục dùng đúng đường dẫn.
- Thêm README và `.gitignore` ở root cho workflow của workspace Rust.
- Đồng bộ version workspace và các package Rust thành `2.3.0`.

Đây là baseline khởi tạo/chuyển repo; không thêm tính năng mới trong bản này.
