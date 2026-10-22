//! Thread-local ordinary-writer fixture. Never changes the process token.
use std::{
    io,
    marker::PhantomData,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    rc::Rc,
};
use windows_sys::Win32::{Foundation::*, Security::*, System::Threading::*};

fn check(value: i32) -> io::Result<()> {
    if value == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<usize>> {
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token, class, null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 64 * 1024 {
        return Err(io::Error::other("token fixture exceeds bound"));
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    check(unsafe {
        GetTokenInformation(
            token,
            class,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    })?;
    Ok(buffer)
}

fn thread_token() -> io::Result<Option<OwnedHandle>> {
    let mut token = null_mut();
    if unsafe {
        OpenThreadToken(
            GetCurrentThread(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
            1,
            &mut token,
        )
    } != 0
    {
        Ok(Some(unsafe { OwnedHandle::from_raw_handle(token) }))
    } else if io::Error::last_os_error().raw_os_error() == Some(ERROR_NO_TOKEN as i32) {
        Ok(None)
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn effective_token() -> io::Result<OwnedHandle> {
    if let Some(token) = thread_token()? {
        return Ok(token);
    }
    let mut token = null_mut();
    check(unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut token,
        )
    })?;
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

pub fn privilege_enabled(name: &str) -> io::Result<bool> {
    let token = effective_token()?;
    let data = token_info(token.as_raw_handle(), TokenPrivileges)?;
    let privileges = unsafe { &*data.as_ptr().cast::<TOKEN_PRIVILEGES>() };
    let entries = unsafe {
        std::slice::from_raw_parts(
            privileges.Privileges.as_ptr(),
            privileges.PrivilegeCount as usize,
        )
    };
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut luid = LUID::default();
    check(unsafe { LookupPrivilegeValueW(null(), name.as_ptr(), &mut luid) })?;
    Ok(entries.iter().any(|entry| {
        entry.Luid.LowPart == luid.LowPart
            && entry.Luid.HighPart == luid.HighPart
            && entry.Attributes & SE_PRIVILEGE_ENABLED != 0
    }))
}

fn sid_bytes(sid: PSID) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize).to_vec() }
}

type Principal = (Vec<u8>, Vec<(Vec<u8>, u32)>);

fn principal(token: HANDLE) -> io::Result<Principal> {
    let user = token_info(token, TokenUser)?;
    let user = unsafe { &*user.as_ptr().cast::<TOKEN_USER>() };
    let groups = token_info(token, TokenGroups)?;
    let groups = unsafe { &*groups.as_ptr().cast::<TOKEN_GROUPS>() };
    let groups =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) };
    let mut groups: Vec<_> = groups
        .iter()
        .map(|group| (sid_bytes(group.Sid), group.Attributes))
        .collect();
    groups.sort();
    Ok((sid_bytes(user.User.Sid), groups))
}

fn token_id(token: HANDLE) -> io::Result<(u32, i32)> {
    let data = token_info(token, TokenStatistics)?;
    let id = unsafe { (*data.as_ptr().cast::<TOKEN_STATISTICS>()).TokenId };
    Ok((id.LowPart, id.HighPart))
}

pub struct OrdinaryToken {
    prior: Option<OwnedHandle>,
    _installed: OwnedHandle,
    // Thread impersonation must never be restored on a different thread.
    _thread: PhantomData<Rc<()>>,
}

impl OrdinaryToken {
    pub fn enter() -> Self {
        Self::install().expect("install ordinary-writer thread token")
    }

    fn install() -> io::Result<Self> {
        let prior = thread_token()?;
        let source = effective_token()?;
        let expected = principal(source.as_raw_handle())?;
        let mut restricted = null_mut();
        check(unsafe {
            CreateRestrictedToken(
                source.as_raw_handle(),
                DISABLE_MAX_PRIVILEGE,
                0,
                null(),
                0,
                null(),
                0,
                null(),
                &mut restricted,
            )
        })?;
        let restricted = unsafe { OwnedHandle::from_raw_handle(restricted) };
        let mut installed = null_mut();
        check(unsafe {
            DuplicateTokenEx(
                restricted.as_raw_handle(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
                null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut installed,
            )
        })?;
        let installed = unsafe { OwnedHandle::from_raw_handle(installed) };
        check(unsafe { SetThreadToken(null(), installed.as_raw_handle()) })?;
        let guard = Self {
            prior,
            _installed: installed,
            _thread: PhantomData,
        };
        let effective = effective_token()?;
        assert_eq!(
            principal(effective.as_raw_handle())?,
            expected,
            "privilege-only fixture preserves user and all group attributes"
        );
        assert!(!privilege_enabled("SeBackupPrivilege")?);
        assert!(!privilege_enabled("SeRestorePrivilege")?);
        // Actual full-ACL controls distinguish a denied ordinary token from a
        // fixture that accidentally removed all useful filesystem authority.
        let control = tempfile::tempdir()?;
        let before = control.path().join("before");
        std::fs::create_dir(&before)?;
        std::fs::write(before.join("payload"), b"ordinary")?;
        let after = control.path().join("after");
        std::fs::rename(&before, &after)?;
        assert_eq!(std::fs::read(after.join("payload"))?, b"ordinary");
        std::fs::remove_file(after.join("payload"))?;
        std::fs::remove_dir(after)?;
        Ok(guard)
    }

    fn restore(&self) -> io::Result<()> {
        let expected = self
            .prior
            .as_ref()
            .map(|token| token_id(token.as_raw_handle()))
            .transpose()?;
        check(unsafe {
            SetThreadToken(
                null(),
                self.prior
                    .as_ref()
                    .map_or(null_mut(), AsRawHandle::as_raw_handle),
            )
        })?;
        let actual = thread_token()?
            .as_ref()
            .map(|token| token_id(token.as_raw_handle()))
            .transpose()?;
        if actual != expected {
            return Err(io::Error::other(
                "prior thread token identity was not restored",
            ));
        }
        Ok(())
    }
}

impl Drop for OrdinaryToken {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("ordinary-writer thread token restoration failed: {error}");
            assert!(
                std::thread::panicking(),
                "thread token restoration failed: {error}"
            );
        }
    }
}
