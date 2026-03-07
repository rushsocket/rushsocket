/// Returns the current memory usage as a percentage of total system memory.
/// Linux-only; returns None on other platforms or if /proc is unavailable.
pub fn memory_usage_percent() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let rss_kb = parse_proc_field("/proc/self/status", "VmRSS:")?;
        let total_kb = parse_proc_field("/proc/meminfo", "MemTotal:")?;
        if total_kb == 0 {
            return None;
        }
        Some((rss_kb as f64 / total_kb as f64) * 100.0)
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn parse_proc_field(path: &str, field: &str) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        if line.starts_with(field) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return parts[1].parse().ok();
            }
        }
    }
    None
}
