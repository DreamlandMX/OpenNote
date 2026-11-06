//! The approved exception (2026-10-30): the app's own AppContainer may read the
//! *attributes* of the user's profile folder and roaming AppData folder, so the
//! engine can confirm they exist. This proves that is all it adds: nothing inside
//! either folder can be listed, read or written, and the entry is not inherited.
//! The test revokes the entry and checks no trace of its SID remains.
//!
//! Whether a file's attributes can be queried *by path* is compared with a run
//! that has no grant, not asserted outright. On some Windows builds that query
//! succeeds under the real AppData folder for any AppContainer, whatever the
//! file's ACL says, and nothing this app does changes that (investigated
//! 2026-11-04). What this app must guarantee is that its
//! own entry gives the container nothing beyond the folder object itself.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use onote_latex_sandbox::{
    delete_profile, profile_folders, profile_sid, revoke, run, Exit, SandboxSpec,
};

struct Cleanup {
    profile: String,
    folders: Vec<PathBuf>,
    sentinels: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        // Revoke first: deleting the profile would orphan the entry.
        for folder in &self.folders {
            let _ = revoke(&self.profile, folder);
        }
        let _ = delete_profile(&self.profile);
        for path in &self.sentinels {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn icacls(path: &Path) -> String {
    let output = std::process::Command::new("icacls")
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn profile_folders_can_be_confirmed_but_not_entered() {
    let folders = profile_folders().unwrap();
    let (home, roaming) = (folders[0].clone(), folders[1].clone());
    let tag = format!("opennote-sandboxtest-{}", std::process::id());

    // Sentinels the restricted process must not reach: a file directly in
    // each folder and a subfolder with a file under roaming AppData.
    let home_file = home.join(format!(".{tag}.txt"));
    let roaming_file = roaming.join(format!("{tag}.txt"));
    let roaming_dir = roaming.join(&tag);
    let base = tempfile::tempdir().unwrap();
    let engine = base.path().join("engine");
    let output = base.path().join("output");
    std::fs::create_dir_all(&engine).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    let cleanup = Cleanup {
        profile: format!("opennote.sandboxtest.profile.{}", std::process::id()),
        folders: folders.clone(),
        sentinels: vec![home_file.clone(), roaming_file.clone(), roaming_dir.clone()],
    };
    std::fs::write(&home_file, b"home secret").unwrap();
    std::fs::write(&roaming_file, b"roaming secret").unwrap();
    std::fs::create_dir_all(&roaming_dir).unwrap();
    std::fs::write(roaming_dir.join("inner.txt"), b"inner secret").unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_sandbox-probe"),
        engine.join("probe.exe"),
    )
    .unwrap();
    let runtime = Path::new(r"C:\Windows\System32\vcruntime140.dll");
    if runtime.is_file() {
        std::fs::copy(runtime, engine.join("vcruntime140.dll")).unwrap();
    }

    let inner = roaming_dir.join("inner.txt");
    let planted = roaming.join(format!("{tag}-planted.txt"));
    let args = [
        "known-folders",
        "attrs",
        text(&home),
        "attrs",
        text(&roaming),
        "list",
        text(&home),
        "list",
        text(&roaming),
        "read",
        text(&home_file),
        "read",
        text(&roaming_file),
        "attrs",
        text(&roaming_file),
        "attrs",
        text(&home_file),
        "list",
        text(&roaming_dir),
        "read",
        text(&inner),
        "write",
        text(&planted),
    ];
    let probe = |profile: &str, attribute_only_dirs: Vec<PathBuf>| -> Vec<String> {
        let spec = SandboxSpec {
            profile: profile.to_owned(),
            program: engine.join("probe.exe"),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            cwd: output.clone(),
            read_dirs: vec![engine.clone()],
            write_dirs: vec![output.clone()],
            network: onote_latex_sandbox::Network::None,
            byte_budget: None,
            attribute_only_dirs,
            timeout: Duration::from_secs(20),
            max_output_bytes: 64 * 1024,
            env: Vec::new(),
        };
        let result = run(&spec, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.exit, Exit::Exited(0));
        let report: Vec<String> = String::from_utf8_lossy(&result.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        eprintln!("{profile} {report:#?}");
        report
    };

    // What an identical container can do with no entry at all.
    let ungranted_profile = format!("opennote.sandboxtest.nogrant.{}", std::process::id());
    let _ungranted_cleanup = Cleanup {
        profile: ungranted_profile.clone(),
        folders: Vec::new(),
        sentinels: Vec::new(),
    };
    let ungranted = probe(&ungranted_profile, Vec::new());
    let report = probe(&cleanup.profile, folders.clone());

    // The verified lookup the engine depends on now succeeds.
    for line in &report[..3] {
        assert!(line.contains(":0x0:"), "known folder lookup failed: {line}");
    }
    let (rest, baseline) = (&report[3..], &ungranted[3..]);
    assert_eq!(rest.len(), 11);
    assert_eq!(baseline.len(), rest.len());
    assert_eq!(rest[0], "attrs:ok:dir=true", "profile folder exists");
    assert_eq!(rest[1], "attrs:ok:dir=true", "roaming folder exists");
    // The entry is what opens the profile folder: without it, the query is refused.
    assert!(
        baseline[0].contains(":denied:"),
        "the profile folder is closed without the entry, got {}",
        baseline[0]
    );

    let denied = [
        (2, "list profile"),
        (3, "list roaming"),
        (4, "read profile file"),
        (5, "read roaming file"),
        (8, "list roaming subfolder"),
        (9, "read roaming subfolder file"),
        (10, "write into roaming"),
    ];
    for (index, what) in denied {
        assert!(
            rest[index].contains(":denied:"),
            "{what} must be denied, got {}",
            rest[index]
        );
    }

    // Files' attributes: the entry adds nothing to what any container could see.
    for (index, what) in [(6, "a roaming file"), (7, "a profile file")] {
        assert_eq!(
            rest[index], baseline[index],
            "the entry must not change access to the attributes of {what}"
        );
    }
    assert!(!planted.exists());

    // The entry is exactly read-attributes on each folder, not inherited, and
    // it is not on anything inside.
    let sid = profile_sid(&cleanup.profile).unwrap();
    for folder in &folders {
        let acl = icacls(folder);
        let entry = acl
            .lines()
            .find(|line| line.contains(&sid))
            .unwrap_or_else(|| panic!("no entry on {}:\n{acl}", folder.display()));
        assert!(
            entry.trim_end().ends_with(&format!("{sid}:(RA)")),
            "unexpected entry: {entry}"
        );
    }
    for inside in [&home_file, &roaming_file, &roaming_dir, &inner] {
        assert!(
            !icacls(inside).contains(&sid),
            "{} carries the entry",
            inside.display()
        );
    }

    // Removal leaves no trace of the SID on either folder.
    for folder in &folders {
        revoke(&cleanup.profile, folder).unwrap();
        assert!(
            !icacls(folder).contains(&sid),
            "entry left on {}",
            folder.display()
        );
    }
    drop(cleanup);
}
