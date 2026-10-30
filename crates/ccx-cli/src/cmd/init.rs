//! `ccx init` — scaffold a solution folder.
//!
//! A solution folder is the unit of work: one `ccx.toml` naming the problem, one
//! source file, and whatever else the language needs. The scaffold is written
//! so that `ccx test` works immediately.

use crate::cmd::{self, Fixable};
use crate::context::{
    default_build_command, default_cmake_target, validate_build_command,
    validate_build_customization, validate_cmake_target, validate_run_command, BuildPreset,
    BuildSystem, CmakeGenerator, SolutionConfig, SOLUTION_MARKER,
};
use crate::Format;
use ccx_core::capsule::{Language, ToolchainName};
use ccx_core::toolchain;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    pub dir: String,
    pub problem: String,
    pub language: String,
    pub toolchain: String,
    pub target_os: String,
    pub target_arch: String,
    pub build_system: String,
    pub cmake_generator: String,
    pub cmake_target: String,
    pub build_preset: String,
    pub compiler_version: String,
    pub compiler_flags: Vec<String>,
    pub program_args: Vec<String>,
    pub build_command: Vec<String>,
    pub run_command: Vec<String>,
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
    toolchain: &str,
    target_os: &str,
    target_arch: &str,
    build_system: &str,
    cmake_generator: &str,
    cmake_target: &str,
    build_preset: &str,
    compiler_version: Option<&str>,
    compiler_flags: &[String],
    program_args: &[String],
    build_command: &[String],
    run_command: &[String],
    dir: &Path,
) -> anyhow::Result<()> {
    if !matches!(
        language,
        "c" | "cpp" | "c++" | "rust" | "d" | "zig" | "odin" | "go"
    ) {
        return Err(Fixable::new(format!("unsupported language {language:?}"))
            .fix("currently supported build languages are `c`, `cpp`, `rust`, `d`, `zig`, `odin`, and `go`")
            .into_error());
    }
    if target_os != "linux" || target_arch != "x86_64" {
        return Err(Fixable::new(format!(
            "target {target_os}/{target_arch} has no pinned execution profile"
        ))
        .fix("the available end-to-end target is linux/x86_64")
        .fix("run `ccx profiles --format json` for known target profiles and availability")
        .into_error());
    }
    if !matches!(build_system, "direct" | "cmake" | "custom") {
        return Err(
            Fixable::new(format!("unsupported build system {build_system:?}"))
                .fix("choose `direct`, `cmake`, or `custom`")
                .into_error(),
        );
    }
    if !matches!(cmake_generator, "ninja" | "make") {
        return Err(
            Fixable::new(format!("unsupported CMake generator {cmake_generator:?}"))
                .fix("choose `ninja` or `make`")
                .into_error(),
        );
    }
    if build_system == "cmake" {
        validate_cmake_target(cmake_target).map_err(|message| {
            Fixable::new(message)
                .fix("choose the executable target name from your CMakeLists.txt")
                .into_error()
        })?;
    } else if cmake_target != default_cmake_target() {
        return Err(
            Fixable::new("a CMake target was selected for a non-CMake build")
                .fix("use `--build-system cmake` or omit `--cmake-target`")
                .into_error(),
        );
    }
    if build_system == "direct" && cmake_generator != "ninja" {
        return Err(
            Fixable::new("a CMake generator was selected for a direct build")
                .fix("use `--build-system cmake` or keep `--cmake-generator ninja`")
                .into_error(),
        );
    }
    let toolchain_name = if toolchain == "auto" {
        match language {
            "rust" => "rust",
            "d" => "gdc",
            "zig" => "zig",
            "odin" => "odin",
            "go" => "go",
            _ => "gcc",
        }
    } else {
        toolchain
    };
    if !matches!(
        toolchain_name,
        "gcc" | "gcc-13" | "clang" | "clang-18" | "rust" | "gdc" | "zig" | "odin" | "go"
    ) || (language == "rust" && toolchain_name != "rust")
        || (language != "rust" && toolchain_name == "rust")
        || (language == "d" && toolchain_name != "gdc")
        || (language != "d" && toolchain_name == "gdc")
        || (language == "zig" && toolchain_name != "zig")
        || (language != "zig" && toolchain_name == "zig")
        || (language == "odin" && toolchain_name != "odin")
        || (language != "odin" && toolchain_name == "odin")
        || (language == "go" && toolchain_name != "go")
        || (language != "go" && toolchain_name == "go")
    {
        return Err(Fixable::new(format!("unsupported toolchain {toolchain:?}"))
            .fix("C and C++ use a pinned GCC or Clang profile; Rust uses `rust`; D uses `gdc`; Zig uses `zig`; Odin uses `odin`; Go uses `go`")
            .into_error());
    }
    if build_system == "cmake" && !matches!(language, "c" | "cpp" | "c++") {
        return Err(
            Fixable::new("CMake scaffolds are currently available for C and C++")
                .fix("use `--build-system direct` for Rust, D, Zig, Odin, or Go")
                .into_error(),
        );
    }

    let language_value = match language {
        "c" => Language::C,
        "cpp" | "c++" => Language::Cpp,
        "rust" => Language::Rust,
        "d" => Language::D,
        "zig" => Language::Zig,
        "odin" => Language::Odin,
        "go" => Language::Go,
        _ => unreachable!(),
    };
    let preset_value = match build_preset {
        "standard" => BuildPreset::Standard,
        "custom" => BuildPreset::Custom,
        _ => {
            return Err(
                Fixable::new(format!("unsupported build preset {build_preset:?}"))
                    .fix("choose `standard` or `custom`")
                    .into_error(),
            );
        }
    };
    let build_system_value = match build_system {
        "direct" => BuildSystem::Direct,
        "cmake" => BuildSystem::Cmake,
        "custom" => BuildSystem::Custom,
        _ => unreachable!(),
    };
    let cmake_generator_value = match cmake_generator {
        "ninja" => CmakeGenerator::Ninja,
        "make" => CmakeGenerator::UnixMakefiles,
        _ => unreachable!(),
    };
    validate_build_customization(
        language_value,
        preset_value,
        build_system_value,
        compiler_flags,
    )
    .map_err(|message| {
        Fixable::new(message)
            .fix("use the standard preset or a bounded, allow-listed C/C++ flag list")
            .into_error()
    })?;
    let build_command_value =
        if build_system_value == BuildSystem::Custom && build_command.is_empty() {
            default_build_command()
        } else {
            build_command.to_vec()
        };
    validate_build_command(language_value, build_system_value, &build_command_value).map_err(
        |message| {
            Fixable::new(message)
                .fix("custom commands use the pinned C/C++ compiler and the fixed output/source placeholders")
                .into_error()
        },
    )?;
    crate::context::validate_program_args(program_args).map_err(|message| {
        Fixable::new(message)
            .fix("pass repeated `--program-arg` values for the fixed executable")
            .into_error()
    })?;
    validate_run_command(run_command).map_err(|message| {
        Fixable::new(message)
            .fix("use a direct argv list with {binary} and {args}; tokens are passed without shell parsing")
            .into_error()
    })?;

    let selected_toolchain = match toolchain_name {
        "gcc" => ToolchainName::Gcc,
        "gcc-13" => ToolchainName::Gcc13,
        "clang" => ToolchainName::Clang,
        "clang-18" => ToolchainName::Clang18,
        "rust" => ToolchainName::Rust,
        "gdc" => ToolchainName::Gdc,
        "zig" => ToolchainName::Zig,
        "odin" => ToolchainName::Odin,
        "go" => ToolchainName::Go,
        _ => unreachable!(),
    };
    let pinned_toolchain = toolchain::pinned(selected_toolchain).ok_or_else(|| {
        Fixable::new(format!("no pinned {toolchain_name} toolchain is available"))
            .fix("choose a compiler profile listed by `ccx doctor`")
            .into_error()
    })?;
    if let Some(version) = compiler_version {
        if version != pinned_toolchain.version {
            return Err(Fixable::new(format!(
                "compiler version {version:?} is not pinned for {toolchain_name:?}"
            ))
            .fix(format!(
                "the available pinned version is {:?}",
                pinned_toolchain.version
            ))
            .into_error());
        }
    }

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
        "rust" => (
            "rs",
            r#"use std::io::{self, Read};

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
}
"#,
        ),
        "d" => (
            "d",
            r#"import std.stdio;

int main() {
    return 0;
}
"#,
        ),
        "zig" => (
            "zig",
            r#"extern "c" fn read(fd: c_int, buffer: [*]u8, count: usize) isize;
extern "c" fn write(fd: c_int, buffer: [*]const u8, count: usize) isize;
extern "c" fn qsort(base: [*]u8, count: usize, size: usize, compare: *const fn (?*const anyopaque, ?*const anyopaque) callconv(.c) c_int) void;

var input: [16 * 1024 * 1024]u8 = undefined;
var values: [200000]i64 = undefined;

fn lessThan(left: ?*const anyopaque, right: ?*const anyopaque) callconv(.c) c_int {
    const a: *const i64 = @ptrCast(@alignCast(left.?));
    const b: *const i64 = @ptrCast(@alignCast(right.?));
    return if (a.* < b.*) -1 else if (a.* > b.*) 1 else 0;
}

fn nextInteger(bytes: []const u8, cursor: *usize) ?i64 {
    while (cursor.* < bytes.len and bytes[cursor.*] <= ' ') : (cursor.* += 1) {}
    if (cursor.* == bytes.len) return null;
    var sign: i64 = 1;
    if (bytes[cursor.*] == '-') {
        sign = -1;
        cursor.* += 1;
    }
    var value: i64 = 0;
    while (cursor.* < bytes.len and bytes[cursor.*] >= '0' and bytes[cursor.*] <= '9') : (cursor.* += 1) {
        value = value * 10 + bytes[cursor.*] - '0';
    }
    return value * sign;
}

pub fn main() void {
    const size = read(0, &input, input.len);
    if (size <= 0) return;
    const bytes = input[0..@intCast(size)];
    var cursor: usize = 0;
    const count: usize = @intCast(nextInteger(bytes, &cursor) orelse return);
    const threshold = nextInteger(bytes, &cursor) orelse return;
    if (count > values.len) return;
    for (0..count) |index| values[index] = nextInteger(bytes, &cursor) orelse return;
    qsort(@ptrCast(&values), count, @sizeOf(i64), lessThan);
    var left: usize = 0;
    var right = count;
    var pairs: i64 = 0;
    while (left < right) {
        if (values[left] + values[right - 1] <= threshold) {
            pairs += @intCast(right - left - 1);
            left += 1;
        } else {
            right -= 1;
        }
    }
    var output: [32]u8 = undefined;
    var length: usize = 0;
    var number: u64 = @intCast(pairs);
    if (number == 0) {
        output[0] = '0';
        length = 1;
    } else {
        while (number > 0) : (number /= 10) {
            output[length] = @intCast('0' + number % 10);
            length += 1;
        }
        var index: usize = 0;
        while (index < length / 2) : (index += 1) {
            const temporary = output[index];
            output[index] = output[length - index - 1];
            output[length - index - 1] = temporary;
        }
    }
    output[length] = '\n';
    _ = write(1, &output, length + 1);
}
"#,
        ),
        "odin" => (
            "odin",
            r#"package main

import "core:fmt"
import "core:os"
import "core:sort"

values: [200000]i64

parse_int :: proc(data: []u8, cursor: ^int) -> (value: i64, ok: bool) {
    for cursor^ < len(data) && data[cursor^] <= ' ' {
        cursor^ += 1
    }
    if cursor^ >= len(data) {
        return 0, false
    }
    sign: i64 = 1
    if data[cursor^] == '-' {
        sign = -1
        cursor^ += 1
    }
    for cursor^ < len(data) && data[cursor^] >= '0' && data[cursor^] <= '9' {
        value = value*10 + i64(data[cursor^]-'0')
        cursor^ += 1
    }
    return value*sign, true
}

main :: proc() {
    data, err := os.read_entire_file_from_file(os.stdin, context.temp_allocator)
    if err != nil {
        return
    }
    cursor := 0
    count64, ok := parse_int(data, &cursor)
    if !ok || count64 < 0 || count64 > 200000 {
        return
    }
    threshold, threshold_ok := parse_int(data, &cursor)
    if !threshold_ok {
        return
    }
    count := int(count64)
    for i in 0..<count {
        values[i], ok = parse_int(data, &cursor)
        if !ok {
            return
        }
    }
    sort.quick_sort(values[:count])
    left, right := 0, count
    pairs: i64 = 0
    for left < right {
        if values[left]+values[right-1] <= threshold {
            pairs += i64(right-left-1)
            left += 1
        } else {
            right -= 1
        }
    }
    fmt.println(pairs)
}
"#,
        ),
        "go" => (
            "go",
            r#"package main

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"sort"
)

func nextInt(data []byte, cursor *int) (int64, bool) {
	for *cursor < len(data) && data[*cursor] <= ' ' {
		*cursor++
	}
	if *cursor >= len(data) {
		return 0, false
	}
	var sign int64 = 1
	if data[*cursor] == '-' {
		sign = -1
		*cursor++
	}
	var value int64
	for *cursor < len(data) && data[*cursor] >= '0' && data[*cursor] <= '9' {
		value = value*10 + int64(data[*cursor]-'0')
		*cursor++
	}
	return value * sign, true
}

func main() {
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		return
	}
	cursor := 0
	n64, ok := nextInt(data, &cursor)
	if !ok || n64 < 0 || n64 > 200000 {
		return
	}
	threshold, ok := nextInt(data, &cursor)
	if !ok {
		return
	}
	n := int(n64)
	values := make([]int64, n)
	for i := range values {
		values[i], ok = nextInt(data, &cursor)
		if !ok {
			return
		}
	}
	sort.Slice(values, func(i, j int) bool { return values[i] < values[j] })
	left, right := 0, n
	var pairs int64
	for left < right {
		if values[left]+values[right-1] <= threshold {
			pairs += int64(right - left - 1)
			left++
		} else {
			right--
		}
	}
	writer := bufio.NewWriter(os.Stdout)
	fmt.Fprintln(writer, pairs)
	writer.Flush()
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
        "cpp" | "c++" => (
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
        _ => unreachable!("language was validated above"),
    };

    let mut written = Vec::new();

    let config = format!(
        "# CodeCurrency solution.\n{}",
        toml::to_string(&SolutionConfig {
            problem: rel,
            language: language.to_string(),
            toolchain: toolchain_name.to_string(),
            opt_level: "-O2".to_string(),
            build_preset: preset_value,
            compiler_version: Some(
                compiler_version
                    .unwrap_or(pinned_toolchain.version.as_str())
                    .to_string(),
            ),
            compiler_flags: compiler_flags.to_vec(),
            program_args: program_args.to_vec(),
            build_command: build_command_value.clone(),
            run_command: run_command.to_vec(),
            target_os: target_os.to_string(),
            target_arch: target_arch.to_string(),
            build_system: build_system_value,
            cmake_generator: cmake_generator_value,
            cmake_target: cmake_target.to_string(),
        })?
    );
    std::fs::write(root.join(SOLUTION_MARKER), config)?;
    written.push(SOLUTION_MARKER.to_string());

    let src_name = format!("main.{ext}");
    std::fs::write(root.join(&src_name), source)?;
    written.push(src_name.clone());
    if build_system == "cmake" {
        let cmake_lists = if language == "c" {
            format!("cmake_minimum_required(VERSION 3.31)\nproject(CodeCurrencySolution LANGUAGES C)\nset(CMAKE_C_STANDARD 17)\nset(CMAKE_C_STANDARD_REQUIRED ON)\nadd_executable({cmake_target} main.c)\n")
        } else {
            format!("cmake_minimum_required(VERSION 3.31)\nproject(CodeCurrencySolution LANGUAGES CXX)\nset(CMAKE_CXX_STANDARD 20)\nset(CMAKE_CXX_STANDARD_REQUIRED ON)\nadd_executable({cmake_target} main.cpp)\n")
        };
        std::fs::write(root.join("CMakeLists.txt"), cmake_lists)?;
        written.push("CMakeLists.txt".to_string());
    }

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
        toolchain: toolchain_name.to_string(),
        target_os: target_os.to_string(),
        target_arch: target_arch.to_string(),
        build_system: build_system.to_string(),
        cmake_generator: cmake_generator.to_string(),
        cmake_target: cmake_target.to_string(),
        build_preset: build_preset.to_string(),
        compiler_version: compiler_version
            .unwrap_or(pinned_toolchain.version.as_str())
            .to_string(),
        compiler_flags: compiler_flags.to_vec(),
        program_args: program_args.to_vec(),
        build_command: build_command_value,
        run_command: run_command.to_vec(),
        files_written: written,
        next_steps: vec![
            format!("edit {src_name} to solve the problem"),
            "ccx test".to_string(),
            "ccx measure --repeat 20".to_string(),
        ],
    };

    cmd::emit(
        format,
        || {
            println!("initialised a solution in {}", report.dir);
            println!("  problem  {}", report.problem);
            println!("  language {}", report.language);
            println!("  toolchain {}", report.toolchain);
            println!("  target   {}/{}", report.target_os, report.target_arch);
            println!("  compiler  {}", report.compiler_version);
            println!("  build system {}", report.build_system);
            if report.build_system == "cmake" {
                println!("  generator {}", report.cmake_generator);
                println!("  target    {}", report.cmake_target);
            }
            println!("  preset    {}", report.build_preset);
            if !report.compiler_flags.is_empty() {
                println!("  flags     {}", report.compiler_flags.join(" "));
            }
            if !report.program_args.is_empty() {
                println!("  arguments {}", report.program_args.join(" "));
            }
            if !report.build_command.is_empty() {
                println!("  build argv {}", report.build_command.join(" "));
            }
            if !report.run_command.is_empty() {
                println!("  run argv   {}", report.run_command.join(" "));
            }
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
    let mut s = out.to_string_lossy().to_string();
    if std::path::MAIN_SEPARATOR != '/' {
        s = s.replace(std::path::MAIN_SEPARATOR, "/");
    }
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
