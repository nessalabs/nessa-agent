//! Bounded read-only diagnostics for native publication failures. No ACL/token
//! mutation, credential output or environment-dependent assertion weakening.

use std::{
    ffi::c_void,
    fs::File,
    io,
    mem::size_of,
    os::windows::io::AsRawHandle,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    System::SystemServices::SE_GROUP_ENABLED,
};

struct Descriptor(*mut c_void);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<usize>> {
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token, class, null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 64 * 1024 {
        return Err(io::Error::other("token diagnostic exceeds bound"));
    }
    let mut words = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe { GetTokenInformation(token, class, words.as_mut_ptr().cast(), needed, &mut needed) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(words)
}

pub fn diagnose(label: &str, file: &File) {
    if let Err(error) = read(label, file) {
        eprintln!("native-permissions {label}: diagnostics unavailable: {error}");
    }
}

fn read(label: &str, file: &File) -> io::Result<()> {
    let token = super::token::effective_token()?;
    let user_info = token_info(token.as_raw_handle(), TokenUser)?;
    let user = unsafe { &*user_info.as_ptr().cast::<TOKEN_USER>() };
    let groups_info = token_info(token.as_raw_handle(), TokenGroups)?;
    let groups = unsafe { &*groups_info.as_ptr().cast::<TOKEN_GROUPS>() };
    eprintln!(
        "native-permissions {label}: groups-truncated={}",
        groups.GroupCount > 128
    );
    let groups = unsafe {
        std::slice::from_raw_parts(
            groups.Groups.as_ptr(),
            (groups.GroupCount as usize).min(128),
        )
    };
    let enabled_group = |kind| {
        groups.iter().any(|group| {
            group.Attributes & SE_GROUP_ENABLED as u32 != 0
                && unsafe { IsWellKnownSid(group.Sid, kind) } != 0
        })
    };
    let elevation = token_info(token.as_raw_handle(), TokenElevation)?;
    let elevated = unsafe { (*elevation.as_ptr().cast::<TOKEN_ELEVATION>()).TokenIsElevated != 0 };
    eprintln!("native-permissions {label}: current-is-system={} enabled-system-group={} enabled-admin-group={} elevated={elevated}",
        unsafe { IsWellKnownSid(user.User.Sid, WinLocalSystemSid) } != 0,
        enabled_group(WinLocalSystemSid), enabled_group(WinBuiltinAdministratorsSid));
    let privileges = token_info(token.as_raw_handle(), TokenPrivileges)?;
    let privileges = unsafe { &*privileges.as_ptr().cast::<TOKEN_PRIVILEGES>() };
    eprintln!(
        "native-permissions {label}: privileges-truncated={}",
        privileges.PrivilegeCount > 128
    );
    let privileges = unsafe {
        std::slice::from_raw_parts(
            privileges.Privileges.as_ptr(),
            (privileges.PrivilegeCount as usize).min(128),
        )
    };
    for name in [
        "SeBackupPrivilege",
        "SeRestorePrivilege",
        "SeTakeOwnershipPrivilege",
    ] {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut luid = LUID::default();
        if unsafe { LookupPrivilegeValueW(null(), wide.as_ptr(), &mut luid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let enabled = privileges.iter().any(|entry| {
            entry.Luid.LowPart == luid.LowPart
                && entry.Luid.HighPart == luid.HighPart
                && entry.Attributes & SE_PRIVILEGE_ENABLED != 0
        });
        eprintln!("native-permissions {label}: {name}-enabled={enabled}");
    }

    let mut acl = null_mut();
    let mut descriptor = null_mut();
    let result = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let _descriptor = Descriptor(descriptor);
    if acl.is_null() {
        eprintln!("native-permissions {label}: null-dacl=true");
        return Ok(());
    }
    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    eprintln!(
        "native-permissions {label}: protected={} ace-count={}",
        control & SE_DACL_PROTECTED != 0,
        unsafe { (*acl).AceCount }
    );
    for index in 0..u32::from(unsafe { (*acl).AceCount }).min(16) {
        let mut ace = null_mut();
        if unsafe { GetAce(acl, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        // ACCESS_ALLOWED_ACE_TYPE, the same native header value the production
        // private-file verifier accepts. Diagnostics do not decide permission.
        if header.AceType == 0 {
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            let sid = (&allowed.SidStart as *const u32).cast_mut().cast();
            eprintln!("native-permissions {label}: ace={index} allow-mask={:#010x} flags={:#04x} current-user={} local-system={}",
                allowed.Mask, header.AceFlags, unsafe { EqualSid(sid, user.User.Sid) } != 0,
                unsafe { IsWellKnownSid(sid, WinLocalSystemSid) } != 0);
        } else {
            eprintln!(
                "native-permissions {label}: ace={index} type={} flags={:#04x}",
                header.AceType, header.AceFlags
            );
        }
    }
    Ok(())
}
