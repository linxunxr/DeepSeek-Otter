// 获取自管运行时：把 dsh 的完整依赖树打进安装包（离线策略，与壳强绑定）。
//
// 产物布局（全部由 tauri.conf.json resources 打包）：
//   src-tauri/binaries/node-x86_64-pc-windows-msvc.exe   ← sidecar node（externalBin）
//   src-tauri/resources/runtime/npm.tgz                  ← npm CLI 包
//   src-tauri/resources/runtime/dsh-store/               ← dsh@pin 完整依赖树的本地 registry
//                                                            （package.json + package-lock.json + node_modules/.pnpm 布局的 tarball 集合）
//
// 用法：node scripts/fetch-runtime.mjs [--force]
// 默认走 npmmirror 镜像；MIRROR/NPM_REGISTRY 可覆盖。
// 升级 dsh：改 upstream.json 的 dshVersion → 跑本脚本 → 提交 lock 与 store 清单 → 重发 Otter 版本。

import { createWriteStream } from "node:fs";
import { mkdir, rm, stat, writeFile, cp, readFile } from "node:fs/promises";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import path from "node:path";
import { Readable } from "node:stream";

const require = createRequire(import.meta.url);

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const upstream = JSON.parse(await readFile(path.join(root, "upstream.json"), "utf8"));
const { nodeVersion, dshVersion } = upstream;

const MIRROR = process.env.MIRROR || "https://cdn.npmmirror.com/binaries/node";
const NODE_URL = `${MIRROR}/v${nodeVersion}/win-x64/node.exe`;
// npm 版本号动态解析：registry 的 tarball 地址必须带具体版本（无版本号返回 422）。
const NPM_REGISTRY = process.env.NPM_REGISTRY || "https://registry.npmmirror.com";

const binDir = path.join(root, "src-tauri", "binaries");
const resDir = path.join(root, "src-tauri", "resources", "runtime");
const storeDir = path.join(resDir, "dsh-store");

// Tauri externalBin 命名：<name>-<target-triple>.exe
const NODE_DEST = path.join(binDir, `node-x86_64-pc-windows-msvc.exe`);
const NPM_DEST = path.join(resDir, "npm.tgz");

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

// ---------------------------------------------------------------------------
// dsh 完整依赖树（离线 store）
// ---------------------------------------------------------------------------
// 思路：在一个临时目录里 `npm install @deepseek-ai/dsh@pin`（在线），得到
// package.json + package-lock.json + node_modules。运行时离线安装直接复用
// package-lock.json 并把 node_modules 整树打进安装包，首装零网络。
//
// 产物：
//   dsh-store/package.json        最小 manifest（dependencies 只含 dsh@pin）
//   dsh-store/package-lock.json   完整锁定的依赖树
//   dsh-store/node_modules/       已解析好的整树（首装直接拷贝或 npm ci --offline）
const STORE_PKG = path.join(storeDir, "package.json");
const STORE_LOCK = path.join(storeDir, "package-lock.json");
const STORE_NM = path.join(storeDir, "node_modules");

async function buildDshStore() {
  console.log(`构建 dsh@${dshVersion} 离线 store…`);
  await rm(storeDir, { recursive: true, force: true });
  await mkdir(storeDir, { recursive: true });

  // npm CLI 从已下载的 npm.tgz 解包获得（与运行时同源，不依赖系统 npm）。
  const npmExtract = path.join(resDir, "npm-cli-pkg");
  if (!(await exists(path.join(npmExtract, "package", "bin", "npm-cli.js")))) {
    await rm(npmExtract, { recursive: true, force: true });
    await mkdir(npmExtract, { recursive: true });
    // GNU tar 会把 "D:/..." 当远程主机（盘符冒号歧义）；--force-local 禁用该解释。
    const tgz = NPM_DEST.replaceAll("\\", "/");
    const dest = npmExtract.replaceAll("\\", "/");
    execFileSync("tar", ["--force-local", "-xzf", tgz, "-C", dest], { stdio: "inherit" });
  }
  const npmCli = path.join(npmExtract, "package", "bin", "npm-cli.js");

  const manifest = {
    name: "otter-dsh-store",
    private: true,
    dependencies: { "@deepseek-ai/dsh": dshVersion },
  };
  await writeFile(
    STORE_PKG,
    JSON.stringify(manifest, null, 2),
    "utf8"
  );

  // 在线 install：生成 lock 并解析整树。用 --no-audit/--no-fund 减少噪音与网络往返。
  console.log("在线解析依赖树（npm install，仅构建期联网）…");
  execFileSync(
    process.execPath,
    [
      npmCli,
      "install",
      "--no-audit",
      "--no-fund",
      "--loglevel=error",
      `--registry=${NPM_REGISTRY}`,
    ],
    { cwd: storeDir, stdio: "inherit" }
  );

  const lock = JSON.parse(await readFile(STORE_LOCK, "utf8"));
  const pkgs = Object.keys(lock.packages || {}).length - 1; // 减去根 ""
  console.log(`依赖树解析完成：${pkgs} 个包`);
  console.log(`dsh 精确版本：${lock.packages?.["node_modules/@deepseek-ai/dsh"]?.version ?? "?"}`);
  // .package-lock.json 是 npm 的隐藏缓存，打包无意义且会过期。
  await rm(path.join(storeDir, ".package-lock.json"), { force: true });
}

const dshChanged = force || !(await exists(STORE_LOCK));
if (dshChanged) {
  await buildDshStore();
} else {
  const lock = JSON.parse(await readFile(STORE_LOCK, "utf8"));
  const installed = lock.packages?.["node_modules/@deepseek-ai/dsh"]?.version;
  if (installed !== dshVersion) {
    console.log(`store 里是 dsh@${installed}，upstream.json pin 是 ${dshVersion}，重建…`);
    await buildDshStore();
  } else {
    console.log(`dsh store 已是 pin 版本（${dshVersion}），跳过`);
  }
}

// 把 store 整树打成单个 tar.gz：安装包体积从 268MB 降到 ~48MB（压缩），
// 且运行时首装是"解包一个归档"而非"拷贝几万小文件"——CI/慢盘上后者会超时。
const STORE_TAR = path.join(resDir, "dsh-store.tar.gz");
console.log("打包 dsh-store.tar.gz…");
execFileSync("tar", ["--force-local", "-czf", STORE_TAR.replaceAll("\\", "/"), "-C", storeDir.replaceAll("\\", "/"), "node_modules", "package.json", "package-lock.json"], { stdio: "inherit" });
const { size: tarSize } = await stat(STORE_TAR);
console.log(`  dsh-store.tar.gz（${(tarSize / 1024 / 1024).toFixed(1)} MB）`);

// 把 upstream.json 复制到 --no-bundle 产物目录能找到的位置（src-tauri/），
// 供运行时 pinned_version_candidates 解析；NSIS 形态由 tauri.conf.json resources 打包。
const upstreamSrc = path.join(root, "upstream.json");
const upstreamDst = path.join(root, "src-tauri", "upstream.json");
await writeFile(upstreamDst, await readFile(upstreamSrc, "utf8"), "utf8");
console.log(`upstream.json → ${upstreamDst}`);
console.log("完成。dsh 已全量打进安装包，首装离线可用。");
