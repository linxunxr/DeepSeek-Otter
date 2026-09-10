// 把更新清单提交到 Gitee 仓库根的 latest.json（Gitee 无 latest/download 固定地址，
// 仓库 raw 地址 /raw/main/latest.json 永久稳定且匿名可读，updater 跟随 302 重定向）。
// 幂等：文件已存在走 PUT（带上一版 sha）。
//
// 用法: node scripts/sync-latest-to-gitee.mjs <manifest-file>
//   先降级后升级：发布时先传 latest.github.json（保底），verify 通过后传 latest.gitee.json。

import { readFileSync } from "node:fs";
import { exit } from "node:process";

const GITEE_API = "https://gitee.com/api/v5";
const OWNER = process.env.GITEE_OWNER || "linxunxr";
const REPO = process.env.GITEE_REPO || "DeepSeek-Otter";
const token = process.env.GITEE_TOKEN;

const manifestFile = process.argv[2];
if (!manifestFile || !token) {
  console.error("用法: GITEE_TOKEN=… node scripts/sync-latest-to-gitee.mjs <manifest-file>");
  exit(1);
}

const content = readFileSync(manifestFile, "utf-8");
const headers = { authorization: `token ${token}` };
const timeout = { signal: AbortSignal.timeout(30_000) };

async function gitee(path, init = {}) {
  const resp = await fetch(`${GITEE_API}${path}`, {
    ...init,
    headers: { ...headers, ...(init.headers || {}) },
    ...timeout,
  });
  const text = await resp.text();
  if (!resp.ok) throw new Error(`Gitee API ${resp.status} ${path}: ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : null;
}

// 当前文件（取 sha 用于 PUT 更新）。
let sha;
try {
  const current = await gitee(
    `/repos/${OWNER}/${REPO}/contents/${encodeURIComponent("latest.json")}?ref=main`,
  );
  sha = current?.sha;
} catch {
  /* 文件不存在，走新建 */
}

const resp = await fetch(
  `${GITEE_API}/repos/${OWNER}/${REPO}/contents`,
  {
    method: "POST",
    headers: { ...headers, "Content-Type": "application/json" },
    body: JSON.stringify({
      access_token: token,
      content: Buffer.from(content, "utf-8").toString("base64"),
      message: `chore(updater): 更新 latest.json${sha ? "（PUT）" : ""}`,
      branch: "main",
      ...(sha ? { sha } : {}),
    }),
    ...timeout,
  },
);
if (!resp.ok) {
  // Gitee contents API：新建用 POST，更新用 PUT（与 GitHub 相同语义）。
  const putResp = await fetch(
    `${GITEE_API}/repos/${OWNER}/${REPO}/contents/${encodeURIComponent("latest.json")}`,
    {
      method: "PUT",
      headers: { ...headers, "Content-Type": "application/json" },
      body: JSON.stringify({
        access_token: token,
        content: Buffer.from(content, "utf-8").toString("base64"),
        message: "chore(updater): 更新 latest.json",
        branch: "main",
        ...(sha ? { sha } : {}),
      }),
      ...timeout,
    },
  );
  if (!putResp.ok) {
    console.error(`✗ 提交失败：POST ${resp.status} / PUT ${putResp.status}`);
    exit(1);
  }
}

console.log(`✓ latest.json 已提交到 Gitee（来源：${manifestFile}）`);
