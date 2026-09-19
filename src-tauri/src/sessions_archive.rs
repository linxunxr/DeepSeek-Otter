// 控制中心"迁移工具"的会话归档：读取 Zcode 的会话库（SQLite，表 session/message/part），
// 每个会话导出为 Markdown 归档（标题/目录/时间/对话正文），写入 <dshHome>/imported-sessions/。
//
// 为什么是 Markdown 归档而不是可恢复会话：dsh 会话是私有 SessionEvent V3 流
// （zstd 帧，无官方导入 API），伪造内部格式有污染会话库的风险；归档可查阅可搜索，
// 待 dsh 提供官方导入通道后再升级为格式级迁移。
//
// 实现避开 Rust sqlite 依赖：借壳自带的 Node（24，内置 node:sqlite）执行内嵌脚本，
// 只读打开 Zcode 库，绝不写回。

use std::process::Command;

use crate::backend::resolve_node;
use crate::settings::dsh_home;

/// 内嵌迁移脚本（node:sqlite 只读遍历，输出 JSON 结果）。
const MIGRATE_SCRIPT: &str = r####"
const { DatabaseSync } = require("node:sqlite");
const fs = require("node:fs");
const path = require("node:path");

const [dbPath, outDir] = process.argv.slice(2);
if (!dbPath || !outDir) { console.error("usage: node migrate.js <db> <outDir>"); process.exit(2); }
if (!fs.existsSync(dbPath)) { console.error("db not found: " + dbPath); process.exit(3); }

fs.mkdirSync(outDir, { recursive: true });
const db = new DatabaseSync(dbPath, { readOnly: true });

const sessions = db
  .prepare("SELECT id, title, directory, time_created, time_updated FROM session ORDER BY time_created")
  .all();

const partsByMessage = new Map();
for (const p of db.prepare("SELECT message_id, data, sequence FROM part ORDER BY sequence").all()) {
  const list = partsByMessage.get(p.message_id) ?? [];
  list.push(p);
  partsByMessage.set(p.message_id, list);
}

const fmtTime = (ms) => (ms ? new Date(ms).toISOString().replace("T", " ").slice(0, 19) : "?");

let exported = 0;
let skipped = 0;
const names = new Set();
for (const s of sessions) {
  const msgs = db
    .prepare("SELECT id, data FROM message WHERE session_id = ? ORDER BY sequence")
    .all(s.id);
  const lines = [];
  for (const m of msgs) {
    let d = {};
    try { d = JSON.parse(m.data); } catch { continue; }
    const role = d.role;
    if (role !== "user" && role !== "assistant") continue;
    const parts = (partsByMessage.get(m.id) ?? [])
      .map((p) => { try { return JSON.parse(p.data); } catch { return null; } })
      .filter((p) => p && p.type === "text" && typeof p.text === "string" && p.text.trim());
    if (!parts.length) continue;
    lines.push(`## ${role === "user" ? "用户" : "助手"}（${fmtTime(d.time?.created ?? s.time_created)}）\n`);
    for (const p of parts) lines.push(p.text.trim() + "\n");
  }
  if (!lines.length) { skipped++; continue; }

  const stamp = s.time_created ? new Date(s.time_created).toISOString().slice(0, 10) : "unknown";
  let base = String(s.title ?? "").replace(/[\\/:*?"<>|\r\n]+/g, " ").trim().slice(0, 40) || "untitled";
  let name = `${stamp} ${base}.md`;
  let i = 2;
  while (names.has(name.toLowerCase())) { name = `${stamp} ${base} (${i++}).md`; }
  names.add(name.toLowerCase());

  const head = [
    `# ${s.title ?? "（无标题）"}`,
    ``,
    `- 迁移自 Zcode（归档只读，非可恢复会话）`,
    `- 工作目录：${s.directory ?? "?"}`,
    `- 创建：${fmtTime(s.time_created)}　更新：${fmtTime(s.time_updated)}`,
    ``,
  ].join("\n");
  fs.writeFileSync(path.join(outDir, name), head + lines.join("\n") + "\n", "utf8");
  exported++;
}

console.log(JSON.stringify({ exported, skipped, total: sessions.length }));
"####;

#[tauri::command]
pub fn migrate_zcode_sessions(app: tauri::AppHandle) -> Result<String, String> {
    // Zcode 数据目录：~/.zcode 或其实际位置（cli 可能是指向别处的符号链接，
    // 会话库在 <真实 cli>/db/db.sqlite；两处都探测）。
    let home = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
    let candidates = [
        home.join(".zcode/cli/db/db.sqlite"),
        home.join(".zcode/db/db.sqlite"),
    ];
    let db_path = candidates
        .iter()
        .find(|p| p.is_file())
        .ok_or("未找到 Zcode 会话库（~/.zcode/cli/db/db.sqlite）")?;

    let out_dir = dsh_home(&app).join("imported-sessions");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建输出目录失败：{e}"))?;

    let node = resolve_node(&app).ok_or("找不到内置 node 运行时")?;
    // 用 dsh 入口同目录定位 node；脚本落临时文件后以 node 直接执行（不经 dsh）。
    let script_path = out_dir.join(".migrate-zcode-sessions.cjs");
    std::fs::write(&script_path, MIGRATE_SCRIPT).map_err(|e| format!("写入临时脚本失败：{e}"))?;
    let out = Command::new(&node)
        .arg(&script_path)
        .arg(db_path)
        .arg(&out_dir)
        .output()
        .map_err(|e| format!("执行迁移脚本失败：{e}"));
    let _ = std::fs::remove_file(&script_path);
    let out = out?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(format!("迁移脚本失败：{}", text));
    }
    // 最后一行是 JSON 结果。
    let last = text.trim().lines().last().unwrap_or("");
    serde_json::from_str::<serde_json::Value>(last)
        .map_err(|_| format!("迁移脚本输出异常：{text}"))?;
    Ok(format!(
        "已导出 {} 个会话（跳过空会话 {} 个）→ {}",
        serde_json::from_str::<serde_json::Value>(last).unwrap()["exported"],
        serde_json::from_str::<serde_json::Value>(last).unwrap()["skipped"],
        out_dir.display()
    ))
}
