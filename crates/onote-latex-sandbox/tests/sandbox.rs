//! Real sandbox verification on Windows (LaTeX plan step 2): the probe runs in
//! the AppContainer + Job and reports what it could do. Every test works only
//! in fresh temporary directories and deletes its own AppContainer profile.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use onote_latex_sandbox::{delete_profile, run, Exit, SandboxSpec};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    _base: tempfile::TempDir,
    profile: String,
    engine: PathBuf,
    input: PathBuf,
    output: PathBuf,
    outside: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = delete_profile(&self.profile);
    }
}

fn fixture() -> Fixture {
    let base = tempfile::tempdir().unwrap();
    let [engine, input, output, outside] =
        ["engine", "input", "output", "outside"].map(|name| base.path().join(name));
    for directory in [&engine, &input, &output, &outside] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::copy(
        env!("CARGO_BIN_EXE_sandbox-probe"),
        engine.join("probe.exe"),
    )
    .unwrap();
    // A machine's System32 copy of the VC runtime may not be readable by any
    // AppContainer; an app-local copy next to the program is what the loader
    // finds first, exactly as the engine directory will carry one.
    let runtime = Path::new(r"C:\Windows\System32\vcruntime140.dll");
    if runtime.is_file() {
        std::fs::copy(runtime, engine.join("vcruntime140.dll")).unwrap();
    }
    std::fs::write(input.join("main.tex"), b"\\documentclass{article}").unwrap();
    std::fs::write(outside.join("secret.txt"), b"outside secret").unwrap();
    Fixture {
        profile: format!(
            "opennote.sandboxtest.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        _base: base,
        engine,
        input,
        output,
        outside,
    }
}

fn spec(fixture: &Fixture, args: &[&str], timeout: Duration) -> SandboxSpec {
    SandboxSpec {
        profile: fixture.profile.clone(),
        program: fixture.engine.join("probe.exe"),
        args: args.iter().map(|arg| (*arg).into()).collect(),
        cwd: fixture.output.clone(),
        read_dirs: vec![fixture.engine.clone(), fixture.input.clone()],
        write_dirs: vec![fixture.output.clone()],
        network: onote_latex_sandbox::Network::None,
        byte_budget: None,
        attribute_only_dirs: Vec::new(),
        timeout,
        max_output_bytes: 1024 * 1024,
        env: Vec::new(),
    }
}

fn lines(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn path(value: &Path) -> &str {
    value.to_str().unwrap()
}

fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

#[test]
fn only_granted_directories_are_reachable() {
    let fixture = fixture();
    let secret = fixture.outside.join("secret.txt");
    let input = fixture.input.join("main.tex");
    let planted_input = fixture.input.join("planted.tex");
    let pdf = fixture.output.join("main.pdf");
    let planted_outside = fixture.outside.join("planted.txt");
    let args = [
        "read",
        path(&input),
        "read",
        path(&secret),
        "write",
        path(&planted_input),
        "write",
        path(&pdf),
        "write",
        path(&planted_outside),
    ];
    let result = run(
        &spec(&fixture, &args, Duration::from_secs(20)),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.exit, Exit::Exited(0), "{:?}", lines(&result.stdout));
    let report = lines(&result.stdout);
    assert_eq!(report[0], "read:ok", "granted input is readable");
    assert!(
        report[1].starts_with("read:denied"),
        "outside: {}",
        report[1]
    );
    assert!(
        report[2].starts_with("write:denied"),
        "input is read-only: {}",
        report[2]
    );
    assert_eq!(report[3], "write:ok", "the output directory is writable");
    assert!(
        report[4].starts_with("write:denied"),
        "outside: {}",
        report[4]
    );
    assert!(!fixture.input.join("planted.tex").exists());
    assert!(!fixture.outside.join("planted.txt").exists());
    assert_eq!(std::fs::read(&secret).unwrap(), b"outside secret");
    assert_eq!(result.remaining_processes, 0);
}

#[test]
fn the_network_is_unreachable_even_on_loopback() {
    let fixture = fixture();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let result = run(
        &spec(
            &fixture,
            &["connect", &address, "connect", "1.1.1.1:443"],
            Duration::from_secs(20),
        ),
        &AtomicBool::new(false),
    )
    .unwrap();
    let report = lines(&result.stdout);
    assert!(
        report[0].starts_with("connect:denied"),
        "loopback: {}",
        report[0]
    );
    assert!(
        report[1].starts_with("connect:denied"),
        "internet: {}",
        report[1]
    );
    // Nothing reached the listener.
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn a_child_cannot_break_away_and_a_timeout_kills_the_whole_tree() {
    let fixture = fixture();
    let started = Instant::now();
    let result = run(
        &spec(
            &fixture,
            &["breakaway", "spawn-hang", "hang"],
            Duration::from_secs(3),
        ),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.exit, Exit::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(15));
    let report = lines(&result.stdout);
    assert!(report[0].starts_with("breakaway:denied"), "{}", report[0]);
    let spawn = report[1]
        .strip_prefix("spawn:ok:")
        .expect("grandchild started");
    let pids: Vec<u32> = spawn.split(':').map(|pid| pid.parse().unwrap()).collect();
    assert_eq!(result.remaining_processes, 0);
    for pid in pids {
        assert!(!process_alive(pid), "process {pid} survived the job");
    }
}

#[test]
fn cancel_stops_the_run_and_leaves_no_process() {
    let fixture = fixture();
    let cancel = Arc::new(AtomicBool::new(false));
    let trigger = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        trigger.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let result = run(
        &spec(&fixture, &["spawn-hang", "hang"], Duration::from_secs(60)),
        &cancel,
    )
    .unwrap();
    canceller.join().unwrap();
    assert_eq!(result.exit, Exit::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "cancel was not prompt"
    );
    assert_eq!(result.remaining_processes, 0);
    let report = lines(&result.stdout);
    let spawn = report[0]
        .strip_prefix("spawn:ok:")
        .expect("grandchild started");
    for pid in spawn.split(':').map(|pid| pid.parse::<u32>().unwrap()) {
        assert!(!process_alive(pid), "process {pid} survived cancel");
    }
}

#[test]
fn output_is_bounded_and_drained_without_blocking() {
    let fixture = fixture();
    let result = run(
        &spec(
            &fixture,
            &["flood", "5242880", "exit", "7"],
            Duration::from_secs(30),
        ),
        &AtomicBool::new(false),
    )
    .unwrap();
    // The child is not blocked by a full pipe, so it reaches its own exit code.
    assert_eq!(result.exit, Exit::Exited(7));
    assert!(result.truncated);
    assert_eq!(result.stdout.len(), 1024 * 1024);
}

#[test]
fn a_byte_budget_stops_the_run_and_leaves_no_process() {
    let fixture = fixture();
    let big = fixture.output.join("big.bin");
    let mut limited = spec(
        &fixture,
        &["fill", path(&big), "16777216", "hang"],
        Duration::from_secs(60),
    );
    limited.byte_budget = Some((fixture.output.clone(), 1024 * 1024));
    let started = Instant::now();
    let result = run(&limited, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.exit, Exit::ByteBudgetExceeded);
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(result.remaining_processes, 0);
    // Stopped well before the 16 MiB it wanted to write.
    assert!(std::fs::metadata(&big).unwrap().len() < 16 * 1024 * 1024);
}

#[test]
fn a_relative_program_is_refused() {
    let fixture = fixture();
    let mut relative = spec(&fixture, &[], Duration::from_secs(5));
    relative.program = PathBuf::from("probe.exe");
    let error = run(&relative, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.stage, "relativePath");
}
