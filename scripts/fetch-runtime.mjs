// 获取自管运行时：下载官方 node.exe 与 npm 包到 src-tauri/binaries/，
// 供 Tauri externalBin/resources 打包。dev 模式手动运行，打包前必须执行。
//
// 产物布局（runtime/ 前缀是 Tauri externalBin 的目标三元组约定，见 resources 里的 npm）：
//   src-tauri/binaries/node-x86_64-pc-windows-msvc.exe   ← sidecar node（externalBin）
//   src-tauri/resources/runtime/npm/                     ← npm CLI 包（resources）
//
// 用法：node scripts/fetch-runtime.mjs [--force]
// 网络不通时重试或设置 MIRROR 环境变量换镜像。

import { createWriteStream } from "node:fs";
import { mkdir, rm, stat, writeFile } from "node:fs/promises";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { Readable } from "node:stream";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const upstream = JSON.parse(await readFile(path.join(root, "upstream.json"), "utf8"));
const { nodeVersion, dshVersion } = upstream;

const MIRROR = process.env.MIRROR || "https://cdn.npmmirror.com/binaries/node";
const NODE_URL = `${MIRROR}/v${nodeVersion}/win-x64/node.exe`;
// npm 版本号动态解析：registry 的 tarball 地址必须带具体版本（无版本号返回 422）。
const NPM_REGISTRY = process.env.NPM_REGISTRY || "https://registry.npmmirror.com";

const binDir = path.join(root, "src-tauri", "binaries");
const resDir = path.join(root, "src-tauri", "resources", "runtime");

// Tauri externalBin 命名：<name>-<target-triple>.exe
const NODE_DEST = path.join(binDir, `node-x86_64-pc-windows-msvc.exe`);
const NPM_DEST = path.join(resDir, "npm.tgz");

function readFile(p) {
  return import("node:fs/promises").then((m) => m.readFile(p, "utf8"));
}

async function exists(p) {
  try {
    await stat(p);
    return true;
  } catch {
    return false;
  }
}

async function download(url, dest) {
  console.log(`下载 ${url}`);
  console.log(`  → ${dest}`);
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) throw new Error(`HTTP ${res.status} ${url}`);
  await pipeline(Readable.fromWeb(res.body), createWriteStream(dest));
  const { size } = await stat(dest);
  console.log(`  完成（${(size / 1024 / 1024).toFixed(1)} MB）`);
}

const force = process.argv.includes("--force");
await mkdir(binDir, { recursive: true });
await mkdir(resDir, { recursive: true });

if (force || !(await exists(NODE_DEST))) {
  await download(NODE_URL, NODE_DEST);
} else {
  console.log(`node 已存在：${NODE_DEST}（--force 重下）`);
}

if (force || !(await exists(NPM_DEST))) {
  // 从 registry 元数据解析 npm 最新版本再下载 tarball。
  const metaRes = await fetch(`${NPM_REGISTRY}/npm`);
  if (!metaRes.ok) throw new Error(`HTTP ${metaRes.status} 查询 npm 元数据`);
  const meta = await metaRes.json();
  const npmVer = meta["dist-tags"].latest;
  await download(`${NPM_REGISTRY}/npm/-/npm-${npmVer}.tgz`, NPM_DEST);
} else {
  console.log(`npm 已存在：${NPM_DEST}（--force 重下）`);
}

console.log(`upstream pin：node ${nodeVersion}，dsh ${dshVersion}`);
console.log("完成。dsh 首次启动时由应用在线安装（见 backend.rs install 流程）。");
