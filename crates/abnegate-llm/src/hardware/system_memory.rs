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
/// platform is `None`. It runs `sysctl` or reads `/proc`, so it blocks the
/// calling thread.
pub fn system_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sysctl")
            .args(["-n", SYSCTL_MEMORY_KEY])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
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
}
