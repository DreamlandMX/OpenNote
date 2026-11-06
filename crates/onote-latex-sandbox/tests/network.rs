//! The preparation profile's network: `internetClient` only. It must still be
//! unable to reach this machine (loopback) or the local network, and the
//! compile profile must reach nothing at all.
#![cfg(windows)]

use std::net::{IpAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use onote_latex_sandbox::{delete_profile, run, Exit, Network, SandboxSpec};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    _base: tempfile::TempDir,
    profile: String,
    engine: PathBuf,
    output: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = delete_profile(&self.profile);
    }
}

fn fixture() -> Fixture {
    let base = tempfile::tempdir().unwrap();
    let engine = base.path().join("engine");
    let output = base.path().join("output");
    std::fs::create_dir_all(&engine).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_sandbox-probe"),
        engine.join("probe.exe"),
    )
    .unwrap();
    let runtime = Path::new(r"C:\Windows\System32\vcruntime140.dll");
    if runtime.is_file() {
        std::fs::copy(runtime, engine.join("vcruntime140.dll")).unwrap();
    }
    Fixture {
        profile: format!(
            "opennote.sandboxtest.net.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        _base: base,
        engine,
        output,
    }
}

fn probe(fixture: &Fixture, network: Network, args: &[String]) -> Vec<String> {
    let spec = SandboxSpec {
        profile: fixture.profile.clone(),
        program: fixture.engine.join("probe.exe"),
        args: args.iter().map(Into::into).collect(),
        cwd: fixture.output.clone(),
        read_dirs: vec![fixture.engine.clone()],
        write_dirs: vec![fixture.output.clone()],
        attribute_only_dirs: Vec::new(),
        timeout: Duration::from_secs(30),
        max_output_bytes: 64 * 1024,
        env: Vec::new(),
        network,
        byte_budget: None,
    };
    let result = run(&spec, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.exit, Exit::Exited(0));
    assert_eq!(result.remaining_processes, 0);
    String::from_utf8_lossy(&result.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// This machine's address on its local network (no packet is sent: a UDP
/// "connect" only picks the outgoing interface).
fn lan_address() -> IpAddr {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("192.0.2.1:9").unwrap();
    let address = socket.local_addr().unwrap().ip();
    assert!(
        !address.is_loopback() && !address.is_unspecified(),
        "no LAN address: {address}"
    );
    address
}

fn listener(address: IpAddr) -> TcpListener {
    let listener = TcpListener::bind((address, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    listener
}

#[test]
fn internet_capability_still_cannot_reach_loopback_or_the_lan() {
    let fixture = fixture();
    let loopback = listener(IpAddr::from([127, 0, 0, 1]));
    let lan = listener(lan_address());
    let args = vec![
        "connect".to_owned(),
        loopback.local_addr().unwrap().to_string(),
        "connect".to_owned(),
        lan.local_addr().unwrap().to_string(),
    ];
    let report = probe(&fixture, Network::InternetClientOnly, &args);
    assert!(
        report[0].starts_with("connect:denied"),
        "loopback: {}",
        report[0]
    );
    assert!(
        report[1].starts_with("connect:denied"),
        "LAN: {}",
        report[1]
    );
    for socket in [&loopback, &lan] {
        assert_eq!(
            socket.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "a connection reached {:?}",
            socket.local_addr()
        );
    }
}

#[test]
fn the_compile_profile_cannot_reach_the_lan_either() {
    let fixture = fixture();
    let lan = listener(lan_address());
    let report = probe(
        &fixture,
        Network::None,
        &["connect".to_owned(), lan.local_addr().unwrap().to_string()],
    );
    assert!(
        report[0].starts_with("connect:denied"),
        "LAN: {}",
        report[0]
    );
}

#[test]
#[ignore = "needs internet access"]
fn internet_capability_reaches_the_internet_and_none_does_not() {
    let fixture = fixture();
    let target = vec!["connect".to_owned(), "1.1.1.1:443".to_owned()];
    let allowed = probe(&fixture, Network::InternetClientOnly, &target);
    assert_eq!(
        allowed[0], "connect:ok",
        "preparation must reach the internet"
    );
    let refused = probe(&fixture, Network::None, &target);
    assert!(refused[0].starts_with("connect:denied"), "{}", refused[0]);
}
