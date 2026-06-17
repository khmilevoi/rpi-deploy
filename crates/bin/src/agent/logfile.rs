use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const LOG_PREFIX: &str = "pi-agent.log.";

#[derive(Clone)]
pub struct DailyMakeWriter {
    dir: PathBuf,
}

impl DailyMakeWriter {
    pub fn new(dir: PathBuf) -> DailyMakeWriter {
        DailyMakeWriter { dir }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DailyMakeWriter {
    type Writer = DailyWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DailyWriter {
            dir: self.dir.clone(),
        }
    }
}

pub struct DailyWriter {
    dir: PathBuf,
}

impl Write for DailyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self
            .dir
            .join(format!("{}{}", LOG_PREFIX, utc_date(SystemTime::now())));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let prefix = format!("{} ", utc_timestamp(SystemTime::now()));
        file.write_all(prefix.as_bytes())?;
        file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn log_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                if name.to_string_lossy().starts_with(LOG_PREFIX) {
                    files.push(entry.path());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    files.sort();
    Ok(files)
}

pub fn prune_old(dir: &Path, retention_days: u64) -> std::io::Result<()> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(retention_days.saturating_mul(86_400)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for file in log_files(dir)? {
        let meta = std::fs::metadata(&file)?;
        if meta.modified().unwrap_or(SystemTime::now()) < cutoff {
            let _ = std::fs::remove_file(file);
        }
    }
    Ok(())
}

pub fn read(
    dir: &Path,
    tail: Option<usize>,
    since_unix: Option<i64>,
) -> std::io::Result<Vec<String>> {
    let mut lines = VecDeque::new();
    let tail = tail.unwrap_or(usize::MAX);
    for file in log_files(dir)? {
        let text = std::fs::read_to_string(file)?;
        for line in text.lines() {
            if let Some(cutoff) = since_unix {
                if !line_is_since(line, cutoff) {
                    continue;
                }
            }
            if tail != usize::MAX && lines.len() == tail {
                lines.pop_front();
            }
            lines.push_back(line.to_string());
        }
    }
    Ok(lines.into_iter().collect())
}

pub async fn follow(
    dir: PathBuf,
    since_unix: Option<i64>,
    mut send: impl FnMut(String) -> bool,
) -> std::io::Result<()> {
    let mut emitted = read(&dir, None, since_unix)?.len();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let all = read(&dir, None, since_unix)?;
        for line in all.iter().skip(emitted) {
            if !send(line.clone()) {
                return Ok(());
            }
        }
        emitted = all.len();
    }
}

fn line_is_since(line: &str, cutoff: i64) -> bool {
    let Some(prefix) = line.split_whitespace().next() else {
        return false;
    };
    parse_rfc3339_utc(prefix)
        .map(|ts| ts >= cutoff)
        .unwrap_or(true)
}

fn parse_rfc3339_utc(s: &str) -> Option<i64> {
    if s.len() < 20 || &s[4..5] != "-" || &s[7..8] != "-" || &s[10..11] != "T" {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let minute: i64 = s[14..16].parse().ok()?;
    let second: i64 = s[17..19].parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

fn utc_date(now: SystemTime) -> String {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

fn utc_timestamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + (m <= 2) as i64;
    (year as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_tail_keeps_last_lines_across_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pi-agent.log.2026-01-01"), "a\nb\n").unwrap();
        std::fs::write(dir.path().join("pi-agent.log.2026-01-02"), "c\n").unwrap();
        assert_eq!(read(dir.path(), Some(2), None).unwrap(), vec!["b", "c"]);
    }

    #[test]
    fn since_filters_rfc3339_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pi-agent.log.2026-01-01"),
            "2026-01-01T00:00:00Z old\n2026-01-01T01:00:00Z new\n",
        )
        .unwrap();
        assert_eq!(
            read(dir.path(), None, Some(1_767_226_400)).unwrap(),
            vec!["2026-01-01T01:00:00Z new"]
        );
    }
}
