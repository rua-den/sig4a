# SIG4A · Kane's Tool Box

Ứng dụng Windows có khay hệ thống, công cụ Grammar / Translation / OCR chạy với Local AI, theo dõi quota Claude và xem trước các artefact có thể dọn.

## Build và mở ứng dụng trên Windows

Cần Rust stable qua `rustup`, bộ công cụ C++ MSVC x64 (Visual Studio Build Tools và Windows SDK), và [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) để mở dashboard.

Từ thư mục gốc repo:

```powershell
Set-Location rust
cargo build --locked --release --workspace
.\target\release\claude-usage.exe
```

Có thể build và đóng gói bản phát hành bằng script ở thư mục gốc repo:

```powershell
.\build.ps1
```

Nếu đang ở thư mục khác, gọi script bằng đường dẫn đầy đủ.

Script đọc version từ `rust/Cargo.toml`, build workspace release và tạo `dist\sig4a-<version>-windows.zip`. Bên trong ZIP, hai file EXE vẫn giữ tên chuẩn để tray, dashboard và hook tìm thấy nhau khi đặt cạnh nhau.

Giữ **`claude-usage.exe` và `claude-usage-dashboard.exe` cạnh nhau** khi chép sang thư mục khác. Chạy `claude-usage.exe` để mở tray, sau đó mở dashboard từ biểu tượng tray. Trang **Tổng quan** có các lối tắt. Giao diện mặc định đen–cam; dùng **Màu giao diện** trên thanh tiêu đề để chọn **Cam**, **Xanh** hoặc **Tím**. Màu được lưu cho lần mở sau. Khi thu hẹp cửa sổ, cuộn phần nội dung để tới các nút phía dưới.

## Phím tắt và công cụ văn bản

| Phím tắt | Thao tác |
| --- | --- |
| `Ctrl+Alt+G` | Kiểm tra ngữ pháp tiếng Anh |
| `Ctrl+Alt+T` | Dịch sang tiếng Việt mặc định; chọn **English** nếu cần |
| `Ctrl+Alt+O` | Mở bộ chọn vùng màn hình để nhận chữ |

Với Grammar và Translation, bôi đen văn bản rồi bấm phím tắt. Popup tự gửi phần được chọn nếu ứng dụng nguồn hỗ trợ Windows UI Automation. Nếu không lấy được vùng chọn, popup đặt con trỏ vào ô nhập; dùng `Ctrl+V` hoặc nhập tay. Có thể mở hai công cụ này từ dashboard, sửa kết quả rồi bấm **Sao chép**. Khi xử lý lỗi, dùng **Thử lại** hoặc mở **Cài đặt Local AI**; văn bản đang soạn không bị xóa.

Với OCR, kéo chọn vùng sau khi bấm phím tắt. Sau khi thả chuột, popup xuất hiện cạnh vùng đã chọn và tự nhận chữ. Bấm **Sao chép** để chép chữ nhận được, hoặc **Dịch** để dịch sang tiếng Việt ngay trong popup; **Sao chép bản dịch** chép riêng bản dịch. `Esc` trong bộ chọn sẽ hủy chụp: lần đầu không mở popup rỗng; nếu đang chụp lại từ popup, kết quả trước vẫn còn. Trong tab OCR của dashboard, cũng có thể dán ảnh PNG, JPEG hoặc WebP tối đa 6 MB, hoặc dùng **Chụp vùng màn hình**, rồi bấm **Nhận chữ**.

Mặc định popup OCR vẫn nổi khi bạn bấm sang ứng dụng khác. Đổi tại **Cài đặt → OCR Popup → Giữ popup OCR nổi khi bấm ra ngoài**, rồi bấm **Lưu cài đặt**. Thay đổi áp dụng từ lần chụp tiếp theo. Khi popup có focus, bấm `Esc` hoặc nút đóng để đóng nó.

## Cài Local AI

Mở **Local AI** từ Tổng quan hoặc nút **Local AI readiness** trong **Cài đặt**. Chọn backend, địa chỉ và model. SIG4A chỉ gửi yêu cầu đến địa chỉ HTTP loopback (ví dụ `127.0.0.1` hoặc `localhost`), không dùng endpoint hoặc model cloud. OCR cần model hỗ trợ ảnh; nếu cần dịch tiếng Việt, chọn model phù hợp với tiếng Việt.

- **[Ollama](https://docs.ollama.com/windows)**: chạy Ollama trên Windows và dùng `http://127.0.0.1:11434`. Ví dụ để tải preset LiquidAI dùng thử OCR hoặc Grammar tiếng Anh:

  ```powershell
  ollama pull hf.co/LiquidAI/LFM2.5-VL-450M-GGUF:Q8_0
  ```

- **[llama.cpp](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)**: chạy một bản `llama-server.exe` hỗ trợ model của bạn. Ví dụ CPU cho model vision có file GGUF và projector riêng; thay hai đường dẫn bằng các file tương ứng:

  ```powershell
  llama-server.exe --host 127.0.0.1 --port 8080 -m "C:\Models\vision-model.gguf" --mmproj "C:\Models\vision-mmproj.gguf" --alias local-vision -c 4096 -np 1 -ngl 0
  ```

  Trong SIG4A, chọn **llama.cpp server**, endpoint `http://127.0.0.1:8080` và ID model mà server công bố (ví dụ `local-vision`).

Bấm **Kiểm tra kết nối** hoặc **Lấy model**, chọn ID đã nạp, rồi bấm **Thử model với ảnh mẫu** và **Lưu Local AI**. Kết nối được chưa có nghĩa là model đọc được ảnh. Nếu gặp lỗi, kiểm tra server, cổng và model theo trạng thái hiển thị rồi thử lại.

Nếu muốn tray tự chạy preset LFM, đặt `llama-server.exe` cạnh file tray hoặc trong `PATH`, bật **Chạy model LFM cùng tray** rồi bấm **Lưu Local AI**. Tray dùng `127.0.0.1:18080`, model `lfm25-vl-q8`; lần đầu có thể tải model. Kiểm tra lại khi server đã sẵn sàng. Nếu tray báo không kết nối được, kiểm tra binary rồi tắt/bật tùy chọn (lưu sau mỗi lần đổi) để thử lại. Thoát tray sẽ dừng server do tray mở, không dừng server khởi chạy bên ngoài.

## Claude Usage, widget và hook

**Claude Usage** xếp theo mức cần chú ý. Mỗi profile có quota và thời điểm reset riêng cho **5h** và **7d**, lần tải gần nhất, trạng thái cập nhật và cảnh báo nếu dữ liệu cũ hoặc cần đăng nhập lại. Chạm ngưỡng không có nghĩa là prompt đang bị chặn. Nhãn **Hook sẽ chặn theo cache** dựa trên hook đã cài ở profile, lệnh chạy hợp lệ và chính sách trong cache còn hiệu lực; không phải bằng chứng một phiên CLI đã chạy hook. Khi vừa đổi chính sách, hook vẫn dùng cache hiện tại cho tới lần cập nhật tiếp theo.

Phần **Cài đặt** tách **Widget**, **Cảnh báo** và **Hook CLI** khỏi danh sách account. Có thể xem tình trạng cài hook, cài hoặc gỡ hook trong **Hook CLI**.

## Dọn ổ và file trùng

1. Vào **Dọn ổ**, nhập rồi lưu thư mục muốn quét. Cài mới không tự chọn thư mục. Kiểm tra **Phạm vi đã lưu** trước khi quét.
2. Bấm **Quét artefact**. Kiểm tra phạm vi của phiên quét, cảnh báo nếu kết quả bị cắt, đường dẫn đầy đủ, loại và dung lượng. Không mục nào được chọn sẵn.
3. Chọn mục, mở **Đưa vào Thùng rác…** và kiểm tra lại danh sách, số mục và dung lượng trước khi xác nhận. Nếu Windows báo không thể đưa vào Thùng rác và đề nghị xóa vĩnh viễn, hãy hủy nếu chỉ muốn đưa vào Thùng rác.
4. **Xoá vĩnh viễn…** là một lựa chọn riêng, có xác nhận riêng và không khôi phục qua Thùng rác. Khi đổi lựa chọn hoặc quét lại, phải xác nhận lại. Xem kết quả thành công và lỗi sau thao tác.

Mục **File trùng (chỉ đọc)** chỉ lập báo cáo; không có nút xóa và không đưa file vào danh sách artefact đang chọn.

## Kiểm thử trên Windows

`rust/rust-toolchain.toml` dùng kênh stable với target `x86_64-pc-windows-msvc`. `ui.html` và `assets/icon.ico` ở gốc repo là tài nguyên được dashboard và build script sử dụng. Từ thư mục `rust/`, chạy toàn bộ workspace, không lọc theo tên test:

```powershell
cargo check --locked --workspace --all-targets
cargo build --locked --release --workspace
cargo test --locked --workspace -- --test-threads=1
```

Chạy trên desktop Windows tương tác đã có WebView2, trong phiên kiểm thử riêng và không dùng profile thật. Fixture parity của `cu-core` cần múi giờ Windows **SE Asia Standard Time** (UTC+07:00); dùng runner riêng đã cấu hình múi giờ đó. Test debug có các đầu dò UI dành riêng cho kiểm thử; chạy smoke test bằng hai file release thông thường, không bật feature `e2e`. Bài thử TLS trực tiếp tới dịch vụ bên ngoài vốn được đánh dấu `ignore`; không cần chạy để kiểm thử fixture không có thông tin đăng nhập.

Các lệnh kiểm tra định dạng và lint bổ sung:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```
