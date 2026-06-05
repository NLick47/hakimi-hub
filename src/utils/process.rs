use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(windows)]
mod win_mutex {
    use std::ffi::c_void;

    pub fn try_create(mutex_name: &str) -> Option<MutexGuard> {
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateMutexA(
                lpMutexAttributes: *mut c_void,
                bInitialOwner: i32,
                lpName: *const u8,
            ) -> *mut c_void;

            fn CloseHandle(handle: *mut c_void) -> i32;
        }

        unsafe {
            let name = format!("{}\0", mutex_name);
            let handle = CreateMutexA(std::ptr::null_mut(), 1, name.as_ptr());
            if handle.is_null() {
                return None;
            }

            let already_exists = GetLastError() == 183;
            if already_exists {
                CloseHandle(handle);
                return None;
            }

            Some(MutexGuard { handle })
        }
    }

    pub struct MutexGuard {
        handle: *mut c_void,
    }

    unsafe impl Send for MutexGuard {}

    impl Drop for MutexGuard {
        fn drop(&mut self) {
            unsafe {
                #[link(name = "kernel32")]
                extern "system" {
                    fn CloseHandle(handle: *mut c_void) -> i32;
                }
                CloseHandle(self.handle);
            }
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }
}

#[cfg(unix)]
mod unix_lock {
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::io::AsRawFd;

    pub fn try_lock(pid_file: &std::path::Path) -> Option<FileGuard> {
        let file = File::create(pid_file).ok()?;
        let fd = file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return None;
        }
        let mut f = &file;
        let _ = f.write_all(format!("{}\n", std::process::id()).as_bytes());
        let _ = f.flush();
        Some(FileGuard { _file: file })
    }

    pub struct FileGuard {
        _file: File,
    }
}

static SINGLE_INSTANCE_GUARD: Mutex<Option<SingleInstanceGuard>> = Mutex::new(None);

enum SingleInstanceGuard {
    #[cfg(windows)]
    WinMutex(#[allow(dead_code)] win_mutex::MutexGuard),
    #[cfg(unix)]
    UnixFile(unix_lock::FileGuard),
}

fn pid_file_path() -> PathBuf {
    crate::utils::paths::pid_file_path()
}

#[cfg(windows)]
const MUTEX_NAME: &str = "Global\\hakimi-hub-single-instance";

pub fn is_running() -> bool {
    SINGLE_INSTANCE_GUARD.lock().map_or(false, |g| g.is_some())
}

#[cfg(windows)]
pub fn acquire_single_instance() -> anyhow::Result<()> {
    if is_running() {
        anyhow::bail!("当前进程已持有单实例锁");
    }

    match win_mutex::try_create(MUTEX_NAME) {
        Some(guard) => {
            write_pid_file()?;
            if let Ok(mut g) = SINGLE_INSTANCE_GUARD.lock() {
                *g = Some(SingleInstanceGuard::WinMutex(guard));
            }
            Ok(())
        }
        None => anyhow::bail!("已有另一个 Hakimi Hub 实例在运行"),
    }
}

#[cfg(unix)]
pub fn acquire_single_instance() -> anyhow::Result<()> {
    if is_running() {
        anyhow::bail!("当前进程已持有单实例锁");
    }

    let pid_file = pid_file_path();
    match unix_lock::try_lock(&pid_file) {
        Some(guard) => {
            if let Ok(mut g) = SINGLE_INSTANCE_GUARD.lock() {
                *g = Some(SingleInstanceGuard::UnixFile(guard));
            }
            Ok(())
        }
        None => anyhow::bail!("已有另一个 Hakimi Hub 实例在运行"),
    }
}

pub fn release_single_instance() {
    if let Ok(mut g) = SINGLE_INSTANCE_GUARD.lock() {
        *g = None;
    }
    remove_pid_file();
}

pub fn is_process_alive_pub(pid: u32) -> bool {
    is_process_alive(pid)
}

fn write_pid_file() -> std::io::Result<()> {
    let pid = std::process::id();
    let start_time = get_process_start_time(pid).unwrap_or_default();
    let content = format!("{}\n{}", pid, start_time);
    fs::write(pid_file_path(), content)
}

fn remove_pid_file() {
    let _ = fs::remove_file(pid_file_path());
}

pub fn read_pid_file() -> std::io::Result<Option<u32>> {
    let pid_file = pid_file_path();
    if !pid_file.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&pid_file)?;
    let pid: u32 = match content
        .trim()
        .splitn(2, '\n')
        .next()
        .and_then(|s| s.parse().ok())
    {
        Some(p) => p,
        None => {
            let _ = fs::remove_file(&pid_file);
            return Ok(None);
        }
    };
    Ok(Some(pid))
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        CloseHandle(handle);
        true
    }
}

#[cfg(unix)]
fn get_process_start_time(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let fields: Vec<&str> = stat.split_whitespace().collect();
    fields.get(21).map(|s| s.to_string())
}

#[cfg(windows)]
fn get_process_start_time(pid: u32) -> Option<String> {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[repr(C)]
    struct FileTime {
        dw_low_date_time: u32,
        dw_high_date_time: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> *mut c_void;
        fn GetProcessTimes(
            hProcess: *mut c_void,
            lpCreationTime: *mut FileTime,
            lpExitTime: *mut FileTime,
            lpKernelTime: *mut FileTime,
            lpUserTime: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }

        let mut creation = FileTime {
            dw_low_date_time: 0,
            dw_high_date_time: 0,
        };
        let mut exit = FileTime {
            dw_low_date_time: 0,
            dw_high_date_time: 0,
        };
        let mut kernel = FileTime {
            dw_low_date_time: 0,
            dw_high_date_time: 0,
        };
        let mut user = FileTime {
            dw_low_date_time: 0,
            dw_high_date_time: 0,
        };

        let result = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);

        if result == 0 {
            return None;
        }

        let start_ticks =
            ((creation.dw_high_date_time as u64) << 32) | (creation.dw_low_date_time as u64);
        Some(start_ticks.to_string())
    }
}
