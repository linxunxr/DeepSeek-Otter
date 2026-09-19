// 控制中心页面逻辑：导航切换 + 概览与更新（复用 updater.ts）+ 诊断导出。
// 壳本地页面之一，与加载页同源，享有 Tauri IPC 权限（见 capabilities）。

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { initUpdaterUI, doCheck } from "./updater";

// 版本徽标：顶栏 + 概览页当前版本。
getVersion()
  .then((v) => {
    const badge = document.getElementById("ver-badge");
    const cur = document.getElementById("cur-ver");
    if (badge) badge.textContent = `v${v}`;
    if (cur) cur.textContent = v;
  })
  .catch(() => {});

// 左侧导航切换内容区。
const nav = document.getElementById("nav");
if (nav) {
  nav.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>("button[data-page]");
    if (!btn) return;
    const page = btn.dataset.page ?? "";
    nav.querySelectorAll("button").forEach((b) => b.classList.toggle("active", b === btn));
    document
      .querySelectorAll("main section")
      .forEach((s) => s.classList.toggle("active", s.id === `page-${page}`));
  });
}

// 更新面板与加载页共用 updater.ts；initUpdaterUI 自带启动静默检查。
initUpdaterUI();
document.getElementById("check-now")?.addEventListener("click", () => void doCheck(false));

// 诊断导出（IPC 命令在 lib.rs）。
document.getElementById("export-diag")?.addEventListener("click", async () => {
  const result = document.getElementById("diag-result");
  if (!result) return;
  result.textContent = "正在导出…";
  try {
    const path = await invoke<string>("export_diagnostics");
    result.textContent = `已导出：${path}`;
  } catch (e) {
    result.textContent = `导出失败：${e}`;
  }
});
