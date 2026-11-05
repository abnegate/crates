#[cfg(target_os = "macos")]
const SYSCTL_MEMORY_KEY: &str = "hw.memsize";
#[cfg(target_os = "linux")]
const MEMINFO_PATH: &str = "/proc/meminfo";
#[cfg(any(target_os = "linux", test))]
const MEMINFO_TOTAL_PREFIX: &str = "MemTotal:";
#[cfg(any(target_os = "linux", test))]
const BYTES_PER_KIBIBYTE: u64 = 1024;

/// The system memory of this machine, in bytes, or `None` when it cannot be
/// read.
///
/// On macOS it is the physical size from `sysctl hw.memsize`, whatever the
/// CPU. On Linux it is `MemTotal` in `/proc/meminfo`: usable RAM, which is
/// physical RAM less reserved memory and the kernel image. Every other
/// platform is `None`. It runs `/usr/sbin/sysctl`, never one found on
/// `PATH`, or reads `/proc`, so it blocks the calling thread.
pub fn system_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        crate::hardware::sysctl::read(SYSCTL_MEMORY_KEY)?
            .parse()
            .ok()
    }
    #[cfg(target_os = "linux")]
    {
        total_from_meminfo(&std::fs::read_to_string(MEMINFO_PATH).ok()?)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// The `MemTotal` line of `/proc/meminfo`, which the kernel reports in
/// kibibytes despite its `kB` unit, in bytes.
#[cfg(any(target_os = "linux", test))]
fn total_from_meminfo(meminfo: &str) -> Option<u64> {
    let kibibytes: u64 = meminfo
        .lines()
        .find_map(|line| line.strip_prefix(MEMINFO_TOTAL_PREFIX))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kibibytes.saturating_mul(BYTES_PER_KIBIBYTE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a test run again in a child process finds set, naming the test.
    #[cfg(target_os = "macos")]
    const CHILD: &str = "ABNEGATE_LLM_TEST_CHILD";

    /// What a `sysctl` planted on `PATH` reports.
    #[cfg(target_os = "macos")]
    const PLANTED: u64 = 4096;

    const MEMINFO: &str = "MemTotal:       65777208 kB\n\
                           MemFree:         1234567 kB\n\
                           MemAvailable:   40000000 kB\n";

    #[test]
    fn meminfo_total_is_read_in_bytes() {
        assert_eq!(total_from_meminfo(MEMINFO), Some(65_777_208 * 1024));
    }

    #[test]
    fn meminfo_total_is_found_wherever_it_appears() {
        assert_eq!(
            total_from_meminfo("MemFree: 1 kB\nMemTotal: 2048 kB\n"),
            Some(2048 * 1024)
        );
    }

    #[test]
    fn meminfo_without_a_total_is_unknown() {
        assert_eq!(total_from_meminfo("MemFree: 1234567 kB\n"), None);
        assert_eq!(total_from_meminfo(""), None);
    }

    #[test]
    fn a_malformed_meminfo_total_is_unknown() {
        assert_eq!(total_from_meminfo("MemTotal: lots kB\n"), None);
        assert_eq!(total_from_meminfo("MemTotal: -1 kB\n"), None);
        assert_eq!(total_from_meminfo("MemTotal:\n"), None);
    }

    #[test]
    fn a_meminfo_total_too_large_for_bytes_saturates() {
        assert_eq!(
            total_from_meminfo(&format!("MemTotal: {} kB\n", u64::MAX)),
            Some(u64::MAX)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn this_machine_reports_its_memory() {
        assert!(system_memory_bytes().is_some_and(|bytes| bytes > 0));
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn an_unsupported_platform_reports_no_memory() {
        assert_eq!(system_memory_bytes(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_sysctl_on_the_path_is_never_run() {
        use std::os::unix::fs::PermissionsExt;
        const NAME: &str = "hardware::system_memory::tests::a_sysctl_on_the_path_is_never_run";
        if std::env::var(CHILD).as_deref() == Ok(NAME) {
            let bytes = system_memory_bytes();
            assert!(bytes.is_some_and(|bytes| bytes != PLANTED), "{bytes:?}");
            return;
        }
        let directory = tempfile::TempDir::new().unwrap();
        let planted = directory.path().join("sysctl");
        let marker = directory.path().join("ran");
        std::fs::write(
            &planted,
            format!(
                "#!/bin/sh\necho ran > '{}'\necho {PLANTED}\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o755)).unwrap();

        let output = std::process::Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, NAME)
            .env("PATH", directory.path())
            .output()
            .expect("the child runs");
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!marker.exists(), "the sysctl on PATH ran");
    }
}
