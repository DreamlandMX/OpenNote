//! The real engine inside the sandbox (LaTeX plan step 2). Opt-in, because the
//! engine is not part of the repository: set `ONOTE_TECTONIC_EXE` to the
//! extracted `tectonic.exe` and run with `--ignored`. The binary is checked
//! against the hash recorded from the verified official archive before it is
//! started, and it is only ever started through the sandbox.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use onote_latex_sandbox::{delete_profile, run, Exit, SandboxSpec};
use sha2::{Digest, Sha256};

/// `tectonic.exe` inside tectonic-0.17.0-x86_64-pc-windows-msvc.zip
/// (archive SHA-256 f61ce51f0b0ade1015b7de7ef368541c5424e9756ecbd0d7af97d6d48030845f).
const ENGINE_SHA256: &str = "99ffcfdbf1ebf8bdda9e791942e3d06aedb12463fddc33f07de6f5211c8bf08d";

struct Profile(String);

impl Drop for Profile {
    fn drop(&mut self) {
        // Revoke the approved profile-folder entries before the profile goes.
        for folder in onote_latex_sandbox::profile_folders().unwrap_or_default() {
            let _ = onote_latex_sandbox::revoke(&self.0, &folder);
        }
        let _ = delete_profile(&self.0);
    }
}

fn engine() -> PathBuf {
    let path =
        PathBuf::from(std::env::var_os("ONOTE_TECTONIC_EXE").expect("set ONOTE_TECTONIC_EXE"));
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(&bytes)),
        ENGINE_SHA256,
        "refusing to start an engine that is not the verified 0.17.0 binary"
    );
    path
}

fn spec(
    profile: &str,
    engine: &std::path::Path,
    args: Vec<std::ffi::OsString>,
    work: &std::path::Path,
    input: &std::path::Path,
    cache: &std::path::Path,
) -> SandboxSpec {
    SandboxSpec {
        profile: profile.to_owned(),
        program: engine.to_owned(),
        args,
        cwd: work.to_owned(),
        read_dirs: vec![engine.parent().unwrap().to_owned(), input.to_owned()],
        write_dirs: vec![work.to_owned(), cache.to_owned()],
        network: onote_latex_sandbox::Network::None,
        byte_budget: None,
        attribute_only_dirs: onote_latex_sandbox::profile_folders().unwrap(),
        timeout: Duration::from_secs(60),
        max_output_bytes: 1024 * 1024,
        env: vec![(
            "TECTONIC_CACHE_DIR".to_owned(),
            cache.as_os_str().to_owned(),
        )],
    }
}

fn tree_bytes(directory: &std::path::Path) -> (u64, usize) {
    let (mut bytes, mut files) = (0u64, 0usize);
    let mut stack = vec![directory.to_owned()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).unwrap().flatten() {
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

/// Measures a real preparation: online in the internet-only profile, then an
/// offline compile in a no-network profile with the cache **read-only**.
#[test]
#[ignore = "needs ONOTE_TECTONIC_EXE and internet access"]
fn preparing_online_then_compiling_offline_with_a_read_only_cache() {
    let engine = engine();
    let base = tempfile::tempdir().unwrap();
    let [work, input, cache, offline] =
        ["work", "input", "cache", "offline"].map(|name| base.path().join(name));
    for directory in [&work, &input, &cache, &offline] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        input.join("main.tex"),
        "\\documentclass[11pt,a4paper]{article}\n\\usepackage[margin=25mm]{geometry}\n\
\\usepackage[T1]{fontenc}\n\\usepackage[utf8]{inputenc}\n\\usepackage{graphicx}\n\
\\usepackage{hyperref}\n\\title{Probe}\n\\date{2026-10-30}\n\\begin{document}\n\\maketitle\n\
\\section{Introduction}\nHello $e^{i\\pi}+1=0$.\n\\end{document}\n",
    )
    .unwrap();
    let prepare = Profile(format!("opennote.sandboxtest.prep.{}", std::process::id()));
    let compile = Profile(format!("opennote.sandboxtest.comp.{}", std::process::id()));
    let folders = onote_latex_sandbox::profile_folders().unwrap();
    let args = |out: &std::path::Path, only_cached: bool| {
        let mut args: Vec<std::ffi::OsString> = vec!["-X".into(), "compile".into()];
        if only_cached {
            args.push("--only-cached".into());
        }
        args.extend([
            "--untrusted".into(),
            "--synctex".into(),
            "--outdir".into(),
            out.as_os_str().to_owned(),
            input.join("main.tex").into_os_string(),
        ]);
        args
    };

    let started = Instant::now();
    let online = run(
        &SandboxSpec {
            profile: prepare.0.clone(),
            program: engine.clone(),
            args: args(&work, false),
            cwd: work.clone(),
            read_dirs: vec![engine.parent().unwrap().to_owned(), input.clone()],
            write_dirs: vec![work.clone(), cache.clone()],
            attribute_only_dirs: folders.clone(),
            timeout: Duration::from_secs(600),
            max_output_bytes: 1024 * 1024,
            env: vec![(
                "TECTONIC_CACHE_DIR".to_owned(),
                cache.as_os_str().to_owned(),
            )],
            network: onote_latex_sandbox::Network::InternetClientOnly,
            byte_budget: Some((cache.clone(), 1024 * 1024 * 1024)),
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    let (bytes, files) = tree_bytes(&cache);
    eprintln!(
        "PREPARE exit={:?} seconds={} cacheBytes={bytes} cacheFiles={files} remaining={}\n{}",
        online.exit,
        started.elapsed().as_secs(),
        online.remaining_processes,
        String::from_utf8_lossy(&online.stderr)
    );
    assert_eq!(online.exit, Exit::Exited(0));
    assert!(work.join("main.pdf").is_file());

    let offline_run = run(
        &SandboxSpec {
            profile: compile.0.clone(),
            program: engine.clone(),
            args: args(&offline, true),
            cwd: offline.clone(),
            // The cache is input now: readable, not writable.
            read_dirs: vec![
                engine.parent().unwrap().to_owned(),
                input.clone(),
                cache.clone(),
            ],
            write_dirs: vec![offline.clone()],
            attribute_only_dirs: folders.clone(),
            timeout: Duration::from_secs(120),
            max_output_bytes: 1024 * 1024,
            env: vec![(
                "TECTONIC_CACHE_DIR".to_owned(),
                cache.as_os_str().to_owned(),
            )],
            network: onote_latex_sandbox::Network::None,
            byte_budget: None,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    eprintln!(
        "OFFLINE exit={:?} remaining={}\n{}",
        offline_run.exit,
        offline_run.remaining_processes,
        String::from_utf8_lossy(&offline_run.stderr)
    );
    let pdf = std::fs::read(offline.join("main.pdf")).expect("offline PDF");
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(offline.join("main.synctex.gz").is_file());
    assert_eq!(offline_run.exit, Exit::Exited(0));
    assert_eq!(
        tree_bytes(&cache),
        (bytes, files),
        "the offline compile wrote nothing to the cache"
    );
}

#[test]
#[ignore = "needs ONOTE_TECTONIC_EXE"]
fn the_engine_runs_restricted_and_reports_missing_resources_offline() {
    let engine = engine();
    let base = tempfile::tempdir().unwrap();
    let [work, input, cache] = ["work", "input", "cache"].map(|name| base.path().join(name));
    for directory in [&work, &input, &cache] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        input.join("main.tex"),
        "\\documentclass{article}\n\\begin{document}\nHello.\n\\end{document}\n",
    )
    .unwrap();
    let profile = Profile(format!(
        "opennote.sandboxtest.tectonic.{}",
        std::process::id()
    ));

    let version = run(
        &spec(
            &profile.0,
            &engine,
            vec!["--version".into()],
            &work,
            &input,
            &cache,
        ),
        &AtomicBool::new(false),
    )
    .unwrap();
    let stdout = String::from_utf8_lossy(&version.stdout);
    assert_eq!(
        version.exit,
        Exit::Exited(0),
        "{stdout} {}",
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(stdout.contains("0.17.0"), "{stdout}");
    assert_eq!(version.remaining_processes, 0);

    // No resources prepared and no network: the build must fail with a
    // message about the missing bundle, promptly, and produce no PDF.
    let started = Instant::now();
    let compile = run(
        &spec(
            &profile.0,
            &engine,
            vec![
                "-X".into(),
                "compile".into(),
                "--only-cached".into(),
                "--untrusted".into(),
                "--outdir".into(),
                work.as_os_str().to_owned(),
                input.join("main.tex").into_os_string(),
            ],
            &work,
            &input,
            &cache,
        ),
        &AtomicBool::new(false),
    )
    .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    eprintln!("ENGINE LOG:\n{log}");
    // Failing for the wrong reason is not a pass: inside the AppContainer the
    // engine must get as far as looking for its resources.
    assert!(
        !log.contains("Unable to find standard directories"),
        "engine cannot resolve its config directory inside the AppContainer: {log}"
    );
    // It reaches its resource lookup and is refused the network.
    assert!(log.contains("this bundle isn't cached"), "{log}");
    assert!(log.contains("couldn't get it from the internet"), "{log}");
    assert!(
        matches!(compile.exit, Exit::Exited(code) if code != 0),
        "{:?}",
        compile.exit
    );
    assert!(
        started.elapsed() < Duration::from_secs(55),
        "offline failure must not hang"
    );
    assert!(!work.join("main.pdf").exists());
    assert_eq!(compile.remaining_processes, 0);
}
