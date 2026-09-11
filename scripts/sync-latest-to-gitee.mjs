// 把更新清单提交到 Gitee 仓库根的 latest.json（Gitee 无 latest/download 固定地址，
// 仓库 raw 地址 /raw/main/latest.json 永久稳定且匿名可读，updater 跟随 302 重定向）。
//
// Gitee contents API 语义（官方文档/实测）：
//   创建：POST /repos/{owner}/{repo}/contents/{path}   ← path 必须在 URL，缺它会 405
//   更新：PUT  /repos/{owner}/{repo}/contents/{path}   ← 必须带现有文件的 sha
//   content 一律 Base64。
//
// 用法: node scripts/sync-latest-to-gitee.mjs <manifest-file>
//   先降级后升级：发布时先传 latest.github.json（保底），verify 通过后传 latest.gitee.json。

import { readFileSync } from "node:fs";
import { exit } from "node:process";

const GITEE_API = "https://gitee.com/api/v5";
const OWNER = process.env.GITEE_OWNER || "mwcxlinxun";
const REPO = process.env.GITEE_REPO || "deep-seek-otter";
const token = process.env.GITEE_TOKEN;
const FILE_PATH = "latest.json";

const manifestFile = process.argv[2];
if (!manifestFile || !token) {
  console.error("用法: GITEE_TOKEN=… node scripts/sync-latest-to-gitee.mjs <manifest-file>");
  exit(1);
}

const content = Buffer.from(readFileSync(manifestFile, "utf-8"), "utf-8").toString("base64");
const url = `${GITEE_API}/repos/${OWNER}/${REPO}/contents/${encodeURIComponent(FILE_PATH)}`;

async function gitee(method, body) {
  const resp = await fetch(url, {
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ access_token: token, branch: "main", ...body }),
    signal: AbortSignal.timeout(30_000),
  });
  const text = await resp.text();
  if (!resp.ok) {
    throw new Error(`Gitee API ${resp.status} ${method}: ${text.slice(0, 200)}`);
  }
  return text ? JSON.parse(text) : null;
}

// 现有文件的 sha（决定创建 vs 更新）。
let sha;
try {
  const current = await gitee("GET", {});
  sha = current?.sha;
} catch {
  /* 404 = 文件不存在，走创建 */
}

try {
  if (sha) {
    await gitee("PUT", { content, message: "chore(updater): 更新 latest.json", sha });
    console.log("✓ latest.json 已更新（PUT）");
  } else {
    await gitee("POST", { content, message: "chore(updater): 新增 latest.json" });
    console.log("✓ latest.json 已创建（POST）");
  }
} catch (e) {
  console.error(`✗ ${e.message}`);
  exit(1);
}
