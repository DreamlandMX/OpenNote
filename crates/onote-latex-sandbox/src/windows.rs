//! AppContainer + Job Object implementation. See the crate docs for the rules.

use std::ffi::{c_void, OsStr};
use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, SetHandleInformation, ERROR_ALREADY_EXISTS, GENERIC_READ,
    HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW,
    SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, REVOKE_ACCESS,
    SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    FreeSid, GetSecurityDescriptorControl, InitializeSecurityDescriptor, SetFileSecurityW,
    SetSecurityDescriptorControl, SetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION,
    NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
    SECURITY_DESCRIPTOR, SE_DACL_AUTO_INHERITED, SE_DACL_PROTECTED, SID_AND_ATTRIBUTES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS,
    EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use crate::{Exit, Network, SandboxError, SandboxRun, SandboxSpec};

/// How long a terminated job may take to report no live process.
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(25);

fn fail(stage: &'static str) -> SandboxError {
    SandboxError {
        stage,
        code: unsafe { GetLastError() },
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

struct Handle(HANDLE);

impl Handle {
    fn valid(raw: HANDLE, stage: &'static str) -> Result<Self, SandboxError> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(fail(stage))
        } else {
            Ok(Self(raw))
        }
    }

    /// Hands the handle to the caller; it is no longer closed here.
    fn into_raw(self) -> HANDLE {
        let raw = self.0;
        std::mem::forget(self);
        raw
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Sid(PSID);

impl Drop for Sid {
    fn drop(&mut self) {
        unsafe {
            FreeSid(self.0);
        }
    }
}

/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const HRESULT_ALREADY_EXISTS: i32 = (0x8007_0000u32 | ERROR_ALREADY_EXISTS) as i32;

/// The profile's SID, creating the profile (with no capabilities) on first use.
fn container_sid(profile: &str) -> Result<Sid, SandboxError> {
    let name = wide(OsStr::new(profile));
    let mut sid: PSID = std::ptr::null_mut();
    let created = unsafe {
        CreateAppContainerProfile(
            name.as_ptr(),
            name.as_ptr(),
            name.as_ptr(),
            std::ptr::null(),
            0,
            &mut sid,
        )
    };
    if created == 0 {
        return Ok(Sid(sid));
    }
    if created != HRESULT_ALREADY_EXISTS {
        return Err(SandboxError {
            stage: "createProfile",
            code: created as u32,
        });
    }
    let derived = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
    if derived != 0 {
        return Err(SandboxError {
            stage: "deriveProfile",
            code: derived as u32,
        });
    }
    Ok(Sid(sid))
}

pub(crate) fn delete_profile(profile: &str) -> Result<(), SandboxError> {
    let name = wide(OsStr::new(profile));
    let result = unsafe { DeleteAppContainerProfile(name.as_ptr()) };
    if result != 0 {
        return Err(SandboxError {
            stage: "deleteProfile",
            code: result as u32,
        });
    }
    Ok(())
}

/// Adds an inheritable allow entry for the container SID to `path`'s DACL.
fn grant(path: &Path, sid: &Sid, mask: u32) -> Result<(), SandboxError> {
    edit_acl(
        path,
        sid,
        GRANT_ACCESS,
        mask,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Write::Propagate,
    )
}

/// How the edited DACL is written back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Write {
    /// `SetNamedSecurityInfoW`: inheritable entries reach existing children.
    /// Only for directories the app created.
    Propagate,
    /// `SetFileSecurityW`: the object's own descriptor only, nothing below it
    /// is visited or rewritten. Used for directories the app does not own.
    ObjectOnly,
}

/// `FILE_READ_ATTRIBUTES` on the directory object itself, not inherited.
fn grant_attributes_only(path: &Path, sid: &Sid) -> Result<(), SandboxError> {
    edit_acl(
        path,
        sid,
        GRANT_ACCESS,
        FILE_READ_ATTRIBUTES,
        NO_INHERITANCE,
        Write::ObjectOnly,
    )
}

/// Applies one entry for the container SID to `path`'s DACL.
fn edit_acl(
    path: &Path,
    sid: &Sid,
    mode: i32,
    mask: u32,
    inheritance: u32,
    write: Write,
) -> Result<(), SandboxError> {
    let name = wide(path.as_os_str());
    let mut old: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let read = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut old,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if read != 0 {
        return Err(SandboxError {
            stage: "readAcl",
            code: read,
        });
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: mode,
        grfInheritance: inheritance,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: sid.0 as *mut u16,
        },
    };
    let mut updated: *mut ACL = std::ptr::null_mut();
    let merged = unsafe { SetEntriesInAclW(1, &entry, old, &mut updated) };
    let outcome = if merged != 0 {
        Err(SandboxError {
            stage: "mergeAcl",
            code: merged,
        })
    } else if write == Write::Propagate {
        let written = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                updated,
                std::ptr::null(),
            )
        };
        if written != 0 {
            Err(SandboxError {
                stage: "writeAcl",
                code: written,
            })
        } else {
            Ok(())
        }
    } else {
        write_object_only(&name, descriptor, updated)
    };
    unsafe {
        if !updated.is_null() {
            LocalFree(updated as *mut c_void);
        }
        LocalFree(descriptor);
    }
    outcome
}

/// Writes `dacl` onto the object alone, keeping the old descriptor's
/// protected / auto-inherited flags so its inheritance behaves as before.
fn write_object_only(
    name: &[u16],
    old: PSECURITY_DESCRIPTOR,
    dacl: *mut ACL,
) -> Result<(), SandboxError> {
    const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
    let mut fresh: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
    let descriptor = &mut fresh as *mut SECURITY_DESCRIPTOR as PSECURITY_DESCRIPTOR;
    let flags = SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED;
    let mut control = 0u16;
    let mut revision = 0u32;
    unsafe {
        if InitializeSecurityDescriptor(descriptor, SECURITY_DESCRIPTOR_REVISION) == 0
            || SetSecurityDescriptorDacl(descriptor, 1, dacl, 0) == 0
            || GetSecurityDescriptorControl(old, &mut control, &mut revision) == 0
            || SetSecurityDescriptorControl(descriptor, flags, control & flags) == 0
        {
            return Err(fail("buildDescriptor"));
        }
        if SetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION, descriptor) == 0 {
            return Err(fail("writeObjectAcl"));
        }
    }
    Ok(())
}

/// The SID of an existing profile, without creating it.
fn existing_sid(profile: &str) -> Result<Sid, SandboxError> {
    let name = wide(OsStr::new(profile));
    let mut sid: PSID = std::ptr::null_mut();
    let derived = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
    if derived != 0 {
        return Err(SandboxError {
            stage: "deriveProfile",
            code: derived as u32,
        });
    }
    Ok(Sid(sid))
}

pub(crate) fn revoke(profile: &str, directory: &Path) -> Result<(), SandboxError> {
    let sid = existing_sid(profile)?;
    // REVOKE_ACCESS removes the explicit entries for this SID only; the object
    // is rewritten alone, so nothing below it is visited.
    edit_acl(
        directory,
        &sid,
        REVOKE_ACCESS,
        0,
        NO_INHERITANCE,
        Write::ObjectOnly,
    )
}

pub(crate) fn profile_sid(profile: &str) -> Result<String, SandboxError> {
    let sid = existing_sid(profile)?;
    let mut text: *mut u16 = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid.0, &mut text) } == 0 {
        return Err(fail("sidToString"));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe {
        LocalFree(text as *mut c_void);
    }
    Ok(value)
}

pub(crate) fn profile_folders() -> Result<Vec<std::path::PathBuf>, SandboxError> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Profile, FOLDERID_RoamingAppData, SHGetKnownFolderPath,
    };
    let mut folders = Vec::new();
    for id in [FOLDERID_Profile, FOLDERID_RoamingAppData] {
        let mut path: *mut u16 = std::ptr::null_mut();
        let result = unsafe { SHGetKnownFolderPath(&id, 0, std::ptr::null_mut(), &mut path) };
        let found = (result == 0).then(|| {
            let mut length = 0;
            while unsafe { *path.add(length) } != 0 {
                length += 1;
            }
            std::path::PathBuf::from(String::from_utf16_lossy(unsafe {
                std::slice::from_raw_parts(path, length)
            }))
        });
        unsafe { CoTaskMemFree(path as *const c_void) };
        folders.push(found.ok_or(SandboxError {
            stage: "knownFolder",
            code: result as u32,
        })?);
    }
    Ok(folders)
}

/// How often a byte budget is measured while the process runs.
const BUDGET_CHECK: Duration = Duration::from_millis(250);

/// Total size of the regular files under directory (reparse points are not
/// followed). An unreadable entry counts as zero; the budget only ever stops
/// a run, it never lets one continue.
fn directory_bytes(directory: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// internetClient (S-1-15-3-1), the one capability the product ever grants.
const INTERNET_CLIENT_SID: &str = "S-1-15-3-1";
const SE_GROUP_ENABLED: u32 = 0x4;

struct Capabilities {
    entries: Vec<SID_AND_ATTRIBUTES>,
}

impl Drop for Capabilities {
    fn drop(&mut self) {
        for entry in &self.entries {
            unsafe {
                LocalFree(entry.Sid);
            }
        }
    }
}

fn capability_list(network: Network) -> Result<Capabilities, SandboxError> {
    let mut entries = Vec::new();
    if network == Network::InternetClientOnly {
        let text = wide(OsStr::new(INTERNET_CLIENT_SID));
        let mut sid: PSID = std::ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) } == 0 {
            return Err(fail("capabilitySid"));
        }
        entries.push(SID_AND_ATTRIBUTES {
            Sid: sid,
            Attributes: SE_GROUP_ENABLED,
        });
    }
    Ok(Capabilities { entries })
}

/// Windows command-line quoting (the rules `CommandLineToArgvW` reverses).
fn quote(argument: &OsStr, line: &mut Vec<u16>) {
    let units: Vec<u16> = argument.encode_wide().collect();
    let needs = units.is_empty()
        || units
            .iter()
            .any(|unit| *unit == b' ' as u16 || *unit == b'\t' as u16 || *unit == b'"' as u16);
    if !needs {
        line.extend_from_slice(&units);
        return;
    }
    line.push(b'"' as u16);
    let mut backslashes = 0usize;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            line.extend(std::iter::repeat(b'\\' as u16).take(backslashes * 2 + 1));
        } else {
            line.extend(std::iter::repeat(b'\\' as u16).take(backslashes));
        }
        backslashes = 0;
        line.push(unit);
    }
    line.extend(std::iter::repeat(b'\\' as u16).take(backslashes * 2));
    line.push(b'"' as u16);
}

/// Passed through from the app's environment. Creating an AppContainer process
/// fails with ERROR_ENVVAR_NOT_FOUND without `LOCALAPPDATA` (the system derives
/// the container's own package folder from it); nothing else is needed.
const PASSTHROUGH: &[&str] = &["LOCALAPPDATA"];

/// A minimal environment: no user paths or secrets from the app's own one.
fn environment(spec: &SandboxSpec) -> Vec<u16> {
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let mut entries: Vec<(String, std::ffi::OsString)> = Vec::new();
    let temp = spec.cwd.as_os_str().to_owned();
    let mut path = system_root.clone();
    path.push("\\System32");
    entries.push(("PATH".into(), path));
    entries.push(("SystemRoot".into(), system_root.clone()));
    entries.push(("TEMP".into(), temp.clone()));
    entries.push(("TMP".into(), temp));
    entries.push(("windir".into(), system_root));
    for name in PASSTHROUGH {
        if let Some(value) = std::env::var_os(name) {
            entries.push(((*name).into(), value));
        }
    }
    for (name, value) in &spec.env {
        // Names are case-insensitive on Windows; an extra replaces a default.
        entries.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        entries.push((name.clone(), value.clone()));
    }
    entries.sort_by(|left, right| {
        left.0
            .to_ascii_uppercase()
            .cmp(&right.0.to_ascii_uppercase())
    });
    let mut block = Vec::new();
    for (key, value) in entries {
        block.extend(OsStr::new(&key).encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

fn pipe(attributes: &SECURITY_ATTRIBUTES) -> Result<(Handle, Handle), SandboxError> {
    let mut read: HANDLE = std::ptr::null_mut();
    let mut write: HANDLE = std::ptr::null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, attributes, 0) } == 0 {
        return Err(fail("createPipe"));
    }
    let (read, write) = (Handle(read), Handle(write));
    // Only the child's end is inheritable.
    if unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(fail("pipeInherit"));
    }
    Ok((read, write))
}

/// Reads a stream to its end, keeping at most `limit` bytes.
fn drain(handle: Handle, limit: usize) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    let mut file = unsafe { File::from_raw_handle(handle.into_raw()) };
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut truncated = false;
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match file.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let room = limit.saturating_sub(kept.len());
                    if read > room {
                        truncated = true;
                    }
                    kept.extend_from_slice(&buffer[..read.min(room)]);
                }
            }
        }
        (kept, truncated)
    })
}

fn active_processes(job: &Handle) -> Result<u32, SandboxError> {
    let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        QueryInformationJobObject(
            job.0,
            JobObjectBasicAccountingInformation,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(fail("queryJob"));
    }
    Ok(info.ActiveProcesses)
}

/// Terminates the job and waits until it reports no live process.
fn end_job(job: &Handle) -> Result<u32, SandboxError> {
    unsafe {
        TerminateJobObject(job.0, 1);
    }
    let deadline = Instant::now() + TERMINATION_GRACE;
    loop {
        let alive = active_processes(job)?;
        if alive == 0 || Instant::now() >= deadline {
            return Ok(alive);
        }
        std::thread::sleep(POLL);
    }
}

pub(crate) fn run(spec: &SandboxSpec, cancel: &AtomicBool) -> Result<SandboxRun, SandboxError> {
    if !spec.program.is_absolute() || !spec.cwd.is_absolute() {
        return Err(SandboxError {
            stage: "relativePath",
            code: 0,
        });
    }
    let sid = container_sid(&spec.profile)?;
    for directory in &spec.read_dirs {
        grant(directory, &sid, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE)?;
    }
    for directory in &spec.write_dirs {
        grant(
            directory,
            &sid,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE,
        )?;
    }
    for directory in &spec.attribute_only_dirs {
        grant_attributes_only(directory, &sid)?;
    }

    // The job is configured before any process exists.
    let job = Handle::valid(
        unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) },
        "createJob",
    )?;
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // No JOB_OBJECT_LIMIT_BREAKAWAY_OK / SILENT_BREAKAWAY_OK: children stay in.
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(fail("configureJob"));
    }

    let inherit = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let (out_read, out_write) = pipe(&inherit)?;
    let (err_read, err_write) = pipe(&inherit)?;
    let nul = wide(OsStr::new("NUL"));
    let stdin = Handle::valid(
        unsafe {
            CreateFileW(
                nul.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &inherit,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        },
        "openNul",
    )?;

    // Only these three handles may be inherited, whatever else is inheritable.
    let inherited = [stdin.0, out_write.0, err_write.0];
    // Kept alive until the process exists; empty for Network::None.
    let mut granted = capability_list(spec.network)?;
    let mut capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid.0,
        Capabilities: if granted.entries.is_empty() {
            std::ptr::null_mut()
        } else {
            granted.entries.as_mut_ptr()
        },
        CapabilityCount: granted.entries.len() as u32,
        Reserved: 0,
    };
    let mut size = 0usize;
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 2, 0, &mut size);
    }
    let mut storage = vec![0u8; size];
    let attributes = storage.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    if unsafe { InitializeProcThreadAttributeList(attributes, 2, 0, &mut size) } == 0 {
        return Err(fail("attributeList"));
    }
    struct AttributeList(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for AttributeList {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.0) }
        }
    }
    let _list = AttributeList(attributes);
    if unsafe {
        UpdateProcThreadAttribute(
            attributes,
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            &mut capabilities as *mut _ as *const c_void,
            std::mem::size_of::<SECURITY_CAPABILITIES>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(fail("capabilitiesAttribute"));
    }
    if unsafe {
        UpdateProcThreadAttribute(
            attributes,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inherited.as_ptr() as *const c_void,
            std::mem::size_of_val(&inherited),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(fail("handleListAttribute"));
    }

    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin.0;
    startup.StartupInfo.hStdOutput = out_write.0;
    startup.StartupInfo.hStdError = err_write.0;
    startup.lpAttributeList = attributes;

    let program = wide(spec.program.as_os_str());
    let mut line = Vec::new();
    quote(spec.program.as_os_str(), &mut line);
    for argument in &spec.args {
        line.push(b' ' as u16);
        quote(argument, &mut line);
    }
    line.push(0);
    let mut env = environment(spec);
    let cwd = wide(spec.cwd.as_os_str());
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            program.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | DETACHED_PROCESS
                | CREATE_UNICODE_ENVIRONMENT,
            env.as_mut_ptr() as *const c_void,
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut process,
        )
    };
    if created == 0 {
        return Err(fail("createProcess"));
    }
    let process_handle = Handle(process.hProcess);
    let thread = Handle(process.hThread);
    // The child holds its own copies now; ours must close for EOF to arrive.
    drop(out_write);
    drop(err_write);
    drop(stdin);

    if unsafe { AssignProcessToJobObject(job.0, process_handle.0) } == 0 {
        let error = fail("assignJob");
        unsafe {
            TerminateProcess(process_handle.0, 1);
        }
        return Err(error);
    }
    let stdout = drain(out_read, spec.max_output_bytes);
    let stderr = drain(err_read, spec.max_output_bytes);
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        let error = fail("resume");
        let _ = end_job(&job);
        return Err(error);
    }
    drop(thread);

    let deadline = Instant::now() + spec.timeout;
    let mut next_budget_check = Instant::now();
    let exit = loop {
        if unsafe { WaitForSingleObject(process_handle.0, POLL.as_millis() as u32) }
            == WAIT_OBJECT_0
        {
            let mut code = 0u32;
            unsafe {
                GetExitCodeProcess(process_handle.0, &mut code);
            }
            break Exit::Exited(code);
        }
        if cancel.load(Ordering::Relaxed) {
            break Exit::Cancelled;
        }
        if Instant::now() >= deadline {
            break Exit::TimedOut;
        }
        if let Some((directory, limit)) = &spec.byte_budget {
            if Instant::now() >= next_budget_check {
                next_budget_check = Instant::now() + BUDGET_CHECK;
                if directory_bytes(directory) > *limit {
                    break Exit::ByteBudgetExceeded;
                }
            }
        }
    };
    // Whatever happened, nothing of the job may outlive the run; the caller
    // must treat a non-zero `remaining_processes` as a failed isolation.
    let remaining = end_job(&job)?;
    let (stdout, out_truncated) = stdout.join().unwrap_or_default();
    let (stderr, err_truncated) = stderr.join().unwrap_or_default();
    Ok(SandboxRun {
        exit,
        stdout,
        stderr,
        truncated: out_truncated || err_truncated,
        remaining_processes: remaining,
    })
}
