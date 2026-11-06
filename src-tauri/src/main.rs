#![cfg_attr(not(dev), windows_subsystem = "windows")]

fn main() {
    // Run by the uninstaller before files are removed: revoke the LaTeX
    // sandbox's profile-folder entries, delete its AppContainer profiles and
    // its runtime directory, then exit without opening a window.
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--latex-uninstall-cleanup")
    {
        std::process::exit(opennote_desktop::latex_uninstall_cleanup());
    }
    // Run only by the host, inside the no-network sandbox, on one candidate
    // PDF: the structural check never runs in the GUI process.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(opennote_desktop::LATEX_VALIDATE_PDF_FLAG) {
        std::process::exit(opennote_desktop::latex_validate_pdf(&args[1..]));
    }
    opennote_desktop::run();
}
