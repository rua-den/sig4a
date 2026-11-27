//! Narrow Windows trust checks for the optional one-click local AI setup.
//! This module intentionally never discovers executables through PATH/CWD.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustError {
    OsApi,
    PackageNotRegistered,
    UntrustedPath,
    UntrustedSignature,
    ListenerOwnershipUnknown,
    ListenerNotLoopback,
}

impl std::fmt::Display for TrustError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OsApi => "Windows could not verify the local AI installation",
            Self::PackageNotRegistered => "The trusted Windows Package Manager is not installed",
            Self::UntrustedPath => {
                "The local Ollama executable is not in its trusted install location"
            }
            Self::UntrustedSignature => "Windows could not verify the Ollama signature",
            Self::ListenerOwnershipUnknown => {
                "Windows could not identify the process on Ollama's port"
            }
            Self::ListenerNotLoopback => "Ollama's port is occupied by a non-loopback listener",
        })
    }
}

#[cfg(any(windows, test))]
fn package_name_in_buffer(buffer: &[u16], name: *const u16) -> Option<&[u16]> {
    let byte_offset = (name as usize).checked_sub(buffer.as_ptr() as usize)?;
    let char_size = std::mem::size_of::<u16>();
    if byte_offset % char_size != 0 {
        return None;
    }
    // Index the original slice instead of dereferencing a pointer returned by the OS.
    let remaining = buffer.get(byte_offset / char_size..)?;
    let end = remaining.iter().position(|c| *c == 0)?;
    (end != 0).then(|| &remaining[..=end])
}

#[cfg(test)]
mod tests {
    use super::package_name_in_buffer;

    #[test]
    fn package_name_at_nonzero_utf16_offset() {
        let names = [65, 0, 66, 67, 0];
        assert_eq!(
            package_name_in_buffer(&names, names.as_ptr().wrapping_add(2)),
            Some(&names[2..])
        );
    }

    #[test]
    fn rejects_misaligned_or_out_of_bounds_package_name() {
        let names = [65, 0, 66];
        let base = names.as_ptr();
        let misaligned = base.cast::<u8>().wrapping_add(1).cast::<u16>();
        assert!(package_name_in_buffer(&names, misaligned).is_none());
        assert!(package_name_in_buffer(&names, base.wrapping_sub(1)).is_none());
        assert!(package_name_in_buffer(&names, base.wrapping_add(names.len())).is_none());
        // Reject empty and unterminated names even at a valid UTF-16 offset.
        assert!(package_name_in_buffer(&names, base.wrapping_add(1)).is_none());
        assert!(package_name_in_buffer(&names, base.wrapping_add(2)).is_none());
    }
}

#[cfg(windows)]
mod imp {
    use super::{package_name_in_buffer, Path, PathBuf, TrustError};
    use std::{
        mem::{size_of, zeroed},
        os::windows::{ffi::OsStrExt, fs::MetadataExt},
        path::{Component, Prefix},
        ptr, slice,
    };
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, FILETIME, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
        },
        NetworkManagement::IpHelper::{
            GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
            TCP_TABLE_OWNER_PID_CONNECTIONS, TCP_TABLE_OWNER_PID_LISTENER,
        },
        Networking::WinSock::{AF_INET, AF_INET6},
        Security::{
            Cryptography::CertGetNameStringW,
            WinTrust::{
                WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain,
                WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2,
                WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL,
                WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
                WTD_UI_NONE,
            },
        },
        Storage::{
            FileSystem::{
                CreateFileW, GetDriveTypeW, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
                FILE_SHARE_READ, OPEN_EXISTING,
            },
            Packaging::Appx::{GetPackagePathByFullName, GetPackagesByPackageFamily},
        },
        System::Threading::{
            GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        },
    };

    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const ERROR_NO_DATA: u32 = 232;
    const TCP_STATE_LISTEN: u32 = 2;
    const APP_INSTALLER_FAMILY: &str = "Microsoft.DesktopAppInstaller_8wekyb3d8bbwe";
    const OLLAMA_ORG: &str = "Ollama Inc.";

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }
    fn win_path(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn nul_string(chars: &[u16]) -> String {
        String::from_utf16_lossy(chars.split(|c| *c == 0).next().unwrap_or(chars))
    }

    /// Resolve winget only from the OS registered Microsoft Desktop App Installer package.
    /// This avoids PATH aliases, current-directory executables, and lookalike package names.
    pub fn resolve_winget() -> Result<PathBuf, TrustError> {
        unsafe {
            let family = win_path(APP_INSTALLER_FAMILY);
            let mut count = 0u32;
            let mut chars = 0u32;
            let first = GetPackagesByPackageFamily(
                family.as_ptr(),
                &mut count,
                ptr::null_mut(),
                &mut chars,
                ptr::null_mut(),
            );
            if first != ERROR_INSUFFICIENT_BUFFER || count == 0 || chars == 0 {
                return Err(TrustError::PackageNotRegistered);
            }
            let mut names = vec![ptr::null_mut(); count as usize];
            let mut name_buf = vec![0u16; chars as usize];
            let status = GetPackagesByPackageFamily(
                family.as_ptr(),
                &mut count,
                names.as_mut_ptr(),
                &mut chars,
                name_buf.as_mut_ptr(),
            );
            if status != 0 {
                return Err(TrustError::OsApi);
            }
            let names = names.get(..count as usize).ok_or(TrustError::OsApi)?;
            let name_buf = name_buf.get(..chars as usize).ok_or(TrustError::OsApi)?;
            let mut found = Vec::new();
            for &name in names {
                let Some(full) = package_name_in_buffer(name_buf, name) else {
                    continue;
                };
                let mut n = 0u32;
                let first = GetPackagePathByFullName(full.as_ptr(), &mut n, ptr::null_mut());
                if first != ERROR_INSUFFICIENT_BUFFER || n == 0 {
                    continue;
                }
                let mut path = vec![0u16; n as usize];
                if GetPackagePathByFullName(full.as_ptr(), &mut n, path.as_mut_ptr()) != 0 {
                    continue;
                }
                let root = PathBuf::from(nul_string(&path));
                if !root.is_absolute()
                    || !root
                        .components()
                        .any(|c| c.as_os_str().eq_ignore_ascii_case("WindowsApps"))
                {
                    continue;
                }
                let exe = root.join("winget.exe");
                if exe.is_file() {
                    found.push(exe);
                }
            }
            // Multiple registered versions may coexist. Pick the lexically newest package directory;
            // every candidate belongs to the exact OS package family above.
            found.sort();
            found.pop().ok_or(TrustError::PackageNotRegistered)
        }
    }

    pub struct VerifiedOllama {
        path: PathBuf,
        _locked_file: HANDLE,
    }
    impl VerifiedOllama {
        pub fn path(&self) -> &Path {
            &self.path
        }
    }
    impl Drop for VerifiedOllama {
        fn drop(&mut self) {
            unsafe {
                if !self._locked_file.is_null() && self._locked_file != INVALID_HANDLE_VALUE {
                    CloseHandle(self._locked_file);
                }
            }
        }
    }

    /// Recognize only the documented per-user install path. The open handle denies write/delete
    /// sharing and remains live in `VerifiedOllama`, preventing replacement between verification
    /// and the subsequent child launch.
    pub fn resolve_ollama() -> Result<Option<VerifiedOllama>, TrustError> {
        resolve_ollama_with_trust(false)
    }

    pub fn resolve_ollama_cache_only() -> Result<Option<VerifiedOllama>, TrustError> {
        resolve_ollama_with_trust(true)
    }

    fn resolve_ollama_with_trust(cache_only: bool) -> Result<Option<VerifiedOllama>, TrustError> {
        let local = std::env::var_os("LOCALAPPDATA").ok_or(TrustError::UntrustedPath)?;
        let root = PathBuf::from(local).join("Programs").join("Ollama");
        if !root.is_absolute() {
            return Err(TrustError::UntrustedPath);
        }
        let path = root.join("ollama.exe");
        if cache_only && !diagnostic_local_path(&path) {
            return Err(TrustError::UntrustedPath);
        }
        match path.try_exists() {
            Ok(false) => return Ok(None),
            Err(_) => return Err(TrustError::UntrustedPath),
            Ok(true) => {}
        }
        verify_ollama_exe_with_trust(&path, cache_only).map(Some)
    }

    fn verify_ollama_exe_with_trust(
        path: &Path,
        cache_only: bool,
    ) -> Result<VerifiedOllama, TrustError> {
        let local = std::env::var_os("LOCALAPPDATA").ok_or(TrustError::UntrustedPath)?;
        let expected = PathBuf::from(local)
            .join("Programs")
            .join("Ollama")
            .join("ollama.exe");
        if !path.is_absolute()
            || !same_path(path, &expected)
            || std::fs::symlink_metadata(path)
                .map_err(|_| TrustError::UntrustedPath)?
                .file_type()
                .is_symlink()
        {
            return Err(TrustError::UntrustedPath);
        }
        let path = std::fs::canonicalize(path).map_err(|_| TrustError::UntrustedPath)?;
        let expected = std::fs::canonicalize(expected).map_err(|_| TrustError::UntrustedPath)?;
        if !same_path(&path, &expected) {
            return Err(TrustError::UntrustedPath);
        }
        let path_w = wide(path.as_os_str());
        unsafe {
            let handle = CreateFileW(
                path_w.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            );
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(TrustError::UntrustedPath);
            }
            if !verify_signature(handle, &path_w, cache_only) {
                CloseHandle(handle);
                return Err(TrustError::UntrustedSignature);
            }
            Ok(VerifiedOllama {
                path,
                _locked_file: handle,
            })
        }
    }

    // Do not traverse mapped/network volumes or reparse paths for a no-egress
    // diagnostic (even filesystem discovery could otherwise trigger SMB).
    fn diagnostic_local_path(path: &Path) -> bool {
        let mut parts = path.components();
        let Some(Component::Prefix(prefix)) = parts.next() else {
            return false;
        };
        if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
            || !matches!(parts.next(), Some(Component::RootDir))
        {
            return false;
        }
        let mut root = PathBuf::from(prefix.as_os_str());
        root.push(r"\");
        let drive = wide(root.as_os_str());
        const DRIVE_FIXED: u32 = 3;
        if unsafe { GetDriveTypeW(drive.as_ptr()) } != DRIVE_FIXED {
            return false;
        }
        for part in parts {
            if !matches!(part, Component::Normal(_)) {
                return false;
            }
            root.push(part.as_os_str());
            match std::fs::symlink_metadata(&root) {
                Ok(m) if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 => (),
                _ => return false,
            }
        }
        true
    }

    fn same_path(a: &Path, b: &Path) -> bool {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }

    unsafe fn verify_signature(handle: HANDLE, path: &[u16], cache_only: bool) -> bool {
        let mut file: WINTRUST_FILE_INFO = zeroed();
        file.cbStruct = size_of::<WINTRUST_FILE_INFO>() as u32;
        file.pcwszFilePath = path.as_ptr();
        file.hFile = handle;
        let mut data: WINTRUST_DATA = zeroed();
        data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        if cache_only {
            data.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL;
        }
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let verified = WinVerifyTrust(
            ptr::null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        ) == 0;
        let state = data.hWVTStateData;
        let publisher_ok = if verified && !state.is_null() {
            let provider = WTHelperProvDataFromStateData(state);
            if provider.is_null() {
                false
            } else {
                let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
                if signer.is_null() || (*signer).csCertChain == 0 {
                    false
                } else {
                    let cert = WTHelperGetProvCertFromChain(signer, 0);
                    if cert.is_null() || (*cert).pCert.is_null() {
                        false
                    } else {
                        // 2.5.4.10 is the OrganizationName OID; query the exact
                        // signer cert returned by the successful WinVerifyTrust chain.
                        let oid = b"2.5.4.10\0";
                        let needed = CertGetNameStringW(
                            (*cert).pCert,
                            3,
                            0,
                            oid.as_ptr().cast(),
                            ptr::null_mut(),
                            0,
                        );
                        if needed <= 1 || needed >= 512 {
                            false
                        } else {
                            let mut name = vec![0u16; needed as usize];
                            let written = CertGetNameStringW(
                                (*cert).pCert,
                                3,
                                0,
                                oid.as_ptr().cast(),
                                name.as_mut_ptr(),
                                needed,
                            );
                            written > 1 && nul_string(&name) == OLLAMA_ORG
                        }
                    }
                }
            }
        } else {
            false
        };
        if !state.is_null() {
            data.dwStateAction = WTD_STATEACTION_CLOSE;
            let _ = WinVerifyTrust(
                ptr::null_mut(),
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            );
        }
        verified && publisher_ok
    }

    pub struct ListenerOwner {
        pid: u32,
        image: PathBuf,
        _process: HANDLE,
    }
    impl ListenerOwner {
        pub fn pid(&self) -> u32 {
            self.pid
        }
        pub fn matches(&self, ollama: &VerifiedOllama) -> bool {
            self.is_alive() && same_path(&self.image, ollama.path())
        }
        pub fn creation_identity(&self) -> Option<u64> {
            let zero = FILETIME {
                dwHighDateTime: 0,
                dwLowDateTime: 0,
            };
            let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
            // Synchronization against the retained process is stricter than STILL_ACTIVE,
            // whose numeric exit code is not reserved by the kernel.
            if unsafe { WaitForSingleObject(self._process, 0) } != WAIT_TIMEOUT
                || unsafe {
                    GetProcessTimes(
                        self._process,
                        &mut creation,
                        &mut exit,
                        &mut kernel,
                        &mut user,
                    )
                } == 0
            {
                return None;
            }
            let creation = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
            (creation != 0).then_some(creation)
        }
        pub fn is_alive(&self) -> bool {
            use windows_sys::Win32::System::Threading::GetExitCodeProcess;
            let mut code = 0u32;
            unsafe {
                GetExitCodeProcess(self._process, &mut code) != 0 && code == 259
                /* STILL_ACTIVE */
            }
        }
    }
    impl Drop for ListenerOwner {
        fn drop(&mut self) {
            unsafe {
                if !self._process.is_null() {
                    CloseHandle(self._process);
                }
            }
        }
    }

    /// Inspect OS TCP tables for the default Ollama port. A wildcard/non-loopback listener
    /// (including IPv6 wildcard) is never accepted as a safe pre-existing backend.
    pub fn loopback_listener_owner() -> Result<Option<ListenerOwner>, TrustError> {
        listener_owner_on_port(11434)
    }

    fn listener_owner_on_port(port: u16) -> Result<Option<ListenerOwner>, TrustError> {
        listener_owner_on_port_with_rights(port, false)
    }

    pub fn diagnostic_listener_owner() -> Result<Option<ListenerOwner>, TrustError> {
        listener_owner_on_port_with_rights(11434, true)
    }

    fn listener_owner_on_port_with_rights(
        port: u16,
        diagnostic: bool,
    ) -> Result<Option<ListenerOwner>, TrustError> {
        unsafe {
            let mut found: Option<u32> = None;
            let mut ipv6_loopback_pids = Vec::new();
            let rows4 = tcp_rows(AF_INET as u32)?;
            for row in rows4 {
                if row.dwState != TCP_STATE_LISTEN || u16::from_be(row.dwLocalPort as u16) != port {
                    continue;
                }
                let ip = row.dwLocalAddr.to_ne_bytes();
                if ip != [127, 0, 0, 1] {
                    return Err(TrustError::ListenerNotLoopback);
                }
                if found.replace(row.dwOwningPid).is_some() {
                    return Err(TrustError::ListenerOwnershipUnknown);
                }
            }
            let rows6 = tcp6_rows(AF_INET6 as u32)?;
            for row in rows6 {
                if row.dwState != TCP_STATE_LISTEN || u16::from_be(row.dwLocalPort as u16) != port {
                    continue;
                }
                if row.ucLocalAddr != [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1] {
                    return Err(TrustError::ListenerNotLoopback);
                }
                ipv6_loopback_pids.push(row.dwOwningPid);
            }
            let Some(pid) = found else {
                return if ipv6_loopback_pids.is_empty() {
                    Ok(None)
                } else {
                    Err(TrustError::ListenerNotLoopback)
                };
            };
            if ipv6_loopback_pids.iter().any(|other| *other != pid) {
                return Err(TrustError::ListenerOwnershipUnknown);
            }
            let rights = PROCESS_QUERY_LIMITED_INFORMATION
                | if diagnostic { PROCESS_SYNCHRONIZE } else { 0 };
            let process = OpenProcess(rights, 0, pid);
            if process.is_null() {
                return Err(TrustError::ListenerOwnershipUnknown);
            }
            let mut name = vec![0u16; 32768];
            let mut n = name.len() as u32;
            if QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut n) == 0 {
                CloseHandle(process);
                return Err(TrustError::ListenerOwnershipUnknown);
            }
            name.truncate(n as usize);
            let image_raw = PathBuf::from(String::from_utf16_lossy(&name));
            if diagnostic && !diagnostic_local_path(&image_raw) {
                CloseHandle(process);
                return Err(TrustError::UntrustedPath);
            }
            let image = std::fs::canonicalize(&image_raw).map_err(|_| {
                CloseHandle(process);
                TrustError::ListenerOwnershipUnknown
            })?;
            if !image.is_absolute() {
                CloseHandle(process);
                return Err(TrustError::ListenerOwnershipUnknown);
            }
            Ok(Some(ListenerOwner {
                pid,
                image,
                _process: process,
            }))
        }
    }

    #[cfg(test)]
    mod windows_tests {
        use super::{listener_owner_on_port, TrustError};
        use std::net::{Ipv4Addr, TcpListener};

        #[test]
        fn windows_listener_owner_identifies_current_process_on_loopback() {
            let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback");
            let port = socket.local_addr().expect("query loopback address").port();
            let owner = listener_owner_on_port(port)
                .expect("query Windows TCP listener owner")
                .expect("Windows TCP table contains the loopback listener");
            assert_eq!(owner.pid(), std::process::id());
            assert!(owner.is_alive());
            assert!(!owner.image.as_os_str().is_empty());
        }

        #[test]
        fn windows_listener_owner_rejects_wildcard_listener() {
            let socket = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("bind wildcard");
            let port = socket.local_addr().expect("query wildcard address").port();
            assert!(matches!(
                listener_owner_on_port(port),
                Err(TrustError::ListenerNotLoopback)
            ));
        }
    }

    pub fn connected_rows_v4() -> Result<Vec<MIB_TCPROW_OWNER_PID>, TrustError> {
        unsafe { tcp_rows_class(AF_INET as u32, TCP_TABLE_OWNER_PID_CONNECTIONS) }
    }

    unsafe fn tcp_rows(family: u32) -> Result<Vec<MIB_TCPROW_OWNER_PID>, TrustError> {
        tcp_rows_class(family, TCP_TABLE_OWNER_PID_LISTENER)
    }

    unsafe fn tcp_rows_class(
        family: u32,
        class: i32,
    ) -> Result<Vec<MIB_TCPROW_OWNER_PID>, TrustError> {
        let mut size = 0u32;
        let first = GetExtendedTcpTable(ptr::null_mut(), &mut size, 0, family, class, 0);
        if first != ERROR_INSUFFICIENT_BUFFER || !(4..=1_048_576).contains(&size) {
            return Err(TrustError::OsApi);
        }
        let mut data = vec![0u64; (size as usize + 7) / 8];
        let status = GetExtendedTcpTable(data.as_mut_ptr().cast(), &mut size, 0, family, class, 0);
        if status != 0 || size as usize > data.len() * 8 || size < 4 {
            return Err(TrustError::OsApi);
        }
        let base = data.as_ptr().cast::<u8>();
        let count = *(base.cast::<u32>()) as usize;
        if count > 8192 || 4 + count * size_of::<MIB_TCPROW_OWNER_PID>() > size as usize {
            return Err(TrustError::OsApi);
        }
        Ok(slice::from_raw_parts(base.add(4).cast::<MIB_TCPROW_OWNER_PID>(), count).to_vec())
    }

    unsafe fn tcp6_rows(family: u32) -> Result<Vec<MIB_TCP6ROW_OWNER_PID>, TrustError> {
        let mut size = 0u32;
        let first = GetExtendedTcpTable(
            ptr::null_mut(),
            &mut size,
            0,
            family,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if first != ERROR_INSUFFICIENT_BUFFER && first != ERROR_NO_DATA {
            return Err(TrustError::OsApi);
        }
        if first == ERROR_NO_DATA {
            return Ok(Vec::new());
        }
        if !(4..=1_048_576).contains(&size) {
            return Err(TrustError::OsApi);
        }
        let mut data = vec![0u64; (size as usize + 7) / 8];
        let status = GetExtendedTcpTable(
            data.as_mut_ptr().cast(),
            &mut size,
            0,
            family,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if status != 0 {
            return Err(TrustError::OsApi);
        }
        let base = data.as_ptr().cast::<u8>();
        let count = *(base.cast::<u32>()) as usize;
        if count > 8192 || 4 + count * size_of::<MIB_TCP6ROW_OWNER_PID>() > size as usize {
            return Err(TrustError::OsApi);
        }
        Ok(slice::from_raw_parts(base.add(4).cast::<MIB_TCP6ROW_OWNER_PID>(), count).to_vec())
    }
}

#[cfg(windows)]
pub use imp::{
    connected_rows_v4, diagnostic_listener_owner, loopback_listener_owner, resolve_ollama,
    resolve_ollama_cache_only, resolve_winget, ListenerOwner, VerifiedOllama,
};

#[cfg(not(windows))]
pub fn resolve_winget() -> Result<PathBuf, TrustError> {
    Err(TrustError::OsApi)
}
#[cfg(not(windows))]
pub fn resolve_ollama() -> Result<Option<VerifiedOllama>, TrustError> {
    Err(TrustError::OsApi)
}
#[cfg(not(windows))]
pub fn loopback_listener_owner() -> Result<Option<ListenerOwner>, TrustError> {
    Err(TrustError::OsApi)
}
#[cfg(not(windows))]
pub struct VerifiedOllama {
    path: PathBuf,
}
#[cfg(not(windows))]
impl VerifiedOllama {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
#[cfg(not(windows))]
pub struct ListenerOwner;
