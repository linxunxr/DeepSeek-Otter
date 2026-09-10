// 壳本地页面的全部逻辑：展示 dsh 后端状态，就绪后把窗口导航到回环 Web UI。
// 只通过 Tauri IPC 与壳通信，不直接访问文件系统或后端进程。

import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { initUpdaterUI } from "./updater";

type BackendState = "stopped" | "installing" | "starting" | "running" | "failed";

interface BackendStatus {
  state: BackendState;
  port: number | null;
  message: string | null;
  recentLog: string[];
}

const statusEl = document.getElementById("status") as HTMLDivElement;
const statusTextEl = document.getElementById("status-text") as HTMLSpanElement;
const actionsEl = document.getElementById("actions") as HTMLDivElement;
const logEl = document.getElementById("log") as HTMLPreElement;

const STATE_TEXT: Record<BackendState, string> = {
  stopped: "后端已停止",
  installing: "首次启动：正在安装 dsh 运行时…",
  starting: "正在启动 dsh 后端…",
  running: "后端已就绪，正在打开 Web UI…",
  failed: "后端启动失败",
};

function render(status: BackendStatus): void {
  statusEl.dataset.state = status.state;
  statusTextEl.textContent = status.message ?? STATE_TEXT[status.state];
  const failed = status.state === "failed";
  actionsEl.dataset.visible = failed ? "true" : "false";
  if (!failed) {
    const diag = document.getElementById("export-diag");
    if (diag instanceof HTMLButtonElement) {
      diag.textContent = "导出诊断包";
    }
  }
  logEl.textContent = status.recentLog.join("\n");
  logEl.scrollTop = logEl.scrollHeight;
}

document.getElementById("retry")?.addEventListener("click", () => {
  void invoke("restart_backend");
});

const diagBtn = document.getElementById("export-diag");
diagBtn?.addEventListener("click", async () => {
  if (diagBtn instanceof HTMLButtonElement) {
    diagBtn.disabled = true;
    diagBtn.textContent = "导出中…";
    try {
      const path = await invoke<string>("export_diagnostics");
      diagBtn.textContent = `已导出：${path}`;
    } catch (e) {
      diagBtn.textContent = `导出失败：${e}`;
    } finally {
      setTimeout(() => {
        diagBtn.disabled = false;
        diagBtn.textContent = "导出诊断包";
      }, 4000);
    }
  }
});

async function main(): Promise<void> {
  render(await invoke<BackendStatus>("get_backend_status"));
  await listen<BackendStatus>("backend-status", (event) => render(event.payload));
  initUpdaterUI();
}

void main();
