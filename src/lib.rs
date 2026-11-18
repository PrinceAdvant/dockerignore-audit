//! Check a Docker build context against its `.dockerignore`.
//!
//! Everything in the build context that `.dockerignore` does not exclude is
//! sent to the Docker daemon, and a broad `COPY . .` puts it into the image.
//! That is how `.env` files, private keys, the whole `.git` history and a
//! local `node_modules` end up in production images. This crate parses
//! `.dockerignore` with Docker's rules (patterns are relative to the context
//! root, `**` matches any number of directories, later `!` lines re-include)
//! and reports which risky paths would still be sent, plus the total size of
//! the context.
//!
//! ```
//! use dockerignore_audit::Rules;
//! let rules = Rules::parse("node_modules\n**/*.log\n.env*\n!.env.example\n");
//! assert!(rules.is_ignored("node_modules/react/index.js"));
//! assert!(!rules.is_ignored("packages/web/node_modules/x.js")); // root only
//! assert!(rules.is_ignored("logs/app/server.log"));
//! assert!(rules.is_ignored(".env.production"));
//! assert!(!rules.is_ignored(".env.example"));
//! ```
//!
//! Smaller and safer build contexts are one part of the guide on how to
//! [reduce Docker image size](https://peon.sh/blogs/reduce-docker-image-size).
//! Maintained by a contributor to [Peon](https://peon.sh/), an open-source
//! self-hosted deployment platform.

use std::fs;
use std::io;
use std::path::Path;

/// One `.dockerignore` line.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// Pattern split into path segments, already cleaned.
    pub segments: Vec<String>,
    /// True for `!pattern` lines that re-include paths.
    pub negate: bool,
}

/// A parsed `.dockerignore` file.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    pub rules: Vec<Rule>,
}

fn clean(p: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s.to_string()),
        }
    }
    out
}

/// Match one path segment against a pattern segment with `*` and `?`.
pub fn match_segment(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn match_segments(pat: &[String], path: &[&str]) -> bool {
    match pat.first() {
        None => path.is_empty(),
        Some(s) if s == "**" => (0..=path.len()).any(|i| match_segments(&pat[1..], &path[i..])),
        Some(s) => !path.is_empty() && match_segment(s, path[0]) && match_segments(&pat[1..], &path[1..]),
    }
}

impl Rule {
    /// True when the pattern matches the path or one of its parent directories.
    pub fn matches(&self, path: &str) -> bool {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
        (1..=segs.len()).any(|end| match_segments(&self.segments, &segs[..end]))
    }
}

impl Rules {
    /// Parse `.dockerignore` text. Blank lines and `#` comments are skipped.
    pub fn parse(text: &str) -> Rules {
        let mut rules = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negate, pat) = match line.strip_prefix('!') {
                Some(rest) => (true, rest.trim()),
                None => (false, line),
            };
            let segments = clean(pat);
            if !segments.is_empty() {
                rules.push(Rule { segments, negate });
            }
        }
        Rules { rules }
    }

    /// Docker semantics: the last matching rule wins.
    pub fn is_ignored(&self, path: &str) -> bool {
        let mut ignored = false;
        for r in &self.rules {
            if r.matches(path) {
                ignored = !r.negate;
            }
        }
        ignored
    }
}

/// A risky path that would be sent to the daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub path: String,
    pub reason: &'static str,
}

/// Classify a relative path; returns a reason when it is worth excluding.
pub fn risk(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    if lower == ".env" || (lower.starts_with(".env.") && !lower.ends_with(".example") && !lower.ends_with(".sample")) {
        return Some("environment file, may hold secrets");
    }
    if lower.ends_with(".pem") || lower.ends_with(".key") || lower.ends_with(".p12") || lower == "id_rsa" || lower == "id_ed25519" {
        return Some("private key or certificate");
    }
    match lower.as_str() {
        ".git" => Some("Git history, often large and may contain removed secrets"),
        "node_modules" => Some("host dependencies; install inside the image instead"),
        "target" => Some("Rust or Java build output"),
        "__pycache__" | ".venv" | "venv" => Some("Python cache or virtualenv from the host"),
        ".next" => Some("Next.js build cache from the host"),
        "coverage" | ".nyc_output" => Some("test coverage output"),
        ".terraform" => Some("Terraform providers and possibly state"),
        _ if lower.ends_with(".tfstate") => Some("Terraform state, may hold secrets"),
        _ => None,
    }
}

/// Result of auditing a build context directory.
#[derive(Debug, Default)]
pub struct Report {
    pub files_sent: u64,
    pub bytes_sent: u64,
    pub files_ignored: u64,
    pub findings: Vec<Finding>,
    pub has_dockerignore: bool,
}

/// Walk `dir` and apply its `.dockerignore`. Risky directories are reported
/// once, at the top-most path that would be sent.
pub fn audit(dir: &Path) -> io::Result<Report> {
    let text = fs::read_to_string(dir.join(".dockerignore")).ok();
    let mut rep = Report { has_dockerignore: text.is_some(), ..Default::default() };
    let rules = Rules::parse(text.as_deref().unwrap_or(""));
    walk(dir, "", &rules, &mut rep)?;
    Ok(rep)
}

fn walk(root: &Path, rel: &str, rules: &Rules, rep: &mut Report) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(root.join(rel))?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let ft = e.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        let ignored = rules.is_ignored(&path);
        if !ignored {
            if let Some(reason) = risk(&path) {
                rep.findings.push(Finding { path: path.clone(), reason });
            }
        }
        if ft.is_dir() {
            walk(root, &path, rules, rep)?;
        } else if ignored {
            rep.files_ignored += 1;
        } else {
            rep.files_sent += 1;
            rep.bytes_sent += e.metadata()?.len();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments() {
        assert!(match_segment("*.log", "a.log"));
        assert!(match_segment("a?c", "abc"));
        assert!(!match_segment("*.log", "a.txt"));
    }

    #[test]
    fn docker_semantics() {
        let r = Rules::parse("# comment\n/dist\n**/node_modules\n*.md\n!README.md\n");
        assert!(r.is_ignored("dist/app.js"));
        assert!(r.is_ignored("a/b/node_modules/x"));
        assert!(r.is_ignored("CHANGELOG.md"));
        assert!(!r.is_ignored("README.md"));
        assert!(!r.is_ignored("docs/guide.md")); // *.md is root only
    }

    #[test]
    fn risks() {
        assert!(risk(".env").is_some());
        assert!(risk("config/.env.production").is_some());
        assert!(risk(".env.example").is_none());
        assert!(risk("certs/server.key").is_some());
        assert!(risk("src/main.rs").is_none());
    }

    #[test]
    fn audit_dir() {
        let d = std::env::temp_dir().join(format!("dia-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("node_modules/x")).unwrap();
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("node_modules/x/i.js"), "x").unwrap();
        fs::write(d.join("src/app.js"), "hello").unwrap();
        fs::write(d.join(".env"), "SECRET=1").unwrap();
        fs::write(d.join(".dockerignore"), "node_modules\n").unwrap();
        let rep = audit(&d).unwrap();
        assert!(rep.has_dockerignore);
        assert_eq!(rep.findings.len(), 1);
        assert_eq!(rep.findings[0].path, ".env");
        assert_eq!(rep.files_ignored, 1);
        assert_eq!(rep.files_sent, 3);
        fs::remove_dir_all(&d).unwrap();
    }
}
