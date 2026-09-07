//! Win32 handles bind validation and I/O to the same object. Creation supplies a
//! protected DACL atomically; chmod and post-creation ACL repair are not used.
use super::*;
use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
};

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct User {
    _buffer: Vec<usize>,
    sid: PSID,
}
impl User {
    fn current() -> io::Result<Self> {
        unsafe {
            let mut token = null_mut();
            check(OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY,
                &mut token,
            ))?;
            let mut needed = 0;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed);
            let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
            let result = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            );
            let error = io::Error::last_os_error();
            CloseHandle(token);
            if result == 0 {
                return Err(error);
            }
            let sid = (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid;
            Ok(Self {
                _buffer: buffer,
                sid,
            })
        }
    }
    fn descriptor(&self, directory: bool) -> io::Result<LocalAllocation> {
        unsafe {
            let mut text = null_mut();
            check(ConvertSidToStringSidW(self.sid, &mut text))?;
            let allocation = LocalAllocation(text.cast());
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let sid = String::from_utf16(std::slice::from_raw_parts(text, length))
                .map_err(|_| unsafe_file())?;
            drop(allocation);
            let inherit = if directory { "OICI" } else { "" };
            let sddl = format!("O:{sid}D:P(A;{inherit};FA;;;{sid})(A;{inherit};FA;;;SY)");
            let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            let mut descriptor = null_mut();
            check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            ))?;
            Ok(LocalAllocation(descriptor))
        }
    }
}
fn check(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    if !path.is_absolute()
        || !matches!(path.components().next(), Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::Disk(_)))
    {
        return Err(unsafe_file());
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    // Reject alternate data streams and device namespaces, as well as NULs.
    if wide.contains(&0)
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || path
            .as_os_str()
            .to_string_lossy()
            .get(2..)
            .is_some_and(|p| p.contains(':'))
    {
        return Err(unsafe_file());
    }
    Ok(wide.into_iter().chain(Some(0)).collect())
}
fn information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    unsafe {
        let mut info = zeroed();
        check(GetFileInformationByHandle(file.as_raw_handle(), &mut info))?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(unsafe_file());
        }
        Ok(info)
    }
}
/// Refuse traversal through junctions/symlinks, including intermediate components.
fn check_parents(path: &Path) -> io::Result<()> {
    for parent in path.ancestors().skip(1).filter(|p| p.parent().is_some()) {
        let file = raw_open(
            parent,
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            OPEN_EXISTING,
            null(),
        )?;
        information(&file)?;
    }
    Ok(())
}
fn raw_open(
    path: &Path,
    access: u32,
    disposition: u32,
    attributes: *const SECURITY_ATTRIBUTES,
) -> io::Result<File> {
    unsafe {
        let path = wide(path)?;
        let handle = CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            attributes,
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(File::from_raw_handle(handle))
    }
}
fn verify_acl(file: &File) -> io::Result<()> {
    unsafe {
        let handle = file.as_raw_handle();
        let mut flags = 0;
        check(GetVolumeInformationByHandleW(
            handle,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &mut flags,
            null_mut(),
            0,
        ))?;
        if flags & 0x8 == 0 {
            return Err(unsafe_file());
        } // FILE_PERSISTENT_ACLS
        let user = User::current()?;
        let mut owner = null_mut();
        let mut acl = null_mut();
        let mut sd = null_mut();
        let status = GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut sd,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _allocation = LocalAllocation(sd);
        let mut control = 0;
        let mut revision = 0;
        check(GetSecurityDescriptorControl(
            sd,
            &mut control,
            &mut revision,
        ))?;
        if owner.is_null()
            || EqualSid(owner, user.sid) == 0
            || acl.is_null()
            || control & SE_DACL_PROTECTED == 0
        {
            return Err(unsafe_file());
        }
        let mut has_user = false;
        for index in 0..(*acl).AceCount {
            let mut ace = null_mut();
            check(GetAce(acl, index as u32, &mut ace))?;
            let header = &*ace.cast::<ACE_HEADER>();
            // Only explicit allow entries for this user or LocalSystem. Unknown,
            // inherited, callback, object-specific and deny ACEs fail closed.
            if header.AceType != 0
                || (header.AceFlags as u32) & (INHERITED_ACE | INHERIT_ONLY_ACE) != 0
            {
                return Err(unsafe_file());
            }
            let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            let sid = (&allowed.SidStart as *const u32).cast_mut().cast();
            if IsValidSid(sid) == 0 {
                return Err(unsafe_file());
            }
            if EqualSid(sid, user.sid) != 0 {
                has_user = true;
            } else if IsWellKnownSid(sid, WinLocalSystemSid) == 0 {
                return Err(unsafe_file());
            }
        }
        if !has_user {
            return Err(unsafe_file());
        }
        Ok(())
    }
}
pub fn verify_file(file: &File) -> io::Result<()> {
    let info = information(file)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 || info.nNumberOfLinks != 1 {
        return Err(unsafe_file());
    }
    verify_acl(file)
}
pub fn open(path: &Path, mode: OpenMode) -> io::Result<File> {
    check_parents(path)?;
    let user = User::current()?;
    let descriptor = user.descriptor(false)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let access = GENERIC_READ
        | if matches!(mode, OpenMode::Read) {
            0
        } else {
            GENERIC_WRITE
        };
    let disposition = match mode {
        OpenMode::Read | OpenMode::ReadWrite => OPEN_EXISTING,
        OpenMode::OpenOrCreate => OPEN_ALWAYS,
        OpenMode::CreateNew => CREATE_NEW,
    };
    let file = raw_open(path, access, disposition, &attributes)?;
    verify_file(&file)?;
    Ok(file)
}
pub fn verify_directory(path: &Path) -> io::Result<()> {
    check_parents(path)?;
    let file = raw_open(
        path,
        FILE_READ_ATTRIBUTES | READ_CONTROL,
        OPEN_EXISTING,
        null(),
    )?;
    if information(&file)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(unsafe_file());
    }
    verify_acl(&file)
}
pub fn create_directory(path: &Path) -> io::Result<()> {
    if path.exists() {
        return verify_directory(path);
    }
    let parent = path.parent().ok_or_else(unsafe_file)?;
    if !parent.exists() {
        create_directory(parent)?;
    }
    check_parents(path)?;
    let descriptor = User::current()?.descriptor(true)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let result = unsafe { CreateDirectoryW(wide(path)?.as_ptr(), &attributes) };
    if result == 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
        return Err(io::Error::last_os_error());
    }
    verify_directory(path)
}
/// Windows does not expose Unix directory fsync semantics. Files are flushed
/// before publication; replacement requests the platform's write-through move.
pub fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}
pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    check_parents(from)?;
    check_parents(to)?;
    unsafe {
        check(MoveFileExW(
            wide(from)?.as_ptr(),
            wide(to)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::symlink_file;
    fn replace_acl(file: &File, sddl: &str) {
        unsafe {
            let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            let mut sd = null_mut();
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut sd,
                    null_mut()
                ),
                0
            );
            let _allocation = LocalAllocation(sd);
            let mut present = 0;
            let mut defaulted = 0;
            let mut acl = null_mut();
            assert_ne!(
                GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted),
                0
            );
            assert_eq!(
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    acl,
                    null_mut()
                ),
                0
            );
        }
    }
    #[test]
    fn rejects_broad_or_inherited_permissions_and_reparse_points() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private");
        create_directory(&directory).unwrap();
        let path = directory.join("token");
        drop(open(&path, OpenMode::CreateNew).unwrap());
        let handle = raw_open(&path, GENERIC_READ | WRITE_DAC, OPEN_EXISTING, null()).unwrap();
        replace_acl(&handle, "D:P(A;;FA;;;WD)");
        assert!(open(&path, OpenMode::Read).is_err());
        drop(handle);
        // A regular file inherits even a private parent's ACL; it must still be
        // explicitly protected so later parent ACL changes cannot broaden it.
        let inherited = directory.join("inherited");
        std::fs::write(&inherited, b"fixture").unwrap();
        assert!(open(&inherited, OpenMode::Read).is_err());
        let safe_target = directory.join("safe-target");
        drop(open(&safe_target, OpenMode::CreateNew).unwrap());
        let link = directory.join("link");
        match symlink_file(&safe_target, &link) {
            Ok(()) => assert!(open(&link, OpenMode::Read).is_err()),
            Err(error) if error.raw_os_error() == Some(1314) => {
                // Standard Windows accounts without Developer Mode cannot create
                // symbolic links. CI's elevated runner exercises this branch.
            }
            Err(error) => panic!("could not create reparse fixture: {error}"),
        }
        // Existing shared directories are also rejected rather than repaired.
        assert!(verify_directory(root.path()).is_err());
    }
}
