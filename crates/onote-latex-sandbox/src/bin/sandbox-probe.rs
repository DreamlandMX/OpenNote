//! Test-only probe run inside the sandbox. Each step tries something the
//! sandbox must allow or refuse and prints one `step:result` line, so the test
//! asserts what actually happened inside the restricted process.
//!
//! Steps: `read <path>`, `list <dir>`, `attrs <path>`, `write <path>`,
//! `connect <host:port>`, `known-folders`, `breakaway`,
//! `spawn-hang`, `hang`, `flood <bytes>`, `exit <code>`.

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

fn report(step: &str, result: Result<String, std::io::Error>) {
    match result {
        Ok(detail) => println!("{step}:ok{detail}"),
        Err(error) => println!("{step}:denied:{:?}", error.kind()),
    }
    let _ = std::io::stdout().flush();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        let step = args[index].as_str();
        let value = args.get(index + 1).cloned().unwrap_or_default();
        match step {
            "read" => {
                report("read", std::fs::read(&value).map(|_| String::new()));
                index += 2;
            }
            "list" => {
                report(
                    "list",
                    std::fs::read_dir(&value).map(|entries| format!(":{}", entries.count())),
                );
                index += 2;
            }
            "attrs" => {
                // GetFileAttributesW is the existence check Windows itself uses
                // to verify a known folder; it needs only FILE_READ_ATTRIBUTES.
                use std::os::windows::ffi::OsStrExt;
                use windows_sys::Win32::Storage::FileSystem::{
                    GetFileAttributesW, FILE_ATTRIBUTE_DIRECTORY, INVALID_FILE_ATTRIBUTES,
                };
                let name: Vec<u16> = std::ffi::OsStr::new(&value)
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
                let attributes = unsafe { GetFileAttributesW(name.as_ptr()) };
                let result = if attributes == INVALID_FILE_ATTRIBUTES {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(format!(
                        ":dir={}",
                        attributes & FILE_ATTRIBUTE_DIRECTORY != 0
                    ))
                };
                report("attrs", result);
                index += 2;
            }
            "write" => {
                report(
                    "write",
                    std::fs::write(&value, b"probe").map(|()| String::new()),
                );
                index += 2;
            }
            "connect" => {
                let result = value
                    .parse::<SocketAddr>()
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
                    .and_then(|address| {
                        TcpStream::connect_timeout(&address, Duration::from_secs(3))
                    })
                    .map(|_| String::new());
                report("connect", result);
                index += 2;
            }
            "breakaway" => {
                use std::os::windows::process::CommandExt;
                const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
                let result = std::env::current_exe().and_then(|exe| {
                    std::process::Command::new(exe)
                        .args(["exit", "0"])
                        .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
                        .spawn()
                        .map(|mut child| {
                            let _ = child.wait();
                            String::new()
                        })
                });
                report("breakaway", result);
                index += 1;
            }
            "spawn-hang" => {
                // A grandchild that would outlive us if the job did not kill it.
                let result = std::env::current_exe().and_then(|exe| {
                    std::process::Command::new(exe)
                        .arg("hang")
                        .spawn()
                        .map(|child| format!(":{}:{}", std::process::id(), child.id()))
                });
                report("spawn", result);
                index += 1;
            }
            "known-folders" => {
                use windows_sys::Win32::UI::Shell::{
                    FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_RoamingAppData,
                    SHGetKnownFolderPath,
                };
                for (name, id) in [
                    ("roaming", FOLDERID_RoamingAppData),
                    ("local", FOLDERID_LocalAppData),
                    ("profile", FOLDERID_Profile),
                ] {
                    let mut path = std::ptr::null_mut();
                    let result =
                        unsafe { SHGetKnownFolderPath(&id, 0, std::ptr::null_mut(), &mut path) };
                    let text = if result == 0 {
                        let mut length = 0;
                        while unsafe { *path.add(length) } != 0 {
                            length += 1;
                        }
                        String::from_utf16_lossy(unsafe {
                            std::slice::from_raw_parts(path, length)
                        })
                    } else {
                        String::new()
                    };
                    unsafe { windows_sys::Win32::System::Com::CoTaskMemFree(path as *const _) };
                    println!("folder:{name}:{:#x}:{text}", result as u32);
                }
                index += 1;
            }
            "hang" => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            "fill" => {
                // Writes `count` bytes to `path` in small steps, flushing as it
                // goes, so a byte budget on the directory can catch it mid-way.
                let count: usize = args
                    .get(index + 2)
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                let result = std::fs::File::create(&value).and_then(|mut file| {
                    let chunk = vec![b'f'; 256 * 1024];
                    let mut written = 0;
                    while written < count {
                        let take = chunk.len().min(count - written);
                        file.write_all(&chunk[..take])?;
                        file.flush()?;
                        written += take;
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Ok(format!(":{written}"))
                });
                report("fill", result);
                index += 3;
            }
            "flood" => {
                let total: usize = value.parse().unwrap_or(0);
                let chunk = vec![b'x'; 64 * 1024];
                let mut out = std::io::stdout();
                let mut written = 0;
                while written < total {
                    let take = chunk.len().min(total - written);
                    if out.write_all(&chunk[..take]).is_err() {
                        break;
                    }
                    written += take;
                }
                let _ = out.flush();
                index += 2;
            }
            "exit" => std::process::exit(value.parse().unwrap_or(0)),
            other => {
                println!("unknown:{other}");
                index += 1;
            }
        }
    }
}
