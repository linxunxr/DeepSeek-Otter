// 落盘日志：appData/logs/otter.log，按天滚动，保留最近 7 份。
// 壳与 dsh 后端的关键事件（状态迁移、子进程输出、安装进度）都进这里，
// 崩溃/异常时用户可导出诊断包（见 lib.rs 的 export_diagnostics）。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const KEEP_LOGS: usize = 7;

pub struct FileLog {
    dir: PathBuf,
    inner: Mutex<LogInner>,
}

struct LogInner {
    current_day: String,
    file: Option<File>,
}

impl FileLog {
    pub fn new(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);
        Self {
            dir,
            inner: Mutex::new(LogInner {
                current_day: String::new(),
                file: None,
            }),
        }
    }

    /// 写一行（自动带时间戳）。滚动失败静默降级（不能因日志拖垮主流程）。
    pub fn log(&self, msg: &str) {
        let today = today();
        let mut guard = self.inner.lock().unwrap();
        // 跨天滚动。
        if today != guard.current_day || guard.file.is_none() {
            if let Err(e) = self.rotate_locked(&today, &mut guard) {
                eprintln!("日志滚动失败：{e}");
                return;
            }
        }
        if let Some(f) = guard.file.as_mut() {
            let ts = now_hms();
            let _ = writeln!(f, "{ts} {msg}");
            let _ = f.flush();
        }
    }

    /// 当前日志文件路径（诊断导出用）。
    pub fn current_path(&self) -> PathBuf {
        self.dir.join(format!("otter-{}.log", today()))
    }

    fn rotate_locked(&self, day: &str, guard: &mut LogInner) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        self.prune_old();
        let path = self.dir.join(format!("otter-{day}.log"));
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        guard.current_day = day.to_string();
        guard.file = Some(file);
        Ok(())
    }

    /// 只保留最近 KEEP_LOGS 天的日志文件。
    fn prune_old(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut logs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "log")
                    && p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("otter-"))
            })
            .collect();
        if logs.len() >= KEEP_LOGS {
            logs.sort();
            let excess = logs.len() - (KEEP_LOGS - 1);
            for old in logs.into_iter().take(excess) {
                let _ = fs::remove_file(old);
            }
        }
    }
}

fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    civil_from_days(secs / 86400)
}

fn now_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day_secs = secs % 86400;
    format!(
        "{:02}:{:02}:{:02}",
        day_secs / 3600,
        (day_secs % 3600) / 60,
        day_secs % 60
    )
}

/// 天数 → YYYY-MM-DD（Howard Hinnant 的 civil_from_days 算法，无 chrono 依赖）。
fn civil_from_days(z: u64) -> String {
    let z = z as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_epoch_and_known_dates() {
        assert_eq!(civil_from_days(0), "1970-01-01");
        assert_eq!(civil_from_days(19_723), "2024-01-01");
        assert_eq!(civil_from_days(20_651), "2026-07-17");
        assert_eq!(civil_from_days(20_652), "2026-07-18");
        // 闰日：2024-02-29 → 03-01 相邻两天（天数经 Node Date 独立核对）。
        assert_eq!(civil_from_days(19_782), "2024-02-29");
        assert_eq!(civil_from_days(19_783), "2024-03-01");
    }

    #[test]
    fn log_write_and_rollover_names() {
        let dir = std::env::temp_dir().join(format!("otter-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = FileLog::new(dir.clone());
        log.log("hello 世界");
        let path = log.current_path();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("hello 世界"), "内容：{content}");
        assert!(
            path.file_name().unwrap().to_string_lossy().starts_with("otter-20"),
            "文件名：{}",
            path.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
