//! `ccx init` — scaffold a solution folder.
//!
//! A solution folder is the unit of work: one `ccx.toml` naming the problem, one
//! source file, and whatever else the language needs. The scaffold is written
//! so that `ccx test` works immediately.

use crate::cmd::{self, Fixable};
use crate::context::SOLUTION_MARKER;
use crate::Format;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    pub dir: String,
    pub problem: String,
    pub language: String,
    pub files_written: Vec<String>,
    pub next_steps: Vec<String>,
}

/// Locate the problem folder that `--problem` names, searching upward from the
/// solution directory. Problems live in the repo, not in the solution, so a
/// solution anywhere in the tree can point at one.
fn find_problem(from: &Path, id: &str) -> Option<std::path::PathBuf> {
    let mut dir = Some(from.to_path_buf());
    while let Some(d) = dir {
        let candidate = d.join("problems").join(id);
        if candidate.join("problem.json").is_file() {
            return Some(candidate);
        }
        let direct = d.join(id);
        if direct.join("problem.json").is_file() {
            return Some(direct);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

pub fn run(
    format: Format,
    problem: Option<&str>,
    language: &str,
    dir: &Path,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let root = dir.canonicalize()?;

    if root.join(SOLUTION_MARKER).exists() {
        return Err(Fixable::new(format!(
            "{} already contains a {SOLUTION_MARKER}",
            root.display()
        ))
        .fix("choose an empty directory, or edit the existing solution")
        .into_error());
    }

    let problem_id = match problem {
        Some(p) => p.to_string(),
        None => {
            // With no problem named, offer the ones that exist rather than
            // inventing one.
            let mut found = Vec::new();
            let mut d = Some(root.clone());
            while let Some(cur) = d {
                let pdir = cur.join("problems");
                if pdir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&pdir) {
                        for e in entries.flatten() {
                            if e.path().join("problem.json").is_file() {
                                found.push(e.file_name().to_string_lossy().to_string());
                            }
                        }
                    }
                }
                d = cur.parent().map(|p| p.to_path_buf());
            }
            found.sort();
            let mut f = Fixable::new("no problem named");
            if found.is_empty() {
                f = f.fix("no problems were found near here either");
                f = f.fix("create one, or pass --problem <id>");
            } else {
                f = f.fix(format!("available problems: {}", found.join(", ")));
                f = f.fix("pass one with --problem <id>");
            }
            return Err(f.into_error());
        }
    };

    let problem_path = find_problem(&root, &problem_id).ok_or_else(|| {
        Fixable::new(format!("could not find the problem {problem_id:?}"))
            .fix("problems live in a `problems/<id>/` folder containing problem.json")
            .fix("check the id is spelled correctly")
            .into_error()
    })?;

    // The config records a relative path, so a solution stays portable.
    let rel = pathdiff(&root, &problem_path);

    let (ext, source) = match language {
        "c" => (
            "c",
            r#"#include <stdio.h>

int main(void) {
    /* Read the input, solve the problem, print the answer. */
    return 0;
}
"#,
        ),
        "asm" => (
            "s",
            r#"/* x86-64 assembly. The ABI is the problem's declared one. */
.globl main
main:
    xor %eax, %eax
    ret
"#,
        ),
        _ => (
            "cpp",
            r#"#include <bits/stdc++.h>
using namespace std;

int main() {
    ios::sync_with_stdio(false);
    cin.tie(nullptr);
    /* Read the input, solve the problem, print the answer. */
    return 0;
}
"#,
        ),
    };

    let mut written = Vec::new();

    let config = format!(
        "# CodeCurrency solution.\n\
         problem = \"{rel}\"\n\
         language = \"{language}\"\n\
         toolchain = \"gcc\"\n\
         opt_level = \"-O2\"\n"
    );
    std::fs::write(root.join(SOLUTION_MARKER), config)?;
    written.push(SOLUTION_MARKER.to_string());

    let src_name = format!("main.{ext}");
    std::fs::write(root.join(&src_name), source)?;
    written.push(src_name.clone());

    // `.ccx` holds build output and must never be committed.
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, ".ccx/\ncapsule.json\n")?;
        written.push(".gitignore".to_string());
    }

    let report = InitReport {
        dir: root.display().to_string(),
        problem: problem_id,
        language: language.to_string(),
        files_written: written,
        next_steps: vec![
            format!("edit {src_name} to solve the problem"),
            "ccx test".to_string(),
            "ccx measure --repeat 5".to_string(),
        ],
    };

    cmd::emit(
        format,
        || {
            println!("initialised a solution in {}", report.dir);
            println!("  problem  {}", report.problem);
            println!("  language {}", report.language);
            println!();
            println!("  next:");
            for s in &report.next_steps {
                println!("    {s}");
            }
        },
        &report,
    )
}

/// A path relative to `base`, without pulling in a dependency for it.
fn pathdiff(base: &Path, target: &Path) -> String {
    let base_parts: Vec<_> = base.components().collect();
    let target_parts: Vec<_> = target.components().collect();

    let common = base_parts
        .iter()
        .zip(target_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut out = std::path::PathBuf::new();
    for _ in common..base_parts.len() {
        out.push("..");
    }
    for part in &target_parts[common..] {
        out.push(part);
    }
    let s = out.to_string_lossy().to_string();
    if s.is_empty() {
        ".".to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathdiff_goes_up_and_down() {
        let base = Path::new("/a/b/c");
        let target = Path::new("/a/b/problems/x");
        assert_eq!(pathdiff(base, target), "../problems/x");
    }

    #[test]
    fn pathdiff_identical_is_dot() {
        assert_eq!(pathdiff(Path::new("/a/b"), Path::new("/a/b")), ".");
    }
}
