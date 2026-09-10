// 同步 GitHub Release 到 Gitee 发行版（国内镜像）。从灵鉴生产脚本裁剪：
// 单平台两个产物（setup.exe + .sig），远低于 Gitee 20 附件上限。
//
// 做三件事：创建发行版 → 上传附件（attach_files）→ 幂等可重跑。
// 鉴权：GITEE_TOKEN（私人令牌，projects 权限）。
// 灵鉴实证的坑全部保留：>1MB 走 curl（Node fetch 大 multipart 断连）、
// API 30s 超时（fetch 无超时会挂死）、单文件失败重试 3 次不阻断。
//
// GitHub 是权威源，本脚本任何失败只告警，不阻断发布。
//
// 用法: node scripts/sync-release-to-gitee.mjs <dist-dir> <version>
// 环境变量：GITEE_TOKEN 必填；GITEE_OWNER/GITEE_REPO 可覆盖。

import { readdirSync, existsSync, statSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { exit } from "node:process";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

const GITEE_API = "https://gitee.com/api/v5";
const OWNER = process.env.GITEE_OWNER || "mwcxlinxun";
const REPO = process.env.GITEE_REPO || "deep-seek-otter";
const UPLOAD_RETRY = 3;
const API_TIMEOUT_MS = 30_000;

const token = process.env.GITEE_TOKEN;
const distDir = process.argv[2];
const version = process.argv[3]?.replace(/^v/, "");

if (!distDir || !version) {
  console.error("用法: node scripts/sync-release-to-gitee.mjs <dist-dir> <version>");
  exit(1);
}
if (!token) {
  console.warn("⚠ 未设置 GITEE_TOKEN，跳过 Gitee 同步");
  exit(0);
}

const headers = { authorization: `token ${token}` };

async function gitee(path, init = {}) {
  const resp = await fetch(`${GITEE_API}${path}`, {
    ...init,
    headers: { ...headers, ...(init.headers || {}) },
    signal: AbortSignal.timeout(API_TIMEOUT_MS),
  });
  const text = await resp.text();
  let body;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = text;
  }
  if (!resp.ok) {
    const msg = typeof body === "object" ? JSON.stringify(body) : String(body).slice(0, 200);
    throw new Error(`Gitee API ${resp.status} ${path}: ${msg}`);
  }
  return body;
}

async function findOrCreateRelease() {
  const tag = `v${version}`;
  try {
    const releases = await gitee(`/repos/${OWNER}/${REPO}/releases/tags/${encodeURIComponent(tag)}`);
    if (releases?.id) return releases;
  } catch {
    /* 不存在则创建 */
  }
  return gitee(`/repos/${OWNER}/${REPO}/releases`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      tag_name: tag,
      name: tag,
      target_commitish: "main",
      body: `DeepSeek Otter ${tag}`,
      prerelease: false,
    }),
  });
}

const release = await findOrCreateRelease();
if (!release?.id) {
  console.error("✗ 无法获取/创建 Gitee 发行版");
  exit(1);
}
console.log(`✓ Gitee 发行版 v${version}（id=${release.id}）`);

// 待传附件：setup.exe + .sig（清单文件走仓库 raw，不传附件）。
const existing = new Set(
  (release.attach_files || release.assets || []).map((a) => a.name),
);
const files = readdirSync(distDir).filter(
  (f) => f.endsWith(".exe") || (f.endsWith(".sig") && !f.startsWith("latest")),
);

let ok = 0;
let skipped = 0;
let failed = 0;

for (const file of files) {
  if (existing.has(file)) {
    console.log(`↻ ${file} 已存在，跳过`);
    skipped++;
    continue;
  }
  const filePath = join(distDir, file);
  const sizeMB = statSync(filePath).size / 1024 / 1024;
  let done = false;
  for (let attempt = 1; attempt <= UPLOAD_RETRY && !done; attempt++) {
    try {
      // 大文件走 curl：Node fetch 对 >1MB multipart 断连（灵鉴 v0.2.3 实证）。
      if (statSync(filePath).size > 1024 * 1024) {
        const { stdout } = await execFileAsync("curl", [
          "-sS", "--fail-with-body", "--max-time", "600",
          "-X", "POST",
          "-H", `Authorization: token ${token}`,
          "-F", `file=@${filePath}`,
          `${GITEE_API}/repos/${OWNER}/${REPO}/releases/${release.id}/attach_files`,
        ]);
        if (stdout && !stdout.includes('"id"')) {
          console.warn(`  ? ${file} 响应异常: ${stdout.slice(0, 120)}`);
        }
      } else {
        const form = new FormData();
        form.append("file", new Blob([readFileSync(filePath)]), file);
        await gitee(`/repos/${OWNER}/${REPO}/releases/${release.id}/attach_files`, {
          method: "POST",
          body: form,
        });
      }
      console.log(`✓ ${file}（${sizeMB.toFixed(1)}MB）`);
      done = true;
      ok++;
    } catch (e) {
      const msg = e.stderr?.toString().trim() || e.message;
      if (attempt < UPLOAD_RETRY) {
        console.warn(`  ✗ ${file} 第 ${attempt} 次失败: ${msg}，重试...`);
        await new Promise((r) => setTimeout(r, attempt * 3000));
      } else {
        console.warn(`  ✗ ${file} 上传失败（跳过，不阻断）: ${msg}`);
        failed++;
      }
    }
  }
}

console.log(`\nGitee 同步: 成功 ${ok} | 跳过 ${skipped} | 失败 ${failed}`);
if (failed > 0) {
  console.warn("⚠ 有附件失败：重跑本脚本补传（幂等），或 Gitee 网页手动补传；verify 脚本会拦截缺失");
}
