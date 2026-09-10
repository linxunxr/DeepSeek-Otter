// 壳全局状态：dsh 后端生命周期状态机 + 壳页面 URL + 落盘日志。

use crate::backend::Backend;
use crate::logging::FileLog;
use std::sync::Mutex;
use tauri::Url;

pub struct OtterState {
    pub backend: Backend,
    pub log: FileLog,
    shell_url: Mutex<Option<Url>>,
}

impl OtterState {
    pub fn new(app_data_dir: std::path::PathBuf) -> Self {
        Self {
            backend: Backend::new(),
            log: FileLog::new(app_data_dir.join("logs")),
            shell_url: Mutex::new(None),
        }
    }

    /// 启动时记录壳页面初始 URL（dev 与打包形态不同，见 lib.rs setup）。
    pub fn set_shell_url(&self, url: Url) {
        *self.shell_url.lock().unwrap() = Some(url);
    }

    pub fn shell_url(&self) -> Option<Url> {
        self.shell_url.lock().unwrap().clone()
    }
}
