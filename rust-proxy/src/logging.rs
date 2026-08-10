use chrono::{DateTime, Utc};
use chrono_tz::{Tz, UTC};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

struct DailyFileAppender {
    directory: PathBuf,
    timezone: Tz,
    date: String,
    file: File,
}

impl DailyFileAppender {
    fn new(directory: impl AsRef<Path>, timezone: Tz) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        let date = local_date(timezone);
        let file = open_log_file(&directory, &date)?;
        Ok(Self { directory, timezone, date, file })
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let date = local_date(self.timezone);
        if date != self.date {
            self.file = open_log_file(&self.directory, &date)?;
            self.date = date;
        }
        Ok(())
    }
}

impl Write for DailyFileAppender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.rotate_if_needed()?;
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Clone)]
struct TimezoneTimer(Tz);

impl FormatTime for TimezoneTimer {
    fn format_time(&self, writer: &mut Writer<'_>) -> std::fmt::Result {
        write!(writer, "{}", local_now(self.0).format("%Y-%m-%dT%H:%M:%S%.6f%:z"))
    }
}

pub fn init() -> Result<WorkerGuard, Box<dyn std::error::Error + Send + Sync>> {
    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "/app/logs".to_string());
    std::fs::create_dir_all(&log_dir)?;
    let (timezone, invalid_timezone) = timezone_from_env();
    let file_appender = DailyFileAppender::new(&log_dir, timezone)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "rust_proxy=info".into());

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_timer(TimezoneTimer(timezone))
                .with_writer(non_blocking),
        )
        .init();

    if let Some(value) = invalid_timezone {
        tracing::warn!(timezone = value, "Invalid TZ, falling back to UTC");
    }
    Ok(guard)
}

fn timezone_from_env() -> (Tz, Option<String>) {
    match std::env::var("TZ") {
        Ok(value) => match value.parse() {
            Ok(timezone) => (timezone, None),
            Err(_) => (UTC, Some(value)),
        },
        Err(_) => (UTC, None),
    }
}

fn local_now(timezone: Tz) -> DateTime<Tz> {
    Utc::now().with_timezone(&timezone)
}

fn local_date(timezone: Tz) -> String {
    local_now(timezone).format("%Y-%m-%d").to_string()
}

fn open_log_file(directory: &Path, date: &str) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(format!("rust-proxy.log.{date}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn timezone_changes_date_and_timestamp_offset() {
        let utc = Utc.with_ymd_and_hms(2026, 8, 10, 18, 30, 0).unwrap();
        let shanghai = utc.with_timezone(&chrono_tz::Asia::Shanghai);

        assert_eq!(shanghai.format("%Y-%m-%d").to_string(), "2026-08-11");
        assert_eq!(
            shanghai.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
            "2026-08-11T02:30:00+08:00"
        );
    }

    #[test]
    fn bundled_timezone_database_contains_shanghai() {
        assert!(chrono_tz::TZ_VARIANTS.contains(&chrono_tz::Asia::Shanghai));
    }
}
