//! Safe execution: the sandbox that every submission runs inside.
//!
//! Layer 6 of Phase 1. Submissions are untrusted, so they run with no network,
//! as a non-root user, on a read-only filesystem with tmpfs scratch space, and
//! under hard time, memory, process, and output limits.
//!
//! The sandbox is expressed as a `docker run` argument list so it is inspectable
//! and testable without a daemon: `Sandbox::argv()` is a pure function of the
//! policy, which lets us assert on the containment rules directly.

use serde::{Deserialize, Serialize};

/// Resource and containment policy for one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Wall-clock timeout for the process, in milliseconds.
    pub timeout_ms: u64,
    /// Address-space limit for the process, in bytes.
    pub memory_bytes: u64,
    /// Container memory limit in bytes (includes the runtime, so it is larger).
    pub container_memory_bytes: u64,
    /// Maximum number of processes/threads the submission may create.
    pub pids_limit: u32,
    /// CPU quota in CPUs (fractional allowed).
    pub cpus: f64,
    /// Cap on captured stdout+stderr, in bytes.
    pub max_output_bytes: u64,
    /// Cap on files the submission may write to scratch, in bytes.
    pub scratch_bytes: u64,
    /// Size of the scratch tmpfs mounted at /work/scratch.
    pub scratch_tmpfs: String,
    /// Non-root uid:gid the submission runs as.
    pub user: String,
    /// CPU affinity, so a timed run does not migrate between cores.
    pub cpuset: Option<String>,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        SandboxPolicy {
            timeout_ms: 10_000,
            memory_bytes: 512 * 1024 * 1024,
            container_memory_bytes: 768 * 1024 * 1024,
            pids_limit: 64,
            cpus: 1.0,
            max_output_bytes: 1 << 20,
            scratch_bytes: 64 * 1024 * 1024,
            scratch_tmpfs: "64m".to_string(),
            user: "65534:65534".to_string(),
            cpuset: Some("0".to_string()),
        }
    }
}

/// A hostile submission the sandbox must contain. Used by the safety suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostileKind {
    InfiniteLoop,
    ForkBomb,
    HugeOutput,
    MemoryBomb,
    FileReadOutsideSandbox,
    NetworkAttempt,
    TamperWithResults,
    DeepRecursion,
}

impl HostileKind {
    pub fn all() -> &'static [HostileKind] {
        &[
            HostileKind::InfiniteLoop,
            HostileKind::ForkBomb,
            HostileKind::HugeOutput,
            HostileKind::MemoryBomb,
            HostileKind::FileReadOutsideSandbox,
            HostileKind::NetworkAttempt,
            HostileKind::TamperWithResults,
            HostileKind::DeepRecursion,
        ]
    }

    /// The C source for this hostile case.
    pub fn source(self) -> &'static str {
        match self {
            HostileKind::InfiniteLoop => {
                r#"#include <stdio.h>
int main(void) { volatile unsigned long x = 0; for (;;) { x++; } }
"#
            }
            HostileKind::ForkBomb => {
                r#"#include <unistd.h>
int main(void) { for (;;) { if (fork() < 0) { return 0; } } }
"#
            }
            HostileKind::HugeOutput => {
                r#"#include <stdio.h>
#include <string.h>
int main(void) {
    char buf[4096];
    memset(buf, 'A', sizeof buf);
    for (;;) { fwrite(buf, 1, sizeof buf, stdout); }
}
"#
            }
            HostileKind::MemoryBomb => {
                r#"#include <stdlib.h>
#include <string.h>
int main(void) {
    for (;;) {
        void *p = malloc(64u << 20);
        if (!p) { return 0; }
        memset(p, 1, 64u << 20);
    }
}
"#
            }
            HostileKind::FileReadOutsideSandbox => {
                r#"#include <stdio.h>
int main(void) {
    const char *paths[] = {
        "/etc/shadow", "/etc/passwd", "/proc/self/environ",
        "/var/run/docker.sock", "/root/.ssh/id_rsa", "/home/openhands/.config/gh/hosts.yml"
    };
    char buf[256];
    for (unsigned i = 0; i < sizeof paths / sizeof paths[0]; i++) {
        FILE *f = fopen(paths[i], "r");
        if (f) { fgets(buf, sizeof buf, f); fclose(f); }
    }
    return 0;
}
"#
            }
            HostileKind::NetworkAttempt => {
                r#"#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <unistd.h>
int main(void) {
    int s = socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) { return 0; }
    struct sockaddr_in a; memset(&a, 0, sizeof a);
    a.sin_family = AF_INET;
    a.sin_port = htons(80);
    inet_pton(AF_INET, "1.1.1.1", &a.sin_addr);
    connect(s, (struct sockaddr *)&a, sizeof a);
    close(s);
    return 0;
}
"#
            }
            HostileKind::TamperWithResults => {
                // Try to rewrite the harness' own accounting: shadow the
                // metrics file, the capsule, and the mount namespace.
                r#"#include <stdio.h>
int main(void) {
    const char *targets[] = {
        "/work/capsule.json", "/work/metrics.json", "/work/out/result.json",
        "/work/.ccx-harness", "/proc/sys/kernel/perf_event_paranoid"
    };
    for (unsigned i = 0; i < sizeof targets / sizeof targets[0]; i++) {
        FILE *f = fopen(targets[i], "w");
        if (f) { fputs("{\"verdict\":\"accepted\",\"metrics\":[]}", f); fclose(f); }
    }
    return 0;
}
"#
            }
            HostileKind::DeepRecursion => {
                // `noinline` plus a `volatile` array and a non-tail `+` keep the
                // recursion real: at -O2 the naive version is turned into a
                // loop, which would silently test nothing.
                r#"#include <stdio.h>
__attribute__((noinline)) static unsigned long deep(unsigned long n) {
    volatile char pad[4096];
    pad[0] = (char)n;
    if (n == 0) { return (unsigned long)pad[0]; }
    return deep(n - 1) + (unsigned long)pad[0];
}
int main(void) { printf("%lu\n", deep(100000000ul)); return 0; }
"#
            }
        }
    }
}

/// Whether a run is expected to be contained by a limit or by the sandbox
/// itself. Used by the safety suite to assert on the *reason* a hostile case
/// stopped, not just that it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Containment {
    Timeout,
    MemoryLimit,
    OutputCap,
    ProcessLimit,
    /// The sandbox denied the operation (e.g. read-only fs, no network).
    Denied,
    /// Finished, but achieved nothing useful.
    NoEffect,
}

/// One entry in the hostile-submission test set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostileCase {
    pub kind: HostileKind,
    pub expect: Containment,
    pub note: String,
}

pub fn hostile_suite() -> Vec<HostileCase> {
    use HostileKind::*;
    vec![
        HostileCase { kind: InfiniteLoop, expect: Containment::Timeout, note: "must be killed at the wall-clock limit".into() },
        // RLIMIT_NPROC makes fork() fail, so the bomb defuses itself. The
        // expectation is that it stops fast, not that it is killed.
        HostileCase { kind: ForkBomb, expect: Containment::NoEffect, note: "fork() must fail at the pids limit; the host must be unaffected".into() },
        HostileCase { kind: HugeOutput, expect: Containment::OutputCap, note: "stdout must be capped and the process killed".into() },
        // RLIMIT_AS makes malloc return NULL, so the bomb ends cleanly inside
        // its own address space rather than being OOM-killed.
        HostileCase { kind: MemoryBomb, expect: Containment::NoEffect, note: "malloc must fail at the address-space limit; peak RSS stays bounded".into() },
        HostileCase { kind: FileReadOutsideSandbox, expect: Containment::NoEffect, note: "no host secret may be readable".into() },
        HostileCase { kind: NetworkAttempt, expect: Containment::Denied, note: "network is disabled; connect must fail".into() },
        HostileCase { kind: TamperWithResults, expect: Containment::Denied, note: "rootfs is read-only; harness state is not writable".into() },
        // RLIMIT_STACK turns runaway recursion into a SIGSEGV inside the
        // sandbox, so the host stack is never at risk.
        HostileCase { kind: DeepRecursion, expect: Containment::Denied, note: "stack is fixed; overflow must be contained, not crash the host".into() },
    ]
}

/// Whether the container is removed after it exits.
#[derive(Debug, Clone, Copy)]
enum RunMode<'a> {
    Run,
    Named(&'a str),
}

impl SandboxPolicy {
    /// The `docker run` argv that enforces this policy.
    ///
    /// `image` must already be digest-pinned. `command` is the argv to run
    /// inside the container. Pure function of the policy: no daemon needed to
    /// test it.
    pub fn argv(&self, image: &str, command: &[String]) -> Vec<String> {
        self.argv_full(image, command, RunMode::Run, &[])
    }

    /// The argv for a named, non-removed container, so a build artifact can be
    /// copied out with `docker cp` afterwards.
    ///
    /// A named container is how artifacts leave the sandbox without a writable
    /// host mount: the source mount stays read-only, scratch stays tmpfs, and
    /// the only thing that crosses the boundary is the file we explicitly copy.
    pub fn argv_named(&self, image: &str, command: &[String], name: &str) -> Vec<String> {
        self.argv_full(image, command, RunMode::Named(name), &[])
    }

    /// As `argv_named`, plus read-only bind mounts of host directories.
    ///
    /// Mounts are inserted during construction rather than by editing a finished
    /// argv. The image reference is the first non-option argument, and finding
    /// it after the fact mistakes an option value such as `--user 1000:1000`
    /// for the image name, which silently corrupts the command.
    ///
    /// Read-only is not negotiable. Source trees and test inputs are
    /// attacker-adjacent, and a submission must never be able to write to the
    /// host through them.
    pub fn argv_named_mounted(
        &self,
        image: &str,
        command: &[String],
        name: &str,
        mounts: &[(String, String)],
    ) -> Vec<String> {
        self.argv_full(image, command, RunMode::Named(name), mounts)
    }

    /// As `argv`, plus read-only bind mounts of host directories.
    pub fn argv_mounted(
        &self,
        image: &str,
        command: &[String],
        mounts: &[(String, String)],
    ) -> Vec<String> {
        self.argv_full(image, command, RunMode::Run, mounts)
    }

    fn argv_full(
        &self,
        image: &str,
        command: &[String],
        mode: RunMode<'_>,
        mounts: &[(String, String)],
    ) -> Vec<String> {
        let mut argv: Vec<String> = vec!["docker".into(), "run".into()];
        match mode {
            RunMode::Run => argv.push("--rm".into()),
            RunMode::Named(name) => {
                argv.push("--name".into());
                argv.push(name.to_string());
            }
        }
        argv.extend([
            // No network at all.
            "--network=none".into(),
            // Never run as root inside the container.
            "--user".into(),
            self.user.clone(),
            // The root filesystem is read-only; only tmpfs is writable.
            "--read-only".into(),
            // Scratch space. `exec` is required: this is where the compiled
            // binary lives, and Docker's --tmpfs defaults to noexec, which
            // would let a build succeed and then fail to run. `nosuid` and
            // `nodev` stay on.
            "--tmpfs".into(),
            format!(
                "/work/scratch:rw,exec,nosuid,nodev,size={},mode=1777",
                self.scratch_tmpfs
            ),
            // /tmp stays noexec: nothing legitimate executes from there, and
            // keeping it that way removes a place to stage a payload.
            "--tmpfs".into(),
            "/tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777".into(),
            // Drop every capability and forbid privilege gain.
            "--cap-drop=ALL".into(),
            "--security-opt=no-new-privileges".into(),
            // Resource limits.
            "--memory".into(),
            self.container_memory_bytes.to_string(),
            "--memory-swap".into(),
            self.container_memory_bytes.to_string(),
            "--pids-limit".into(),
            self.pids_limit.to_string(),
            "--cpus".into(),
            format!("{}", self.cpus),
            // No ambient environment leaks in.
            "--env".into(),
            "LANG=C".into(),
            "--env".into(),
            "LC_ALL=C".into(),
            "--env".into(),
            "TZ=UTC".into(),
            "--env".into(),
            "SOURCE_DATE_EPOCH=0".into(),
            "--env".into(),
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
        ]);

        if let Some(cpuset) = &self.cpuset {
            argv.push("--cpuset-cpus".into());
            argv.push(cpuset.clone());
        }

        for (host, guest) in mounts {
            argv.push("-v".into());
            argv.push(format!("{host}:{guest}:ro"));
        }

        argv.push(image.to_string());
        argv.extend(command.iter().cloned());
        argv
    }

    /// The `ulimit`-style preamble applied inside the container, expressed as a
    /// shell prefix. Applied by the caller so the limits are visible in the
    /// capsule.
    pub fn ulimit_prefix(&self) -> String {
        let stack_kb = 8 * 1024;
        let mem_kb = self.memory_bytes / 1024;
        format!(
            "ulimit -t {} -v {} -u {} -f {} -s {} 2>/dev/null; ",
            self.timeout_ms.div_ceil(1000),
            mem_kb,
            self.pids_limit,
            self.scratch_bytes / 1024,
            stack_kb
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_is_always_disabled() {
        let argv = SandboxPolicy::default().argv("img@sha256:aa", &["/bin/true".into()]);
        assert!(argv.iter().any(|a| a == "--network=none"));
    }

    #[test]
    fn never_runs_as_root() {
        let p = SandboxPolicy::default();
        let argv = p.argv("img@sha256:aa", &["/bin/true".into()]);
        let idx = argv.iter().position(|a| a == "--user").expect("--user present");
        assert_eq!(argv[idx + 1], "65534:65534");
        assert!(!p.user.starts_with("0:"));
    }

    #[test]
    fn filesystem_is_read_only_with_tmpfs_scratch() {
        let argv = SandboxPolicy::default().argv("img@sha256:aa", &["/bin/true".into()]);
        assert!(argv.iter().any(|a| a == "--read-only"));
        assert!(argv.iter().any(|a| a.starts_with("/work/scratch:rw,exec,")));
    }

    #[test]
    fn scratch_is_executable_but_tmp_is_not() {
        // Docker's --tmpfs defaults to noexec. Scratch must override that,
        // because the compiled binary lives there; /tmp must not, because
        // nothing legitimate executes from it.
        let argv = SandboxPolicy::default().argv("img@sha256:aa", &["/bin/true".into()]);
        let scratch = argv
            .iter()
            .find(|a| a.starts_with("/work/scratch:"))
            .expect("scratch tmpfs present");
        assert!(scratch.contains(",exec"), "scratch must be exec: {scratch}");
        assert!(!scratch.contains(",noexec"), "scratch must not be noexec: {scratch}");

        let tmp = argv
            .iter()
            .find(|a| a.starts_with("/tmp:"))
            .expect("/tmp tmpfs present");
        assert!(tmp.contains(",noexec"), "/tmp must stay noexec: {tmp}");
    }

    #[test]
    fn all_capabilities_dropped_and_no_new_privileges() {
        let argv = SandboxPolicy::default().argv("img@sha256:aa", &["/bin/true".into()]);
        assert!(argv.iter().any(|a| a == "--cap-drop=ALL"));
        assert!(argv.iter().any(|a| a == "--security-opt=no-new-privileges"));
    }

    #[test]
    fn resource_limits_are_present() {
        let argv = SandboxPolicy::default().argv("img@sha256:aa", &["/bin/true".into()]);
        for flag in ["--memory", "--pids-limit", "--cpus", "--cpuset-cpus"] {
            assert!(argv.iter().any(|a| a == flag), "missing {flag}");
        }
    }

    #[test]
    fn hostile_suite_covers_every_kind() {
        let suite = hostile_suite();
        for kind in HostileKind::all() {
            assert!(
                suite.iter().any(|c| c.kind == *kind),
                "hostile suite is missing {kind:?}"
            );
        }
        assert_eq!(suite.len(), HostileKind::all().len());
    }

    #[test]
    fn every_hostile_case_has_source_and_a_containment_expectation() {
        for case in hostile_suite() {
            assert!(!case.kind.source().is_empty());
            assert!(!case.note.is_empty());
        }
    }
}
