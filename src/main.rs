use std::path::PathBuf;
use std::process::ExitCode;

fn human(b: u64) -> String {
    let units = ["B", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

fn main() -> ExitCode {
    let mut dir = PathBuf::from(".");
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "-h" | "--help" => {
                eprintln!("usage: dockerignore-audit [DIR]");
                eprintln!("Applies DIR/.dockerignore and lists risky paths that would still be sent to Docker.");
                return ExitCode::from(2);
            }
            _ => dir = PathBuf::from(a),
        }
    }
    let rep = match dockerignore_audit::audit(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}: {e}", dir.display());
            return ExitCode::from(2);
        }
    };
    if !rep.has_dockerignore {
        println!("no .dockerignore found: the whole directory is sent");
    }
    println!(
        "context: {} files, {} sent; {} files excluded",
        rep.files_sent,
        human(rep.bytes_sent),
        rep.files_ignored
    );
    for f in &rep.findings {
        println!("  send  {:<40} {}", f.path, f.reason);
    }
    if rep.findings.is_empty() {
        println!("no risky paths in the context");
        ExitCode::SUCCESS
    } else {
        eprintln!("{} risky path(s) would be sent; add them to .dockerignore", rep.findings.len());
        ExitCode::from(1)
    }
}
