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

## Kane's Tool Box

Đặt `claude-usage.exe` và `claude-usage-dashboard.exe` cạnh nhau rồi chạy `claude-usage.exe` để mở tray. Dashboard có trang Tổng quan để mở nhanh các công cụ.

- **Grammar** và **Translation**. Bôi đen chữ rồi bấm `Ctrl+Alt+G` hoặc `Ctrl+Alt+T`. Popup tự chạy trên đoạn đã chọn. Với ứng dụng không cung cấp vùng chọn qua Windows UI Automation, hãy dán văn bản vào popup. Kết quả có thể chỉnh sửa trước khi sao chép.
- **OCR**. Bấm `Ctrl+Alt+O` rồi kéo chọn vùng trên màn hình. Model vision đọc ảnh và đưa chữ vào popup để **Sao chép** hoặc **Dịch**. Dùng `Esc` để hủy vùng chụp.

Trong **Local AI**, chọn Ollama và dùng `http://127.0.0.1:11434`. Chọn **Lấy model** để xem các model đã tải. Để dùng preset LiquidAI, chạy lệnh sau khi Ollama đã mở.

```powershell
ollama pull hf.co/LiquidAI/LFM2.5-VL-450M-GGUF:Q8_0
```

Nếu dùng llama.cpp, chạy `llama-server` trên CPU với một slot và context 4096 token.

```powershell
llama-server --host 127.0.0.1 --port 8080 -hf LiquidAI/LFM2.5-VL-450M-GGUF:Q8_0 --alias lfm25-vl-q8 -c 4096 -np 1 -ngl 0
```

Chọn backend **llama.cpp**, endpoint `http://127.0.0.1:8080` và model `lfm25-vl-q8`. Với Translation sang tiếng Việt, chọn model hỗ trợ tiếng Việt. Preset LFM phù hợp để thử OCR và Grammar tiếng Anh.

Trong **Local AI**, bật **Chạy model LFM cùng tray** rồi bấm **Lưu Local AI** để tray tự chạy một `llama-server.exe` ở chế độ ẩn, tại `127.0.0.1:18080`, với model `lfm25-vl-q8` trên CPU. Máy cần có `llama-server.exe` cạnh file tray hoặc trong `PATH`. Lần đầu có thể tải khoảng 482 MB. Nếu khởi chạy thất bại, tắt rồi bật lại tùy chọn và lưu mỗi lần để thử lại. Thoát tray sẽ dừng phiên server do tray mở.
