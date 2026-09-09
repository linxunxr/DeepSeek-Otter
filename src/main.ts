// 壳本地页面的全部逻辑：展示 dsh 后端状态，就绪后把窗口导航到回环 Web UI。
// 只通过 Tauri IPC 与壳通信，不直接访问文件系统或后端进程。

import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

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
  actionsEl.dataset.visible = status.state === "failed" ? "true" : "false";
  logEl.textContent = status.recentLog.join("\n");
  logEl.scrollTop = logEl.scrollHeight;
}

document.getElementById("retry")?.addEventListener("click", () => {
  void invoke("restart_backend");
});

async function main(): Promise<void> {
  render(await invoke<BackendStatus>("get_backend_status"));
  await listen<BackendStatus>("backend-status", (event) => render(event.payload));
}

void main();
