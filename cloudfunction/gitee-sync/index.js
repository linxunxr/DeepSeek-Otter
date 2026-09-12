// DeepSeek Otter 发版中转云函数：GitHub Release → Gitee 附件 + 清单升级。
//
// 部署在境内可达 GitHub 的 Region（推荐香港），流程：
//   HTTP 触发（?secret=…&version=0.1.0）→ 下载 GitHub Release 附件（setup.exe + .sig）
//   → Gitee 建/查发行版 → attach_files 上传附件 → 拼 Gitee-URL 清单 → PUT latest.json
//
// 环境变量：
//   GITEE_TOKEN   必填，Gitee 私人令牌（projects 权限）
//   SYNC_SECRET   必填，触发鉴权密钥（GitHub Actions Secrets 同名配置）
//   GITEE_OWNER   缺省 mwcxlinxun
//   GITEE_REPO    缺省 deep-seek-otter
//
// 幂等：附件已存在跳过；latest.json 已是目标版本内容时仍重写（无副作用）。
// 失败重试：触发方（GitHub Actions）负责整体重试；本函数内单文件上传重试 2 次。

const GITEE_API = "https://gitee.com/api/v5";
const GITHUB_RAW = "https://github.com/linxunxr/DeepSeek-Otter/releases/download";

const OWNER = process.env.GITEE_OWNER || "mwcxlinxun";
const REPO = process.env.GITEE_REPO || "deep-seek-otter";
const TOKEN = process.env.GITEE_TOKEN;
const SECRET = process.env.SYNC_SECRET;

// SCF 事件函数入口。
exports.main_handler = async (event) => {
  // HTTP 触发：query 在 event.queryStringParameters。
  const q = event.queryStringParameters || {};
  if (SECRET && q.secret !== SECRET) {
    return reply(403, { error: "secret mismatch" });
  }
  const version = (q.version || "").replace(/^v/, "");
  if (!version || !/^[\w.\-]+$/.test(version)) {
    return reply(400, { error: "version required (e.g. 0.1.0)" });
  }
  if (!TOKEN) {
    return reply(500, { error: "GITEE_TOKEN not configured" });
  }

  const tag = `v${version}`;
  const assetBase = `${GITHUB_RAW}/${tag}`;
  const exeName = `DeepSeek Otter_${version}_x64-setup.exe`;
  const sigName = `${exeName}.sig`;
  const log = [];

  try {
    // 1. 从 GitHub 下载附件（Setup.exe ~51MB；SCF /tmp 空间足够）。
    log.push(`下载 ${exeName}…`);
    const exeBuf = await download(`${assetBase}/${encodeURIComponent(exeName)}`);
    log.push(`exe ${mb(exeBuf.length)} 下载完成`);
    const sigText = (await download(`${assetBase}/${encodeURIComponent(sigName)}`)).toString("utf-8");
    log.push("sig 下载完成");

    // 2. Gitee 发行版（建/查）。
    const release = await findOrCreateRelease(tag, version);
    log.push(`发行版 id=${release.id}`);

    // 3. 上传附件（幂等：已存在跳过）。
    const existing = new Set((release.attach_files || release.assets || []).map((a) => a.name));
    if (!existing.has(exeName)) {
      await uploadAttachment(release.id, exeName, exeBuf, log);
    } else {
      log.push("exe 附件已存在，跳过");
    }
    if (!existing.has(sigName)) {
      await uploadAttachment(release.id, sigName, Buffer.from(sigText, "utf-8"), log);
    } else {
      log.push("sig 附件已存在，跳过");
    }

    // 4. 升级 latest.json 为 Gitee-URL 清单。
    const manifest = {
      version,
      notes: `${tag} 更新`,
      pub_date: new Date().toISOString(),
      platforms: {
        "windows-x86_64": {
          signature: sigText.trim(),
          url: `${GITEE_BASE()}/${tag}/${encodeURIComponent(exeName)}`,
        },
      },
    };
    await syncLatestJson(manifest);
    log.push("latest.json 已升级为 Gitee-URL 清单");

    return reply(200, { ok: true, log });
  } catch (e) {
    log.push(`失败：${e.message}`);
    return reply(500, { error: e.message, log });
  }
};

const GITEE_BASE = () => `https://gitee.com/${OWNER}/${REPO}/releases/download`;

function reply(statusCode, body) {
  return {
    isBase64Encoded: false,
    statusCode,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body, null, 2),
  };
}

const mb = (n) => (n / 1024 / 1024).toFixed(1) + "MB";

async function download(url) {
  const resp = await fetch(url, { redirect: "follow", signal: AbortSignal.timeout(600_000) });
  if (!resp.ok) throw new Error(`下载失败 HTTP ${resp.status}: ${url}`);
  return Buffer.from(await resp.arrayBuffer());
}

async function giteeApi(path, init = {}) {
  const resp = await fetch(`${GITEE_API}${path}`, {
    ...init,
    headers: { "Content-Type": "application/json", ...(init.headers || {}) },
    body: init.body ? JSON.stringify({ access_token: TOKEN, ...init.body }) : undefined,
    signal: AbortSignal.timeout(60_000),
  });
  const text = await resp.text();
  if (!resp.ok) throw new Error(`Gitee API ${resp.status} ${path}: ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : null;
}

async function findOrCreateRelease(tag, version) {
  try {
    const rel = await giteeApi(`/repos/${OWNER}/${REPO}/releases/tags/${encodeURIComponent(tag)}`);
    if (rel?.id) return rel;
  } catch {
    /* 不存在则创建 */
  }
  return giteeApi(`/repos/${OWNER}/${REPO}/releases`, {
    method: "POST",
    body: { tag_name: tag, name: tag, target_commitish: "main", body: `DeepSeek Otter ${tag}`, prerelease: false },
  });
}

// multipart 上传。Node 18 全局可用 FormData/Blob。
async function uploadAttachment(releaseId, name, buf, log) {
  for (let attempt = 1; attempt <= 3; attempt++) {
    try {
      const form = new FormData();
      form.append("access_token", TOKEN);
      form.append("file", new Blob([buf]), name);
      const resp = await fetch(`${GITEE_API}/repos/${OWNER}/${REPO}/releases/${releaseId}/attach_files`, {
        method: "POST",
        body: form,
        signal: AbortSignal.timeout(600_000),
      });
      const text = await resp.text();
      if (!resp.ok || !text.includes('"id"')) {
        throw new Error(`HTTP ${resp.status}: ${text.slice(0, 150)}`);
      }
      log.push(`✓ ${name}（${mb(buf.length)}）`);
      return;
    } catch (e) {
      if (attempt >= 3) throw new Error(`附件 ${name} 上传失败：${e.message}`);
      log.push(`  ${name} 第 ${attempt} 次失败（${e.message}），重试…`);
      await new Promise((r) => setTimeout(r, 5000));
    }
  }
}

// Gitee contents API：GET 拿 sha → PUT 更新 / POST 创建（path 必须在 URL）。
async function syncLatestJson(manifest) {
  const path = `/repos/${OWNER}/${REPO}/contents/${encodeURIComponent("latest.json")}`;
  const content = Buffer.from(JSON.stringify(manifest, null, 2) + "\n", "utf-8").toString("base64");
  let sha;
  try {
    sha = (await giteeApi(path))?.sha;
  } catch {
    /* 不存在 */
  }
  if (sha) {
    await giteeApi(path, { method: "PUT", body: { content, message: "chore(updater): 升级为 Gitee-URL 清单", sha } });
  } else {
    await giteeApi(path, { method: "POST", body: { content, message: "chore(updater): 新增 latest.json" } });
  }
}
