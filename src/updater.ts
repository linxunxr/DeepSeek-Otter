// 壳页面的更新检查与安装 UI（托盘"检查更新"触发 / 手动按钮）。
// 安装前由 Rust 侧停掉 dsh 后端（Windows install 阶段应用会被退出）。

import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { listen } from "@tauri-apps/api/event";

interface UpdatePanel {
  visible: boolean;
  status: "idle" | "checking" | "available" | "not-available" | "downloading" | "error";
  version: string | null;
  notes: string | null;
  progress: number;
  error: string | null;
}

const panel: UpdatePanel = {
  visible: false,
  status: "idle",
  version: null,
  notes: null,
  progress: 0,
  error: null,
};

const panelEl = document.getElementById("update-panel") as HTMLDivElement;
const statusTextEl = document.getElementById("update-status-text") as HTMLSpanElement;
const notesEl = document.getElementById("update-notes") as HTMLDivElement;
const progressBarEl = document.getElementById("update-progress") as HTMLDivElement;
const installBtn = document.getElementById("update-install") as HTMLButtonElement;
const closeBtn = document.getElementById("update-close") as HTMLButtonElement;

const STATUS_TEXT: Record<UpdatePanel["status"], string> = {
  idle: "",
  checking: "正在检查更新…",
  available: "发现新版本",
  "not-available": "已是最新版本",
  downloading: "正在下载更新…",
  error: "更新失败",
};

function renderPanel(): void {
  panelEl.style.display = panel.visible ? "flex" : "none";
  statusTextEl.textContent = STATUS_TEXT[panel.status];
  notesEl.style.display = panel.notes && panel.status === "available" ? "block" : "none";
  notesEl.textContent = panel.notes ?? "";
  progressBarEl.style.display = panel.status === "downloading" ? "block" : "none";
  progressBarEl.style.width = `${panel.progress}%`;
  installBtn.style.display = panel.status === "available" ? "inline-block" : "none";
  installBtn.disabled = panel.status === "downloading";
  if (panel.status === "error") {
    statusTextEl.textContent = `更新失败：${panel.error ?? "未知错误"}`;
  }
}

async function doCheck(): Promise<void> {
  panel.visible = true;
  panel.status = "checking";
  panel.error = null;
  panel.notes = null;
  renderPanel();
  try {
    const update = await check();
    if (update) {
      panel.status = "available";
      panel.version = update.version;
      panel.notes = update.body ?? null;
    } else {
      panel.status = "not-available";
    }
  } catch (e) {
    panel.status = "error";
    panel.error = typeof e === "string" ? e : String(e);
  }
  renderPanel();
}

async function doInstall(): Promise<void> {
  panel.status = "downloading";
  panel.progress = 0;
  renderPanel();
  try {
    const update = await check();
    if (!update) {
      panel.status = "not-available";
      renderPanel();
      return;
    }
    let total = 0;
    let downloaded = 0;
    await update.downloadAndInstall((event) => {
      switch (event.event) {
        case "Started":
          total = event.data.contentLength ?? 0;
          break;
        case "Progress":
          downloaded += event.data.chunkLength;
          panel.progress = total > 0 ? Math.round((downloaded / total) * 100) : 0;
          renderPanel();
          break;
        case "Finished":
          panel.progress = 100;
          renderPanel();
          break;
      }
    });
    await relaunch();
  } catch (e) {
    panel.status = "error";
    panel.error = typeof e === "string" ? e : String(e);
    renderPanel();
  }
}

installBtn.addEventListener("click", () => void doInstall());
closeBtn.addEventListener("click", () => {
  panel.visible = false;
  renderPanel();
});

export function initUpdaterUI(): void {
  void listen("check-update", () => void doCheck());
}
