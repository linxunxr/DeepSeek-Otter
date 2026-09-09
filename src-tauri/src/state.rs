// 壳全局状态：dsh 后端生命周期状态机 + 壳页面 URL。

use crate::backend::Backend;
use std::sync::Mutex;
use tauri::Url;

pub struct OtterState {
    pub backend: Backend,
    shell_url: Mutex<Option<Url>>,
}

impl OtterState {
    pub fn new() -> Self {
        Self {
            backend: Backend::new(),
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
