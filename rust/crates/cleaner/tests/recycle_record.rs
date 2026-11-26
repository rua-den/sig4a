//! Test "record-only" cho FEATURE F: mọi `ShellFn` ở đây là hàm giả, **không gọi FFI thật**,
//! không xoá file/folder nào. Chỉ kiểm tra cờ, buffer và luồng lỗi.

use cu_cleaner::{
    build_double_nul_buffer, DelErr, Deleter, Item, Mode, RuleKind, ShellDeleter, ShellOp,
    ShellStatus, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, RECYCLE_FLAGS,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

fn fake_item(path: &str) -> Item {
    Item::new(path.to_string(), 1, RuleKind::Bin)
}

// ---------------------------------------------------------------------------
// 1) Cờ và allow_undo
// ---------------------------------------------------------------------------
static FLAGS_CALLS: AtomicUsize = AtomicUsize::new(0);
static FLAGS_SEEN: AtomicU32 = AtomicU32::new(0);
static FLAGS_UNDO: AtomicBool = AtomicBool::new(false);
static FLAGS_PATH: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn flags_op(op: &ShellOp) -> Result<ShellStatus, DelErr> {
    FLAGS_CALLS.fetch_add(1, Ordering::SeqCst);
    FLAGS_SEEN.store(op.flags, Ordering::SeqCst);
    FLAGS_UNDO.store(op.allow_undo, Ordering::SeqCst);
    *FLAGS_PATH.lock().unwrap() = op.paths.clone();
    Ok(ShellStatus { aborted: false })
}

#[test]
fn op_carries_recycle_flags_and_allow_undo() {
    let deleter = ShellDeleter { op: flags_op };
    let path = r"C:\fake\proj\bin";
    assert!(deleter.delete(&fake_item(path), Mode::Recycle).is_ok());

    assert_eq!(FLAGS_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(FLAGS_SEEN.load(Ordering::SeqCst), RECYCLE_FLAGS);
    assert_eq!(
        FLAGS_SEEN.load(Ordering::SeqCst),
        FOF_ALLOWUNDO | FOF_NOERRORUI | FOF_WANTNUKEWARNING | FOF_SILENT
    );
    assert_eq!(FLAGS_SEEN.load(Ordering::SeqCst) & FOF_NOCONFIRMATION, 0);
    assert!(FLAGS_UNDO.load(Ordering::SeqCst));
    assert_eq!(*FLAGS_PATH.lock().unwrap(), vec![path.to_string()]);
}

// ---------------------------------------------------------------------------
// 2) Buffer double-NUL (hàm thuần, không FFI)
// ---------------------------------------------------------------------------
#[test]
fn buffer_is_double_nul_terminated() {
    let paths = vec![r"C:\fake\a".to_string(), r"C:\fake\b".to_string()];
    let buffer = build_double_nul_buffer(&paths);

    let nul_count = buffer.iter().filter(|&&u| u == 0).count();
    assert_eq!(nul_count, paths.len() + 1);
    assert_eq!(buffer[buffer.len() - 2..], [0, 0]);

    let expected: Vec<u16> = paths
        .iter()
        .flat_map(|p| p.encode_utf16().chain(std::iter::once(0)))
        .chain(std::iter::once(0))
        .collect();
    assert_eq!(buffer, expected);
}

#[test]
fn buffer_of_single_path_ends_with_two_nuls() {
    let buffer = build_double_nul_buffer(&[r"C:\fake\proj\obj".to_string()]);
    assert_eq!(buffer.iter().filter(|&&u| u == 0).count(), 2);
    assert_eq!(buffer[buffer.len() - 2..], [0, 0]);
}

// ---------------------------------------------------------------------------
// 3) fAnyOperationsAborted -> DelErr
// ---------------------------------------------------------------------------
fn aborted_op(_op: &ShellOp) -> Result<ShellStatus, DelErr> {
    Ok(ShellStatus { aborted: true })
}

#[test]
fn aborted_status_becomes_del_err() {
    let deleter = ShellDeleter { op: aborted_op };
    let err = deleter
        .delete(&fake_item(r"C:\fake\proj\TestResults"), Mode::Recycle)
        .unwrap_err();
    assert!(err.0.contains("hủy"));
}

// ---------------------------------------------------------------------------
// 4) Path dài / verbatim -> Err, KHÔNG gọi op
// ---------------------------------------------------------------------------
static LONG_CALLS: AtomicUsize = AtomicUsize::new(0);

fn counting_op(_op: &ShellOp) -> Result<ShellStatus, DelErr> {
    LONG_CALLS.fetch_add(1, Ordering::SeqCst);
    Ok(ShellStatus { aborted: false })
}

#[test]
fn over_long_path_is_rejected_without_calling_op() {
    let deleter = ShellDeleter { op: counting_op };
    let long = format!(r"C:\fake\{}", "x".repeat(300));
    assert!(long.encode_utf16().count() > 260);

    let err = deleter
        .delete(&fake_item(&long), Mode::Recycle)
        .unwrap_err();
    assert_eq!(
        err.0,
        "đường dẫn quá dài, Thùng rác không hỗ trợ; dùng xoá vĩnh viễn"
    );
    assert_eq!(LONG_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn verbatim_prefixed_path_is_rejected_without_calling_op() {
    let deleter = ShellDeleter { op: counting_op };
    let err = deleter
        .delete(&fake_item(r"\\?\C:\fake\proj\bin"), Mode::Recycle)
        .unwrap_err();
    assert!(err.0.contains("quá dài"));
    assert_eq!(LONG_CALLS.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// 5) Implements Deleter, trả Ok khi op giả trả success
// ---------------------------------------------------------------------------
static OK_CALLS: AtomicUsize = AtomicUsize::new(0);

fn success_op(_op: &ShellOp) -> Result<ShellStatus, DelErr> {
    OK_CALLS.fetch_add(1, Ordering::SeqCst);
    Ok(ShellStatus { aborted: false })
}

#[test]
fn shell_deleter_is_a_deleter_and_returns_ok_on_success() {
    fn assert_deleter<T: Deleter>(_t: &T) {}
    let deleter = ShellDeleter { op: success_op };
    assert_deleter(&deleter);

    assert!(deleter
        .delete(&fake_item(r"C:\fake\proj\.vs"), Mode::Recycle)
        .is_ok());
    assert_eq!(OK_CALLS.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// 6) Lỗi từ op được lan truyền
// ---------------------------------------------------------------------------
fn failing_op(_op: &ShellOp) -> Result<ShellStatus, DelErr> {
    Err(DelErr("shell giả báo lỗi".to_string()))
}

#[test]
fn op_error_is_propagated() {
    let deleter = ShellDeleter { op: failing_op };
    let err = deleter
        .delete(&fake_item(r"C:\fake\proj\node_modules"), Mode::Recycle)
        .unwrap_err();
    assert_eq!(err.0, "shell giả báo lỗi");
}
