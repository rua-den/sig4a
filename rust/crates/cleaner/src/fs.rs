//! Windows long-path helpers and the real (read-only) [`FsView`] implementation.

use crate::dups::FileId;
use crate::model::{EntryInfo, FsView, RealFs};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

/// `FILE_ATTRIBUTE_REPARSE_POINT` — junctions, symlinks and mount points all carry this bit.
pub const REPARSE_POINT: u32 = 0x400;

#[cfg(windows)]
unsafe extern "system" {
    fn GetDriveTypeW(root_path_name: *const u16) -> u32;
}

/// Win32 `BY_HANDLE_FILE_INFORMATION` — used because `MetadataExt::file_index` is still unstable.
#[cfg(windows)]
#[repr(C)]
struct ByHandleFileInformation {
    dw_file_attributes: u32,
    ft_creation_time: [u32; 2],
    ft_last_access_time: [u32; 2],
    ft_last_write_time: [u32; 2],
    dw_volume_serial_number: u32,
    n_file_size_high: u32,
    n_file_size_low: u32,
    n_number_of_links: u32,
    n_file_index_high: u32,
    n_file_index_low: u32,
}

#[cfg(windows)]
unsafe extern "system" {
    fn GetFileInformationByHandle(
        h_file: *mut std::ffi::c_void,
        lp_file_information: *mut ByHandleFileInformation,
    ) -> i32;
}

fn is_verbatim(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let w: Vec<u16> = path.as_os_str().encode_wide().collect();
        let q = ['\\' as u16, '\\' as u16, '?' as u16, '\\' as u16];
        let d = ['\\' as u16, '\\' as u16, '.' as u16, '\\' as u16];
        w.starts_with(&q) || w.starts_with(&d)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// Prefix an absolute path with `\\?\` for FS calls. Entry paths are converted back with
/// [`strip_verbatim`] before they ever reach an [`Item`](crate::model::Item).
pub fn to_long_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if is_verbatim(path) || !path.is_absolute() {
            return path.to_path_buf();
        }
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let is_unc = wide.starts_with(&['\\' as u16, '\\' as u16]);
        let mut out = OsString::from(r"\\?\");
        if is_unc {
            out.push("UNC");
            out.push(OsString::from_wide(&wide[2..]));
        } else {
            out.push(path.as_os_str());
        }
        PathBuf::from(out)
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

/// Inverse of [`to_long_path`] (also handles the `\\?\UNC\` form returned by `canonicalize`).
pub fn strip_verbatim(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let w: Vec<u16> = path.as_os_str().encode_wide().collect();
        let prefix = ['\\' as u16, '\\' as u16, '?' as u16, '\\' as u16];
        if !w.starts_with(&prefix) {
            return path.to_path_buf();
        }
        let rest = &w[4..];
        let unc = ['U' as u16, 'N' as u16, 'C' as u16, '\\' as u16];
        if rest.starts_with(&unc) {
            let mut out = vec!['\\' as u16, '\\' as u16];
            out.extend_from_slice(&rest[4..]);
            return PathBuf::from(OsString::from_wide(&out));
        }
        PathBuf::from(OsString::from_wide(rest))
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

#[cfg(windows)]
fn file_attributes(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

#[cfg(not(windows))]
fn file_attributes(meta: &fs::Metadata) -> u32 {
    if meta.file_type().is_symlink() {
        REPARSE_POINT
    } else {
        0
    }
}

fn modified_secs(meta: &fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

impl FsView for RealFs {
    fn entries(&self, dir: &Path) -> io::Result<Vec<EntryInfo>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(to_long_path(dir))? {
            // One unreadable child must not abort the whole directory.
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = strip_verbatim(&entry.path());
            let meta = match fs::symlink_metadata(to_long_path(&path)) {
                Ok(m) => m,
                Err(_) => continue,
            };
            out.push(EntryInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                path,
                is_dir: meta.file_type().is_dir(),
                attrs: file_attributes(&meta),
                len: meta.len(),
                modified: modified_secs(&meta),
            });
        }
        Ok(out)
    }

    fn attrs(&self, path: &Path) -> Option<u32> {
        let meta = fs::symlink_metadata(to_long_path(path)).ok()?;
        Some(file_attributes(&meta))
    }

    fn is_real_fs(&self) -> bool {
        true
    }

    fn identity(&self, path: &Path) -> Option<FileId> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            // Reparse points are filtered out before this is called, so opening the file is safe.
            let file = fs::File::open(to_long_path(path)).ok()?;
            let mut info = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
            // SAFETY: `file` owns a valid handle for the duration of the call, and `info` is a
            // sufficiently aligned, writable allocation of the exact Win32 struct size.
            let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
            if ok == 0 {
                return None;
            }
            // SAFETY: a non-zero return means the struct was fully initialised.
            let info = unsafe { info.assume_init() };
            let index = ((info.n_file_index_high as u64) << 32) | info.n_file_index_low as u64;
            Some(FileId {
                volume: info.dw_volume_serial_number as u64,
                index,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            None
        }
    }
}

/// True for UNC shares, which are rejected as roots (network paths must not be scanned).
pub(crate) fn is_unc(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        matches!(
            path.components().next(),
            Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::UNC(..))
        )
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// True only for a drive root (`D:\`) or a UNC share root. No longer used to reject *scan* roots —
/// a drive root is a legitimate opt-in root — but still guards *deletion* targets in
/// [`validate_target_with`](crate::validate_target_with).
pub(crate) fn is_root_dir(path: &Path) -> bool {
    let mut comps = path.components();
    matches!(
        (comps.next(), comps.next(), comps.next()),
        (Some(Component::Prefix(_)), Some(Component::RootDir), None)
            | (Some(Component::RootDir), None, None)
    )
}

fn normalise_component(c: Component<'_>) -> String {
    c.as_os_str().to_string_lossy().to_lowercase()
}

/// Component-wise, case-insensitive containment. `D:\studies` is NOT inside `D:\stu`.
pub fn path_contains(parent: &Path, child: &Path) -> bool {
    let a: Vec<String> = parent.components().map(normalise_component).collect();
    let b: Vec<String> = child.components().map(normalise_component).collect();
    a.len() <= b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

/// Pure classifier for Win32 `GetDriveTypeW` results: NO_ROOT (1), REMOVABLE (2),
/// REMOTE / mapped network drive (4), CD-ROM (5). Kept pure so it can be tested without a real drive.
pub fn is_blocked_drive_type(drive_type: u32) -> bool {
    matches!(drive_type, 1 | 2 | 4 | 5)
}

/// Reject removable, mapped-network, CD-ROM and non-existent drives. Callers gate this with
/// [`FsView::is_real_fs`](crate::FsView::is_real_fs), so in-memory test trees never touch Win32.
pub(crate) fn is_removable(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let prefix = path.components().next().and_then(|c| match c {
            Component::Prefix(p) => Some(p.as_os_str().to_os_string()),
            _ => None,
        });
        let Some(prefix) = prefix else { return false };
        let mut root: Vec<u16> = prefix.encode_wide().collect();
        root.push('\\' as u16);
        root.push(0);
        // SAFETY: `root` is a null-terminated UTF-16 drive root string.
        is_blocked_drive_type(unsafe { GetDriveTypeW(root.as_ptr()) })
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// Canonicalise when the path really exists, otherwise keep the caller's path (in-memory views).
pub(crate) fn canonical_for(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(p) => strip_verbatim(&p),
        Err(_) => path.to_path_buf(),
    }
}

pub(crate) fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
