// 拼装 Tauri updater 双清单（单平台 windows-x86_64，比灵鉴多平台版大幅简化）。
//
// 产物（写在 dist-dir 下）：
//   latest.github.json  → url 指 GitHub Release 永久地址（上传为 Release assets 的
//                         latest.json，第二更新源；客户端经 releases/latest/download 拉取）
//   latest.gitee.json   → url 指 Gitee Release tag 固定直链（CI 提交到 Gitee 仓库根
//                         作为 latest.json，国内主更新源）
//   两清单除 platforms.*.url 外完全一致（版本/签名/notes 同源生成）。
//
// 用法: node scripts/generate-latest-json.mjs <dist-dir> <version>
// 前提: dist-dir 里有 <Product>_x64-setup.exe 与同名 .sig（createUpdaterArtifacts 产物）。

import { readFileSync, writeFileSync, readdirSync, existsSync } from "node:fs";
import { join } from "node:path";
import { exit } from "node:process";

const GITHUB_BASE =
  "https://github.com/linxunxr/DeepSeek-Otter/releases/latest/download";
const GITEE_BASE =
  "https://gitee.com/mwcxlinxun/deep-seek-otter/releases/download";

const distDir = process.argv[2];
const version = process.argv[3];

if (!distDir || !version) {
  console.error("用法: node scripts/generate-latest-json.mjs <dist-dir> <version>");
  exit(1);
}

const normalizedVersion = version.startsWith("v") ? version : `v${version}`;
const bareVersion = normalizedVersion.replace(/^v/, "");

if (!existsSync(distDir)) {
  console.error(`✗ 产物目录不存在：${distDir}`);
  exit(1);
}

// Otter 只有 windows-x86_64 一个平台：setup.exe + .sig 一对。
const files = readdirSync(distDir);
const sigFiles = files.filter((f) => f.endsWith(".sig"));
if (sigFiles.length === 0) {
  console.error("✗ 未找到 .sig 签名文件（createUpdaterArtifacts 未生效或产物缺失）");
  exit(1);
}

let assetName = null;
let signature = "";
for (const sigFile of sigFiles) {
  const main = sigFile.replace(/\.sig$/, "");
  if (!main.toLowerCase().endsWith(".exe")) continue;
  if (!existsSync(join(distDir, main))) continue;
  assetName = main;
  signature = readFileSync(join(distDir, sigFile), "utf-8").trim();
}
if (!assetName) {
  console.error("✗ 未找到配对的 setup.exe 产物");
  exit(1);
}

const manifest = (url) => ({
  version: bareVersion,
  notes: `${normalizedVersion} 更新`,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": {
      signature,
      url,
    },
  },
});

const githubUrl = `${GITHUB_BASE}/${encodeURIComponent(assetName)}`;
const giteeUrl = `${GITEE_BASE}/${normalizedVersion}/${encodeURIComponent(assetName)}`;

writeFileSync(
  join(distDir, "latest.github.json"),
  JSON.stringify(manifest(githubUrl), null, 2) + "\n",
  "utf-8",
);
writeFileSync(
  join(distDir, "latest.gitee.json"),
  JSON.stringify(manifest(giteeUrl), null, 2) + "\n",
  "utf-8",
);

console.log(`✓ latest.github.json（GitHub 第二源）`);
console.log(`✓ latest.gitee.json（Gitee 国内主源）`);
console.log(`  版本 ${bareVersion} ← ${assetName}`);
