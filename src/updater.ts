// 壳页面的更新检查与安装 UI：启动时自动静默检查（有新版才提示）+
// 托盘"检查更新"手动触发。仅自动到"提示"为止，下载/安装由用户决定；
// 安装前停掉 dsh 后端释放 node.exe 文件锁（Windows install 阶段应用会被退出）。

import { check } from "@tauri-apps/plugin-updater";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface UpdatePanel {
  visible: boolean;
  status: "idle" | "checking" | "available" | "not-available" | "downloading" | "error";
  version: string | null;
  notes: string | null;
  date: string | null;
  currentVersion: string;
  progress: number;
  error: string | null;
}

const panel: UpdatePanel = {
  visible: false,
  status: "idle",
  version: null,
  notes: null,
  date: null,
  currentVersion: "",
  progress: 0,
  error: null,
};

const panelEl = document.getElementById("update-panel") as HTMLDivElement;
const statusTextEl = document.getElementById("update-status-text") as HTMLSpanElement;
const metaEl = document.getElementById("update-meta") as HTMLDivElement;
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

// CHANGELOG 语法子集渲染：h3 / ul-li / 段落、**加粗**、`行内码`。
// 内容来自自家 CHANGELOG（CI 提取，可信），且先整体 HTML 转义再做标记替换，无注入面。
function renderNotes(md: string): string {
  const esc = md.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const inline = (s: string): string =>
    s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>").replace(/`([^`]+)`/g, "<code>$1</code>");
  const out: string[] = [];
  let inList = false;
  const closeList = (): void => {
    if (inList) {
      out.push("</ul>");
      inList = false;
    }
  };
  for (const line of esc.split("\n")) {
    const l = line.trim();
    if (!l) {
      closeList();
      continue;
    }
    if (l.startsWith("### ")) {
      closeList();
      out.push(`<h3>${inline(l.slice(4))}</h3>`);
    } else if (l.startsWith("- ")) {
      if (!inList) {
        out.push("<ul>");
        inList = true;
      }
      out.push(`<li>${inline(l.slice(2))}</li>`);
    } else {
      closeList();
      out.push(`<p>${inline(l)}</p>`);
    }
  }
  closeList();
  return out.join("");
}

function renderPanel(): void {
  panelEl.style.display = panel.visible ? "flex" : "none";
  statusTextEl.textContent = STATUS_TEXT[panel.status];
  const showDetails = panel.notes && panel.status === "available";
  metaEl.style.display = showDetails ? "block" : "none";
  notesEl.style.display = showDetails ? "block" : "none";
  if (showDetails) {
    const date = panel.date ? panel.date.slice(0, 10) : "";
    metaEl.innerHTML = `v${panel.currentVersion} → <strong>v${panel.version}</strong>${date ? `（${date}）` : ""}`;
    notesEl.innerHTML = renderNotes(panel.notes ?? "");
  }
  progressBarEl.style.display = panel.status === "downloading" ? "block" : "none";
  progressBarEl.style.width = `${panel.progress}%`;
  installBtn.style.display = panel.status === "available" ? "inline-block" : "none";
  installBtn.disabled = panel.status === "downloading";
  if (panel.status === "error") {
    statusTextEl.textContent = `更新失败：${panel.error ?? "未知错误"}`;
  }
}

// silent=true 供启动时的自动检查：无新版/失败不亮面板不打扰，
// 仅发现新版才提示（下载安装仍由用户决定）；手动检查（托盘入口/控制中心按钮）全程显示状态。
export async function doCheck(silent = false): Promise<void> {
  if (!silent) {
    panel.visible = true;
    panel.status = "checking";
    renderPanel();
  }
  panel.error = null;
  panel.notes = null;
  if (!panel.currentVersion) {
    panel.currentVersion = await getVersion().catch(() => "");
  }
  try {
    const update = await check();
    if (update) {
      panel.visible = true;
      panel.status = "available";
      panel.version = update.version;
      panel.notes = update.body ?? null;
      panel.date = update.date ?? null;
    } else if (!silent) {
      panel.status = "not-available";
    }
  } catch (e) {
    if (silent) return;
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
    await update.download((event) => {
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
    // 安装前停 dsh 后端：NSIS 写入被运行中 node.exe 锁定的安装目录文件会报
    // "Error opening file for writing"（v0.1.5 实证）；下载完成才停，下载期间 dsh 会话不受影响。
    await invoke("stop_backend");
    // Windows：install 启动安装器后自动退出应用，安装器装完默认自动重启（无需 relaunch）。
    await update.install();
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
  // 启动时自动检查一次（静默）：有新版才亮提示面板。
  void doCheck(true);
}
