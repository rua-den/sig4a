//! Windows-only path resolution for the English setup helper. This is called
//! only on an explicit setup action, never from a page visit or passive poll.
use std::{
    ffi::{OsStr, OsString},
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr::{null, null_mut},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{NtOpenFile, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT},
    },
    Win32::{
        Foundation::{
            RtlNtStatusToDosError, HANDLE, INVALID_HANDLE_VALUE, STATUS_FILE_IS_A_DIRECTORY,
            STATUS_NOT_A_DIRECTORY, STATUS_NO_SUCH_FILE, STATUS_OBJECT_NAME_NOT_FOUND,
            STATUS_REPARSE_POINT_ENCOUNTERED, UNICODE_STRING,
        },
        Storage::FileSystem::{
            CreateFileW, FileAttributeTagInfo, GetFileInformationByHandleEx,
            GetFinalPathNameByHandleW, GetVolumeInformationByHandleW, QueryDosDeviceW,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
            FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ, FILE_TRAVERSE, OPEN_EXISTING,
            SYNCHRONIZE, VOLUME_NAME_DOS,
        },
        System::{
            Kernel::{OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE},
            IO::IO_STATUS_BLOCK,
        },
    },
};

const FOLDER: &str = "live-translate";
const EXE: &str = "Sig4a.LiveTranslate.exe";
const MAX_PATH_CHARS: usize = 32_768;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SetupPathError {
    MissingHelper,
    Invalid,
    AccessDenied,
}
impl SetupPathError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::MissingHelper => "backend_missing",
            Self::Invalid => "invalid_path",
            Self::AccessDenied => "access_denied",
        }
    }
}

fn invalid() -> SetupPathError {
    classify_io(io::Error::last_os_error())
}
fn classify_io(error: io::Error) -> SetupPathError {
    if error.kind() == io::ErrorKind::PermissionDenied {
        SetupPathError::AccessDenied
    } else {
        SetupPathError::Invalid
    }
}

fn classify_child_status(status: i32, helper_component: bool) -> SetupPathError {
    match status {
        STATUS_OBJECT_NAME_NOT_FOUND | STATUS_NO_SUCH_FILE if helper_component => {
            SetupPathError::MissingHelper
        }
        // Preserve the native distinction: wrong type/reparse is not an
        // ACL failure even when Windows maps it to ERROR_ACCESS_DENIED.
        STATUS_FILE_IS_A_DIRECTORY | STATUS_NOT_A_DIRECTORY | STATUS_REPARSE_POINT_ENCOUNTERED => {
            SetupPathError::Invalid
        }
        _ => classify_io(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        )),
    }
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle() as _
}

fn dos_target(drive: u8) -> Result<Vec<u16>, SetupPathError> {
    let drive = [drive as u16, b':' as u16, 0];
    let mut output = [0u16; 1024];
    let n = unsafe { QueryDosDeviceW(drive.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    if n == 0 {
        return Err(invalid());
    }
    let end = output[..n as usize]
        .iter()
        .position(|c| *c == 0)
        .ok_or(SetupPathError::Invalid)?;
    let target = &output[..end];
    let prefix = "\\Device\\HarddiskVolume"
        .encode_utf16()
        .collect::<Vec<_>>();
    // Reject UNC, redirectors, SUBST paths and device aliases before any root
    // open. Only a direct, local disk-volume mapping is accepted here.
    if !target.starts_with(&prefix)
        || target.len() == prefix.len()
        || !target[prefix.len()..].iter().all(|c| (48..=57).contains(c))
    {
        return Err(SetupPathError::Invalid);
    }
    Ok(target.to_vec())
}

fn attributes(handle: &OwnedHandle, directory: bool) -> Result<(), SetupPathError> {
    let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            raw(handle),
            FileAttributeTagInfo,
            (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(invalid());
    }
    if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(SetupPathError::Invalid);
    }
    Ok(())
}

fn wide_leaf(value: &OsStr) -> Result<Vec<u16>, SetupPathError> {
    let name: Vec<_> = value.encode_wide().collect();
    if name.is_empty()
        || name.len() > 255
        || name.iter().any(|c| [0, 47, 58, 92].contains(c))
        || name.last().is_some_and(|c| [32, 46].contains(c))
    {
        return Err(SetupPathError::Invalid);
    }
    Ok(name)
}

fn open_child(
    parent: &OwnedHandle,
    name: &OsStr,
    directory: bool,
    helper_component: bool,
) -> Result<OwnedHandle, SetupPathError> {
    let mut leaf = wide_leaf(name)?;
    let mut native: HANDLE = null_mut();
    let unicode = UNICODE_STRING {
        Length: (leaf.len() * 2) as u16,
        MaximumLength: (leaf.len() * 2) as u16,
        Buffer: leaf.as_mut_ptr(),
    };
    let obj = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: raw(parent),
        ObjectName: &unicode,
        Attributes: (OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE) as u32,
        SecurityDescriptor: null(),
        SecurityQualityOfService: null(),
    };
    let mut status_block: IO_STATUS_BLOCK = unsafe { zeroed() };
    // NTFS metadata-only handles do not participate in file sharing. Include
    // actual read/list access to enforce the no-write/no-delete share mask.
    let access = SYNCHRONIZE
        | FILE_READ_ATTRIBUTES
        | if directory {
            FILE_LIST_DIRECTORY | FILE_TRAVERSE
        } else {
            FILE_READ_DATA
        };
    let status = unsafe {
        NtOpenFile(
            &mut native,
            access,
            &obj,
            &mut status_block,
            FILE_SHARE_READ,
            // Open the exact leaf without following it; enforce both type and
            // reparse attributes on this same held handle before returning.
            FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        )
    };
    if status < 0 {
        return Err(classify_child_status(status, helper_component));
    }
    if native.is_null() || native == INVALID_HANDLE_VALUE {
        return Err(SetupPathError::Invalid);
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(native) };
    attributes(&handle, directory)?;
    Ok(handle)
}

fn pinned_root(drive: u8, mapping: &[u16]) -> Result<OwnedHandle, SetupPathError> {
    // Bind the first open to the actual device rather than repeating a
    // potentially changed drive-letter lookup.
    let path: Vec<_> = "\\\\?\\GLOBALROOT"
        .encode_utf16()
        .chain(mapping.iter().copied())
        .chain([b'\\' as u16, 0])
        .collect();
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE,
            FILE_SHARE_READ,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return Err(invalid());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(h) };
    attributes(&handle, true)?;
    let mut fs = [0u16; 32];
    if unsafe {
        GetVolumeInformationByHandleW(
            h,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    } == 0
    {
        return Err(invalid());
    }
    if fs[..5] != [78, 84, 70, 83, 0] || dos_target(drive)? != mapping {
        return Err(SetupPathError::Invalid);
    }
    Ok(handle)
}

fn canonical_local_name(handle: &OwnedHandle, drive: u8) -> Result<PathBuf, SetupPathError> {
    let mut out = vec![0u16; MAX_PATH_CHARS];
    let n = unsafe {
        GetFinalPathNameByHandleW(
            raw(handle),
            out.as_mut_ptr(),
            out.len() as u32,
            VOLUME_NAME_DOS,
        )
    } as usize;
    if n == 0 || n >= out.len() {
        return Err(invalid());
    }
    let name = PathBuf::from(OsString::from_wide(&out[..n]));
    if !matches!(name.components().next(), Some(Component::Prefix(p))
        if matches!(p.kind(), Prefix::VerbatimDisk(volume) if volume == drive))
    {
        return Err(SetupPathError::Invalid);
    }
    Ok(name)
}

pub(crate) struct SetupHelper {
    pub(crate) executable: PathBuf,
    /// Keep every directory and the executable itself open without write/delete
    /// sharing until the exact suspended child is in the supervision Job.
    guards: Vec<OwnedHandle>,
    drive: u8,
    drive_target: Vec<u16>,
}
impl SetupHelper {
    pub(crate) fn open(current_exe: &Path) -> Result<Self, SetupPathError> {
        let mut components = current_exe
            .parent()
            .ok_or(SetupPathError::Invalid)?
            .components();
        let drive = match components.next() {
            Some(Component::Prefix(p)) => match p.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
                _ => return Err(SetupPathError::Invalid),
            },
            _ => return Err(SetupPathError::Invalid),
        };
        if !matches!(components.next(), Some(Component::RootDir))
            || current_exe.as_os_str().encode_wide().count() >= MAX_PATH_CHARS
        {
            return Err(SetupPathError::Invalid);
        }

        let drive_target = dos_target(drive)?;
        let mut guards = vec![pinned_root(drive, &drive_target)?];
        for component in components {
            let Component::Normal(name) = component else {
                return Err(SetupPathError::Invalid);
            };
            guards.push(open_child(guards.last().unwrap(), name, true, false)?);
        }
        for (name, directory) in [(FOLDER, true), (EXE, false)] {
            guards.push(open_child(
                guards.last().unwrap(),
                OsStr::new(name),
                directory,
                true,
            )?);
        }
        let executable = canonical_local_name(guards.last().unwrap(), drive)?;
        let result = Self {
            executable,
            guards,
            drive,
            drive_target,
        };
        result.confirm()?;
        Ok(result)
    }

    pub(crate) fn confirm(&self) -> Result<(), SetupPathError> {
        for (i, handle) in self.guards.iter().enumerate() {
            attributes(handle, i + 1 != self.guards.len())?;
        }
        if dos_target(self.drive)? != self.drive_target
            || canonical_local_name(self.guards.last().unwrap(), self.drive)? != self.executable
        {
            return Err(SetupPathError::Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    fn junction(from: &Path, to: &Path) {
        let status = Command::new("cmd")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(from)
            .arg(to)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "fixture could not create local NTFS junction"
        );
    }

    #[test]
    fn native_wrong_type_and_reparse_are_not_reported_as_permissions_or_absence() {
        use windows_sys::Win32::Foundation::STATUS_ACCESS_DENIED;
        for code in [
            STATUS_FILE_IS_A_DIRECTORY,
            STATUS_NOT_A_DIRECTORY,
            STATUS_REPARSE_POINT_ENCOUNTERED,
        ] {
            assert_eq!(classify_child_status(code, true), SetupPathError::Invalid);
        }
        assert_eq!(
            classify_child_status(STATUS_ACCESS_DENIED, true),
            SetupPathError::AccessDenied
        );
        assert_eq!(
            classify_child_status(STATUS_OBJECT_NAME_NOT_FOUND, true),
            SetupPathError::MissingHelper
        );
    }

    #[test]
    fn helper_resolution_never_uses_unc_or_unpinned_reparse_points() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let app = root.join("local");
        let target = root.join("other");
        fs::create_dir(&app).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(target.join(EXE), b"fake").unwrap();
        assert_eq!(
            SetupHelper::open(Path::new(r"\\missing-host.invalid\share\dash.exe")).err(),
            Some(SetupPathError::Invalid),
        );
        assert_eq!(
            SetupHelper::open(&app.join("dash.exe")).err(),
            Some(SetupPathError::MissingHelper)
        );
        junction(&app.join(FOLDER), &target);
        assert_eq!(
            SetupHelper::open(&app.join("dash.exe")).err(),
            Some(SetupPathError::Invalid)
        );
        fs::remove_dir(app.join(FOLDER)).unwrap();
        let fixed = app.join(FOLDER);
        fs::create_dir(&fixed).unwrap();
        let indirect = root.join("indirect");
        junction(&indirect, &app);
        assert_eq!(
            SetupHelper::open(&indirect.join("dash.exe")).err(),
            Some(SetupPathError::Invalid)
        );
        fs::remove_dir(indirect).unwrap();
        fs::create_dir(fixed.join(EXE)).unwrap();
        assert_eq!(
            SetupHelper::open(&app.join("dash.exe")).err(),
            Some(SetupPathError::Invalid)
        );
        fs::remove_dir(fixed.join(EXE)).unwrap();
        junction(&fixed.join(EXE), &target);
        // Some Windows filters reject the leaf itself with generic access
        // denied. Either bounded refusal means no executable can escape.
        assert!(matches!(
            SetupHelper::open(&app.join("dash.exe")).err(),
            Some(SetupPathError::Invalid | SetupPathError::AccessDenied)
        ));
        fs::remove_dir(fixed.join(EXE)).unwrap();
        match std::os::windows::fs::symlink_file(target.join(EXE), fixed.join(EXE)) {
            Ok(()) => {
                assert_eq!(
                    SetupHelper::open(&app.join("dash.exe")).err(),
                    Some(SetupPathError::Invalid)
                );
                fs::remove_file(fixed.join(EXE)).unwrap();
            }
            Err(e) if e.raw_os_error() == Some(1314) => {
                eprintln!("File symlink privilege unavailable; NTFS junction cases still ran.");
            }
            Err(e) => panic!("Could not create fixture symlink: {e}"),
        }
        fs::write(fixed.join(EXE), b"fixed local test stub").unwrap();
        let helper = SetupHelper::open(&app.join("dash.exe")).unwrap();
        assert!(fs::OpenOptions::new()
            .write(true)
            .open(fixed.join(EXE))
            .is_err());
        assert!(fs::rename(fixed.join(EXE), fixed.join("rename.exe")).is_err());
        assert!(fs::rename(&fixed, app.join("replacement")).is_err());
        assert!(fs::rename(&app, root.join("moving")).is_err());
        helper.confirm().unwrap();
        drop(helper);
        drop(
            fs::OpenOptions::new()
                .write(true)
                .open(fixed.join(EXE))
                .unwrap(),
        );
        fs::rename(fixed.join(EXE), fixed.join("rename.exe")).unwrap();
        assert_eq!(fs::read(target.join(EXE)).unwrap(), b"fake");
    }
}
