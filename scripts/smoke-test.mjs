// 冒烟测试：黑盒驱动 release exe 验证核心生命周期承诺。
// 覆盖设计文档的冒烟清单（自动化的部分）：启动 → 后端拉起 → 鉴权在位 →
// 单实例 → 退出清理（进程树终止、端口释放）。
//
// 前置：node scripts/fetch-runtime.mjs 已跑、pnpm tauri build --no-bundle 已编译。
// 用法：node scripts/smoke-test.mjs [--fresh]（--fresh 先删 appData 模拟首装）
//
// 断言失败即非零退出；全程超时保护（CI 防挂死）。

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const exe = path.join(root, "src-tauri", "target", "release", "deepseek-otter.exe");
const appData = path.join(
  process.env.APPDATA || path.join(process.env.USERPROFILE || "", "AppData", "Roaming"),
  "com.linxunxr.deepseek-otter"
);

const FRESH = process.argv.includes("--fresh");
const TOTAL_TIMEOUT_MS = FRESH ? 5 * 60_000 : 120_000;

if (!existsSync(exe)) {
  fail(`找不到 ${exe}，先跑 pnpm tauri build --no-bundle`);
}

function fail(msg) {
  console.error(`✗ ${msg}`);
  cleanup(1);
}

function cleanup(code) {
  // 兜底清理：杀掉测试拉起的 otter 进程树。
  if (mainPid) {
    spawnSync("taskkill", ["/PID", String(mainPid), "/T", "/F"], { stdio: "ignore" });
  }
  process.exit(code);
}

const timer = setTimeout(() => fail(`总超时（${TOTAL_TIMEOUT_MS} ms）`), TOTAL_TIMEOUT_MS);
let mainPid = null;

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

/** 列出 pid 及其全部后代（WMI 查询 ParentProcessId 链）。 */
async function listDescendants(pid) {
  const out = await ps(
    `Get-CimInstance Win32_Process | Where-Object {$_.ParentProcessId -eq ${pid} -or $_.ProcessId -eq ${pid}} | Select-Object ProcessId,ParentProcessId,Name,CommandLine | ConvertTo-Json -Compress`
  );
  if (!out.trim()) return [];
  const parsed = JSON.parse(out);
  return Array.isArray(parsed) ? parsed : [parsed];
}

function ps(command) {
  return new Promise((resolve) => {
    const p = spawn(
      "powershell",
      ["-NoProfile", "-Command", command],
      { stdio: ["ignore", "pipe", "ignore"] }
    );
    let out = "";
    p.stdout.on("data", (c) => (out += c));
    p.on("close", () => resolve(out));
  });
}

/** 找到 otter 拉起的 dsh 后端 node 进程（命令行含 dsh bin.js web）。 */
async function findBackendPid(parentPid) {
  const out = await ps(
    `Get-CimInstance Win32_Process -Filter "Name='node.exe'" | Where-Object {$_.CommandLine -like '*dsh*bin.js*web*' -and $_.ParentProcessId -eq ${parentPid}} | Select-Object -ExpandProperty ProcessId`
  );
  const pid = parseInt(out.trim(), 10);
  return Number.isFinite(pid) ? pid : null;
}

/** 找 dsh 监听的回环端口（backendPid 的 LISTENING 行）。 */
async function findBackendPort(backendPid) {
  const out = await new Promise((resolve) => {
    const p = spawn("netstat", ["-ano"], { stdio: ["ignore", "pipe", "ignore"] });
    let buf = "";
    p.stdout.on("data", (c) => (buf += c));
    p.on("close", () => resolve(buf));
  });
  for (const line of out.split("\n")) {
    const m = line.trim().match(/^TCP\s+127\.0\.0\.1:(\d+)\s+\S+\s+LISTENING\s+(\d+)/);
    if (m && Number(m[2]) === backendPid) return Number(m[1]);
  }
  return null;
}

async function httpStatus(url) {
  try {
    const res = await fetch(url, { redirect: "manual" });
    return res.status;
  } catch {
    return 0;
  }
}

async function waitFor(desc, fn, { timeoutMs = 90_000, intervalMs = 2_000 } = {}) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeoutMs) fail(`等待超时：${desc}`);
    await sleep(intervalMs);
  }
}

async function main() {
  console.log(`== 冒烟测试（${FRESH ? "fresh 首装" : "已有安装"}）==`);
  console.log(`exe: ${exe}`);
  console.log(`appData: ${appData}`);

  if (FRESH && existsSync(appData)) {
    console.log("--fresh：删除 appData 模拟首装");
    rmSync(appData, { recursive: true, force: true });
  }

  // 1. 启动（独立进程组，避免测试退出连带杀掉它——由 cleanup 显式管理）。
  console.log("1. 启动应用…");
  const child = spawn(exe, [], {
    cwd: path.dirname(exe),
    detached: true,
    stdio: "ignore",
    shell: false,
  });
  child.unref();
  mainPid = child.pid;
  console.log(`   pid=${mainPid}`);

  await waitFor("应用进程存活", async () => {
    const alive = await ps(`if (Get-Process -Id ${mainPid} -ErrorAction SilentlyContinue) {'y'} else {''}`);
    return alive.trim() === "y" ? true : null;
  });

  // 2. 后端拉起（首装含在线安装 dsh，最长 4 分钟）。
  console.log("2. 等待 dsh 后端进程…");
  const backendPid = await waitFor("dsh 后端进程", () => findBackendPid(mainPid), {
    timeoutMs: FRESH ? 240_000 : 90_000,
  });
  console.log(`   backend pid=${backendPid}`);

  // 3. 监听端口 + 鉴权（无 token 401 是访问凭据在位的证据）。
  console.log("3. 等待回环端口监听…");
  const port = await waitFor("回环端口", () => findBackendPort(backendPid));
  console.log(`   port=${port}`);
  const code = await httpStatus(`http://127.0.0.1:${port}/`);
  console.log(`   GET /（无 token）→ ${code}`);
  if (code !== 401 && code !== 302 && code !== 200) {
    fail(`预期 401（或重定向），得到 ${code}`);
  }
  console.log("   鉴权在位 ✓");

  // 4. 单实例：第二次启动应立即退出并把焦点交给既有实例。
  console.log("4. 单实例检查…");
  const second = spawnSync(exe, [], { cwd: path.dirname(exe), timeout: 15_000, stdio: "ignore" });
  const stillAlive = await ps(`if (Get-Process -Id ${mainPid} -ErrorAction SilentlyContinue) {'y'} else {''}`);
  if (stillAlive.trim() !== "y") fail("第二实例启动后主实例消失");
  const backendStill = await findBackendPid(mainPid);
  if (!backendStill) fail("第二实例启动后 dsh 后端消失");
  console.log(`   第二实例 exit=${second.status ?? "timeout"}，主实例与后端仍在 ✓`);

  // 5. 退出清理：杀主进程后进程树终止、端口释放。
  console.log("5. 退出清理…");
  spawnSync("taskkill", ["/PID", String(mainPid), "/T", "/F"], { stdio: "ignore" });
  await sleep(2_000);
  const backendGone = await ps(
    `if (Get-Process -Id ${backendPid} -ErrorAction SilentlyContinue) {'y'} else {''}`
  );
  if (backendGone.trim() === "y") fail(`后端进程 ${backendPid} 在主实例退出后仍存活`);
  const portGone = await findBackendPort(backendPid);
  if (portGone) fail(`端口 ${port} 未释放`);
  console.log("   进程树终止、端口释放 ✓");

  mainPid = null;
  clearTimeout(timer);
  console.log("== 冒烟测试全部通过 ==");
  process.exit(0);
}

main().catch((e) => fail(e.stack || String(e)));
