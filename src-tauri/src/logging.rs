// 落盘日志：appData/logs/otter.log，按天滚动，保留最近 7 份。
// 壳与 dsh 后端的关键事件（状态迁移、子进程输出、安装进度）都进这里，
// 崩溃/异常时用户可导出诊断包（见 lib.rs 的 export_diagnostics）。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

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

// 时间戳用本地时间（曾用 UTC epoch 手算，比墙钟慢 8 小时，排障时严重误导：
// "10:48 出错"实际是 18:48）。chrono 已是 tauri 的传递依赖，直接复用。
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
