//! Đưa mục vào Thùng rác qua Shell API của Windows (`SHFileOperationW`).
//!
//! Module này chỉ **mô tả** thao tác Shell thông qua [`ShellOp`]. Việc thực thi thật nằm ở
//! [`shell_delete_real`] (chỉ biên dịch trên Windows) và **không bao giờ được gọi từ test**.
//! Test truyền một [`ShellFn`] giả để kiểm tra cờ / buffer mà không đụng tới đĩa.
//!
//! # Hạn chế quan trọng cần biết
//!
//! `FOF_ALLOWUNDO` requests the Recycle Bin but is not a guarantee. We explicitly request
//! a Windows warning if recycling is unavailable and never use Shell's auto-confirm flag.
//! Cancel any OS prompt offering permanent deletion to keep the file intact.
//!
//! Ngoài ra, `SHFileOperationW` không hỗ trợ đường dẫn dài (> `MAX_PATH` = 260) hay tiền tố
//! `\\?\`. [`ShellDeleter::delete`] từ chối các path đó bằng [`DelErr`] và **không** tự đổi sang
//! xoá vĩnh viễn (đổi mode ngầm có thể làm mất đồ trái ý người dùng).

use crate::delete::{DelErr, Deleter};
use crate::model::{Item, Mode};

/// `FO_DELETE` — xoá (ở đây luôn đi kèm `FOF_ALLOWUNDO` để vào Thùng rác).
#[cfg(windows)]
const FO_DELETE: u32 = 3;

/// Shell flag: đưa vào Thùng rác thay vì xoá vĩnh viễn (khi volume cho phép — xem chú thích trên).
pub const FOF_ALLOWUNDO: u32 = 0x0040;
/// Shell flag: không hiện hộp thoại xác nhận từng mục.
pub const FOF_NOCONFIRMATION: u32 = 0x0010;
/// Shell flag: không hiện hộp thoại lỗi.
pub const FOF_NOERRORUI: u32 = 0x0400;
/// Shell flag: không hiện thanh tiến trình.
pub const FOF_SILENT: u32 = 0x0004;

/// Request a warning if Windows is unable to recycle the item.
pub const FOF_WANTNUKEWARNING: u32 = 0x4000;

/// Tổ hợp cờ dùng cho thao tác Thùng rác. No auto-yes for a permanent fallback.
pub const RECYCLE_FLAGS: u32 = FOF_ALLOWUNDO | FOF_WANTNUKEWARNING | FOF_NOERRORUI | FOF_SILENT;

/// `MAX_PATH` trên Windows.
const MAX_PATH: usize = 260;

/// Mô tả một thao tác Shell định được gọi — được truyền cho [`ShellFn`] để test có thể quan sát.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellOp {
    /// Đường dẫn hiển thị (KHÔNG có tiền tố `\\?\`), mỗi phần tử là một mục cần xoá.
    pub paths: Vec<String>,
    /// Cờ `FOF_*` truyền cho `SHFileOperationW`.
    pub flags: u32,
    /// Có yêu cầu đưa vào Thùng rác hay không; luôn `true` với [`ShellDeleter`].
    pub allow_undo: bool,
}

/// Kết quả trả về từ một lần gọi Shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellStatus {
    /// `fAnyOperationsAborted` của `SHFILEOPSTRUCTW`: có ít nhất một thao tác bị hủy.
    pub aborted: bool,
}

/// Hàm thực hiện thao tác Shell. Là `fn` pointer thuần để test có thể truyền bản giả.
pub type ShellFn = fn(&ShellOp) -> Result<ShellStatus, DelErr>;

/// `Deleter` chuyên đưa mục vào Thùng rác.
///
/// Dùng [`ShellDeleter::default`] trong release (gọi FFI thật trên Windows); trong test hãy tạo
/// trực tiếp `ShellDeleter { op: fake_fn }` để không bao giờ chạm tới Shell API.
pub struct ShellDeleter {
    pub op: ShellFn,
}

impl Default for ShellDeleter {
    fn default() -> Self {
        Self {
            op: shell_delete_real,
        }
    }
}

impl Deleter for ShellDeleter {
    /// Luôn dựng một `ShellOp` **recycle** (`FOF_ALLOWUNDO`) bất kể `mode`: deleter này chỉ làm
    /// một việc là đưa vào Thùng rác. Việc xoá vĩnh viễn thuộc về `PermanentDeleter` ở `delete.rs`.
    ///
    /// Engine đã revalidate (root / forbidden / reparse / guard) trước khi gọi đến đây.
    fn delete(&self, item: &Item, _mode: Mode) -> Result<(), DelErr> {
        if is_recycle_path_unsupported(&item.path) {
            // Không gọi op, cũng KHÔNG đổi sang xoá vĩnh viễn: đổi mode ngầm có thể mất đồ.
            return Err(DelErr(
                "đường dẫn quá dài, Thùng rác không hỗ trợ; dùng xoá vĩnh viễn".to_string(),
            ));
        }

        let op = ShellOp {
            paths: vec![item.path.clone()],
            flags: RECYCLE_FLAGS,
            allow_undo: true,
        };

        let status = (self.op)(&op)?;
        if status.aborted {
            return Err(DelErr("Thùng rác: thao tác đã bị hủy".to_string()));
        }
        Ok(())
    }
}

/// `true` nếu Shell API không thể nhận path này: quá dài hoặc đã là dạng verbatim `\\?\`.
fn is_recycle_path_unsupported(path: &str) -> bool {
    path.encode_utf16().count() > MAX_PATH || path.starts_with(r"\\?\")
}

/// Dựng buffer `PCZZWSTR`: nối các path bằng một `\0`, rồi kết thúc bằng `\0\0`.
/// Hàm thuần, không gọi FFI — test có thể gọi trực tiếp để kiểm tra cấu trúc buffer.
pub fn build_double_nul_buffer(paths: &[String]) -> Vec<u16> {
    let mut buffer = Vec::new();
    for path in paths {
        buffer.extend(path.encode_utf16());
        buffer.push(0);
    }
    buffer.push(0);
    buffer
}

/// Triển khai FFI thật. **Chỉ dùng trong release**, test không bao giờ gọi hàm này.
#[cfg(windows)]
fn shell_delete_real(op: &ShellOp) -> Result<ShellStatus, DelErr> {
    use std::ffi::c_void;
    use std::ptr;

    const COINIT_APARTMENTTHREADED: u32 = 0x2;

    /// Khớp layout C của `SHFILEOPSTRUCTW` (`FILEOP_FLAGS` là `WORD` = u16, `BOOL` = i32).
    /// Giữ nguyên tên kiểu Win32 để dễ đối chiếu với tài liệu Microsoft.
    #[allow(clippy::upper_case_acronyms)]
    #[repr(C)]
    struct SHFILEOPSTRUCTW {
        hwnd: *mut c_void,
        w_func: u32,
        p_from: *const u16,
        p_to: *const u16,
        f_flags: u16,
        f_any_operations_aborted: i32,
        h_name_mappings: *mut c_void,
        lpsz_progress_title: *const u16,
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> i32;
        fn CoUninitialize();
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHFileOperationW(lp_file_op: *mut SHFILEOPSTRUCTW) -> i32;
    }

    // `CoInitializeEx` trả HRESULT: S_OK/S_FALSE >= 0 (đã init, cần CoUninitialize), còn
    // RPC_E_CHANGED_MODE (0x80010106, âm) nghĩa là COM đã init ở mode khác → KHÔNG uninit.
    let hr = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    let need_uninit = hr >= 0;

    let buffer = build_double_nul_buffer(&op.paths);
    let mut file_op = SHFILEOPSTRUCTW {
        hwnd: ptr::null_mut(),
        w_func: FO_DELETE,
        p_from: buffer.as_ptr(),
        p_to: ptr::null(),
        f_flags: op.flags as u16,
        f_any_operations_aborted: 0,
        h_name_mappings: ptr::null_mut(),
        lpsz_progress_title: ptr::null(),
    };

    let rc = unsafe { SHFileOperationW(&mut file_op) };
    let aborted = file_op.f_any_operations_aborted != 0;

    if need_uninit {
        unsafe { CoUninitialize() };
    }

    // Mọi lỗi đều thành DelErr, không panic.
    if rc != 0 {
        return Err(DelErr(format!(
            "SHFileOperationW thất bại (mã {rc}) cho {} mục",
            op.paths.len()
        )));
    }
    if aborted {
        return Err(DelErr("Thùng rác: thao tác đã bị hủy".to_string()));
    }
    Ok(ShellStatus { aborted: false })
}

/// Trên nền tảng không phải Windows không có Shell API: trả lỗi rõ ràng, vẫn không panic.
#[cfg(not(windows))]
fn shell_delete_real(_op: &ShellOp) -> Result<ShellStatus, DelErr> {
    Err(DelErr(
        "xoá vào Thùng rác chỉ được hỗ trợ trên Windows".to_string(),
    ))
}
