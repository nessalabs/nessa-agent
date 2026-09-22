//! Win32 handles bind validation and I/O to the same object. Creation supplies a
//! protected DACL atomically; chmod and post-creation ACL repair are not used.
//! Validated drive-absolute paths use normalized extended-length Win32 spelling.
//! Device namespaces, reserved names, and trailing-dot/space ambiguities are
//! rejected before conversion; long paths retain the same handle/ACL checks.
use super::*;
use std::{
    ffi::{c_void, OsStr},
    fs,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, PathBuf, Prefix},
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
        || !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)))
    {
        return Err(unsafe_file());
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    // Reject alternate data streams and device namespaces, as well as NULs.
    if wide.contains(&0)
        || path.components().any(|component| match component {
            Component::ParentDir => true,
            Component::Normal(name) => ambiguous_component(name),
            _ => false,
        })
        || path
            .as_os_str()
            .to_string_lossy()
            .get(2..)
            .is_some_and(|p| p.contains(':'))
    {
        return Err(unsafe_file());
    }
    // Win32's ordinary spelling has a total MAX_PATH limit even when each
    // component is valid. Normalize separators and dot components before using
    // the extended namespace, where Win32 no longer performs that normalization.
    // Only validated drive-absolute paths reach this conversion; callers cannot
    // supply device/UNC namespaces or alternate data streams through it.
    let normalized: PathBuf = path.components().collect();
    Ok(r"\\?\"
        .encode_utf16()
        .chain(normalized.as_os_str().encode_wide().map(|unit| {
            if unit == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                unit
            }
        }))
        .chain(Some(0))
        .collect())
}
// DOS device names remain reserved with an extension. Verbatim paths must not
// turn a previously device-resolved spelling into a newly created regular file.
fn ambiguous_component(name: &OsStr) -> bool {
    if name
        .encode_wide()
        .last()
        .is_some_and(|unit| unit == u16::from(b'.') || unit == u16::from(b' '))
    {
        return true;
    }
    let text = name.to_string_lossy();
    let stem = text
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ');
    ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
        || stem.get(..3).is_some_and(|prefix| {
            (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
                && matches!(
                    &stem[3..],
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
        })
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
    open_with_additional_access(path, mode, 0)
}
fn open_with_additional_access(
    path: &Path,
    mode: OpenMode,
    additional_access: u32,
) -> io::Result<File> {
    check_parents(path)?;
    let user = User::current()?;
    let descriptor = user.descriptor(false)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let access = GENERIC_READ
        | additional_access
        | if matches!(mode, OpenMode::Read | OpenMode::ReadNonblocking) {
            0
        } else {
            GENERIC_WRITE
        };
    let disposition = match mode {
        OpenMode::Read | OpenMode::ReadNonblocking | OpenMode::ReadWrite => OPEN_EXISTING,
        OpenMode::OpenOrCreate => OPEN_ALWAYS,
        OpenMode::CreateNew => CREATE_NEW,
    };
    let file = raw_open(path, access, disposition, &attributes)?;
    verify_file(&file)?;
    Ok(file)
}
pub fn open_temporary(path: &Path) -> io::Result<File> {
    open_with_additional_access(path, OpenMode::CreateNew, DELETE)
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
pub fn create_directory_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    if relative.components().next().is_none()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(unsafe_file());
    }
    verify_directory(root)?;
    create_directory(&root.join(relative))
}
pub fn open_beneath(root: &Path, relative: &Path, mode: OpenMode) -> io::Result<File> {
    if relative.components().next().is_none()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(unsafe_file());
    }
    verify_directory(root)?;
    open(&root.join(relative), mode)
}
pub fn open_temporary_beneath(root: &Path, relative: &Path) -> io::Result<File> {
    if relative.components().next().is_none()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(unsafe_file());
    }
    verify_directory(root)?;
    open_temporary(&root.join(relative))
}
/// Windows does not expose Unix directory fsync semantics. Files are flushed
/// before publication; replacement requests the platform's write-through move.
pub fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}
/// Remove a file named relative to an already-private root.
///
/// Windows has no `unlinkat`, so this verifies the root and the path's parents
/// the way every other entry point here does and then removes by path.
pub fn remove_file_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    let path = beneath(root, relative)?;
    check_parents(&path)?;
    fs::remove_file(path)
}

/// Mark the already-open reservation for deletion instead of resolving its
/// path again. The handle remains bound to the file even if an ancestor name is
/// concurrently replaced, so cleanup cannot delete an outside same-name file.
pub fn remove_reserved_beneath(file: &File, _: &Path, _: &Path) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    unsafe {
        check(SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        ))
    }
}

/// Remove an empty directory named relative to an already-private root.
pub fn remove_directory_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    let path = beneath(root, relative)?;
    check_parents(&path)?;
    fs::remove_dir(path)
}

/// The absolute path of `relative`, once it is a name and the root is private.
fn beneath(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    if relative.components().next().is_none()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(unsafe_file());
    }
    verify_directory(root)?;
    Ok(root.join(relative))
}

pub fn sync_directory_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    if relative.as_os_str().is_empty() {
        verify_directory(root)
    } else {
        verify_directory(&root.join(relative))
    }
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
/// Publish `from` under the unused name `to`, never replacing an existing one.
///
/// The move omits `MOVEFILE_REPLACE_EXISTING`, so a taken destination fails
/// with `AlreadyExists` instead of overwriting another owner's record.
pub fn publish_new(from: &Path, to: &Path) -> io::Result<()> {
    check_parents(from)?;
    check_parents(to)?;
    unsafe {
        check(MoveFileExW(
            wide(from)?.as_ptr(),
            wide(to)?.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        ))
    }
}
pub fn publish_new_beneath(root: &Path, from: &Path, to: &Path) -> io::Result<()> {
    if from.components().next().is_none()
        || to.components().next().is_none()
        || from
            .components()
            .chain(to.components())
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(unsafe_file());
    }
    verify_directory(root)?;
    publish_new(&root.join(from), &root.join(to))
}
pub fn replace_beneath(root: &Path, from: &Path, to: &Path) -> io::Result<()> {
    if from.components().next().is_none()
        || to.components().next().is_none()
        || from
            .components()
            .chain(to.components())
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(unsafe_file());
    }
    verify_directory(root)?;
    replace(&root.join(from), &root.join(to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_private_directory_tree_beneath, PrivateTempFile};
    use std::{
        io::{Read, Write},
        os::windows::fs::symlink_file,
    };

    #[test]
    fn win32_paths_use_normalized_extended_spelling_without_namespace_bypasses() {
        let actual = wide(Path::new(r"C:/private/./session.jsonl")).unwrap();
        assert_eq!(
            String::from_utf16(&actual[..actual.len() - 1]).unwrap(),
            r"\\?\C:\private\session.jsonl"
        );
        assert_eq!(actual.last(), Some(&0));
        for invalid in [
            r"C:\private\NUL.txt",
            r"C:\private\com1",
            r"C:\private\lpt¹.log",
            r"C:\private\CONOUT$",
            r"C:\private.\file",
            r"C:\private\file ",
            r"relative",
            r"C:relative",
            r"C:\private\..\other",
            r"C:\private\file:stream",
            r"\\server\share\file",
            r"\\?\C:\private\file",
            r"\\.\NUL",
            "C:\\private\\nul\0",
        ] {
            assert!(wide(Path::new(invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn long_absolute_paths_support_private_creation_open_and_replacement() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("a".repeat(120)).join("b".repeat(120));
        assert!(directory.as_os_str().encode_wide().count() > 260);
        create_directory(&directory).unwrap();
        verify_directory(&directory).unwrap();
        let from = directory.join("s".repeat(213));
        let to = directory.join("t".repeat(213));
        let mut source = open(&from, OpenMode::CreateNew).unwrap();
        source.write_all(b"replacement").unwrap();
        source.sync_all().unwrap();
        drop(source);
        drop(open(&to, OpenMode::CreateNew).unwrap());
        replace(&from, &to).unwrap();
        let mut restored = open(&to, OpenMode::Read).unwrap();
        let mut contents = String::new();
        restored.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "replacement");
        assert_eq!(
            open(&from, OpenMode::Read).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn private_temporary_drop_has_the_capability_to_remove_its_reservation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        create_directory(&root).unwrap();
        create_private_directory_tree_beneath(&root, Path::new("audit")).unwrap();

        let reserved = PrivateTempFile::new_beneath(&root, Path::new("audit")).unwrap();
        assert_eq!(fs::read_dir(root.join("audit")).unwrap().count(), 1);
        drop(reserved);

        assert_eq!(fs::read_dir(root.join("audit")).unwrap().count(), 0);
    }

    #[test]
    fn failed_private_publication_cleans_its_reservation_without_replacing_the_winner() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        create_directory(&root).unwrap();
        create_private_directory_tree_beneath(&root, Path::new("audit")).unwrap();
        let destination = Path::new("audit/record.json");
        let mut winner = PrivateTempFile::new_beneath(&root, Path::new("audit")).unwrap();
        winner.as_file_mut().write_all(b"winner").unwrap();
        winner.publish_new_beneath(destination).unwrap();
        let mut loser = PrivateTempFile::new_beneath(&root, Path::new("audit")).unwrap();
        loser.as_file_mut().write_all(b"loser").unwrap();

        let error = loser.publish_new_beneath(destination).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(root.join(destination)).unwrap(), b"winner");
        assert_eq!(fs::read_dir(root.join("audit")).unwrap().count(), 1);
    }

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
