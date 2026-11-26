# Change report

## Chưa phát hành

- Dashboard Kane's Tool Box với giao diện mặc định đen–cam, đổi màu từ thanh tiêu đề và điều hướng theo công cụ.
- Grammar, Translation và OCR qua Local AI. Phím tắt OCR mở thẳng bộ chọn vùng, sau đó hiện kết quả và bản dịch tiếng Việt ngay trong popup gần vùng chụp.
- Dọn ổ cần chọn phạm vi và xác nhận đúng danh sách artefact. Thùng rác và xoá vĩnh viễn được xác nhận riêng; báo cáo file trùng chỉ đọc.
- Claude Usage ưu tiên profile cần chú ý, hiển thị riêng các cửa sổ quota và phân biệt chạm ngưỡng với chính sách hook theo cache. Widget, cảnh báo và hook có phần cài đặt riêng.
- Hướng dẫn build, khởi động và kiểm thử trên Windows đã cập nhật trong README.

## 2.3.0 — 2026-11-26

Bản nền cho SIG4A sau khi chuyển sang repo mới:

- Nhập nguyên workspace Rust hiện có từ `claude-usage/usage4all/rust/` vào `rust/`, giữ nguyên cấu trúc và mã nguồn.
- Đưa `ui.html` và `assets/icon.ico` vào repo root để dashboard và build script tiếp tục dùng đúng đường dẫn.
- Thêm README và `.gitignore` ở root cho workflow của workspace Rust.
- Đồng bộ version workspace và các package Rust thành `2.3.0`.

Đây là baseline khởi tạo/chuyển repo; không thêm tính năng mới trong bản này.
