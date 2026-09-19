// 构建冒烟测试专用 exe：覆盖 identifier（.smoke 后缀），与正式实例的单实例锁、
// appData 全隔离——冒烟可在不动用户正在运行的 Otter 的前提下执行。
// 产物复制为 deepseek-otter-smoke.exe 保存（后续常规构建会覆盖同名产物）。
// 用法: pnpm smoke:build（前置同常规构建：fetch-runtime 已跑）

import { spawnSync } from "node:child_process";
import { copyFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const exe = path.join(root, "src-tauri", "target", "release", "deepseek-otter.exe");
const smokeExe = path.join(root, "src-tauri", "target", "release", "deepseek-otter-smoke.exe");

const result = spawnSync(
  "pnpm",
  ["tauri", "build", "--no-bundle", "--config", "src-tauri/tauri.smoke.conf.json"],
  { stdio: "inherit", cwd: root, shell: true }
);
if (result.status !== 0) {
  console.error("✗ smoke 构建失败");
  process.exit(result.status ?? 1);
}
copyFileSync(exe, smokeExe);
console.log(`✓ smoke exe：${smokeExe}`);
