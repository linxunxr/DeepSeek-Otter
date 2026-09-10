// 校验 Gitee 发行版附件是否齐全（"先降级后升级"发布链路的开关）。
// 背景（灵鉴 v0.5.1 事故教训）：updater 的多端点回退只发生在拉清单阶段，
// 下载失败不回退。若 Gitee 清单指向不存在的附件，用户更新直接失败。
// 因此：先提交 GitHub-URL 清单保底 → 本脚本确认附件齐全 → 才升级为 Gitee-URL 清单。
//
// 用法: node scripts/verify-gitee-assets.mjs <version> <dist-dir>
//   附件齐全 exit 0；缺失/发行版不存在 exit 1；API 失败 exit 2。

import { readdirSync } from "node:fs";
import { join } from "node:path";
import { exit } from "node:process";

const GITEE_API = "https://gitee.com/api/v5";
const OWNER = process.env.GITEE_OWNER || "mwcxlinxun";
const REPO = process.env.GITEE_REPO || "deep-seek-otter";
const token = process.env.GITEE_TOKEN;

const version = process.argv[2]?.replace(/^v/, "");
const distDir = process.argv[3];

if (!version || !distDir) {
  console.error("用法: node scripts/verify-gitee-assets.mjs <version> <dist-dir>");
  exit(1);
}

const expected = readdirSync(distDir).filter(
  (f) => f.endsWith(".exe") || (f.endsWith(".sig") && !f.startsWith("latest")),
);

try {
  const resp = await fetch(
    `${GITEE_API}/repos/${OWNER}/${REPO}/releases/tags/${encodeURIComponent(`v${version}`)}`,
    token ? { headers: { authorization: `token ${token}` } } : {},
    { signal: AbortSignal.timeout(30_000) },
  );
  if (!resp.ok) {
    console.error(`✗ 发行版不存在（HTTP ${resp.status}）`);
    exit(1);
  }
  const release = await resp.json();
  const existing = new Set(
    (release.attach_files || release.assets || []).map((a) => a.name),
  );
  const missing = expected.filter((f) => !existing.has(f));
  if (missing.length > 0) {
    console.error(`✗ 缺失附件（保持 GitHub-URL 清单，勿升级 Gitee-URL）：`);
    for (const m of missing) console.error(`  - ${m}`);
    exit(1);
  }
  console.log(`✓ 附件齐全（${expected.length} 个），可以升级为 Gitee-URL 清单`);
  exit(0);
} catch (e) {
  console.error(`✗ API 查询失败：${e.message}`);
  exit(2);
}
