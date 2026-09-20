// 控制中心"迁移工具"的长期记忆迁移：Zcode 的按项目记忆库
// （~/.zcode/cli/memories/projects/<slug>/memory/*.md，frontmatter 含 name/description）
// 迁到 dsh 侧两层落点：
//   正文 → <dshHome>/memories/<项目>/（原样迁移，仅 [[双链]] 转标准相对链接）
//   索引 → <dshHome>/AGENTS.md 标记段（每条一行：标题+摘要+路径，随全局指令常驻注入）
//
// 为什么不是全量进 AGENTS.md：dsh 用户全局唯一自动注入点 AGENTS.md 有 64KiB 预算
// （超限先丢最广的文件，即全局文件本身），而记忆正文可达数百 KB；「索引常驻 +
// 正文按需读」与 Zcode 原生形态（MEMORY.md 索引常驻、单条按需加载）同构。
// dsh 无 memory 子系统，workspace 概念对模型不可见，均非可用落点。
//
// 幂等：memories/ 整目录删除重建；AGENTS.md 标记段存在则替换、缺失则追加、
// 无文件则创建。与 import_agents_md 的整文件覆盖可互相恢复（推荐先迁 AGENTS.md
// 再迁记忆，反向顺序重导一次记忆即找回标记段）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::settings::dsh_home;

const SEGMENT_START: &str = "<!-- otter:memories:start -->";
const SEGMENT_END: &str = "<!-- otter:memories:end -->";

struct MemoryFile {
    file_name: String, // 含 .md，与 frontmatter name 同基名（Zcode 恒等约定）
    title: String,     // 索引展示标题：源 MEMORY.md 的人类可读标题 > frontmatter name
    description: String,
    body: String, // 原文（双链已转换）
}

struct ProjectMemories {
    slug: String,
    dir_name: String, // 输出目录名 = slug 去尾部哈希；同名冲突时回退完整 slug
    files: Vec<MemoryFile>,
}

/// slug 形态为 `<项目目录名小写>-<16 位十六进制>`，哈希由 CLI 对绝对路径派生、
/// 不可反推，仅作目录标识；展示与输出目录名去掉尾部哈希段。
fn strip_slug_hash(slug: &str) -> String {
    if slug.len() > 17 {
        let (head, tail) = slug.split_at(slug.len() - 17);
        if !head.is_empty() {
            if let Some(hex) = tail.strip_prefix('-') {
                if hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return head.to_string();
                }
            }
        }
    }
    slug.to_string()
}

/// Zcode 记忆的 `[[name]]` 双链在同项目目录内闭合（迁移前已全量校验无悬空），
/// dsh 侧模型不识该方言，转为标准相对链接 `[name](name.md)`；
/// 名称内含换行/嵌套括号的畸形片段原样保留不转换。
fn render_wikilinks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("[[") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        match after.find("]]") {
            Some(end) if !after[..end].is_empty()
                && !after[..end].contains('[')
                && !after[..end].contains('\n') =>
            {
                let name = &after[..end];
                out.push_str(&format!("[{name}]({name}.md)"));
                rest = &after[end + 2..];
            }
            _ => {
                out.push_str("[[");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 提取 frontmatter 的 name/description（线性扫描，思路同 skills.rs）。
/// description 存在 YAML 折叠多行形态（`>-` + 后续缩进行），需拼接为单行。
fn parse_name_description(text: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut desc: Option<String> = None;
    let mut in_fm = false;
    let mut folding = false; // 正在收集 description 的折叠续行
    for line in text.lines() {
        if !in_fm {
            if line.trim() == "---" {
                in_fm = true;
            }
            continue;
        }
        if line.trim() == "---" || line.trim() == "..." {
            break;
        }
        let t = line.trim_start();
        if line.starts_with(|c: char| !c.is_whitespace()) {
            folding = false;
            if let Some(v) = t.strip_prefix("name:") {
                name = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
            } else if let Some(v) = t.strip_prefix("description:") {
                let v = v.trim();
                if v.is_empty() || matches!(v, ">-" | ">" | "|-" | "|") {
                    desc = Some(String::new());
                    folding = true;
                } else {
                    desc = Some(v.trim_matches('"').trim_matches('\'').to_string());
                }
            }
        } else if folding {
            if let Some(d) = desc.as_mut() {
                if !d.is_empty() {
                    d.push(' ');
                }
                d.push_str(t);
            }
        }
    }
    (name, desc)
}

/// 解析源 MEMORY.md 的 `- [标题](文件.md)` 行，得到 name → 人类可读标题映射
/// （frontmatter 只有 slug 式 name，中文标题仅存于索引）。
fn parse_source_index(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("- [") else { continue };
        let Some(close) = rest.find("](") else { continue };
        let title = &rest[..close];
        let after = &rest[close + 2..];
        let Some(end) = after.find(')') else { continue };
        let file_name = after[..end].rsplit(['/', '\\']).next().unwrap_or(&after[..end]);
        let name = file_name.strip_suffix(".md").unwrap_or(file_name);
        if !title.is_empty() && !name.is_empty() {
            map.insert(name.to_string(), title.to_string());
        }
    }
    map
}

/// 收集单个项目的记忆：memory/*.md 为主，项目根散落的 *.md 是目录结构演进前的
/// 旧格式遗留一并收入。索引文件 MEMORY.md 只提供标题映射，本身不迁（重新生成）。
fn collect_project_files(proj_dir: &Path) -> Vec<MemoryFile> {
    let mut titles = HashMap::new();
    for idx in [proj_dir.join("memory").join("MEMORY.md"), proj_dir.join("MEMORY.md")] {
        if let Ok(text) = std::fs::read_to_string(&idx) {
            titles.extend(parse_source_index(&text));
        }
    }
    let mut files = Vec::new();
    let mem = proj_dir.join("memory");
    let roots: Vec<PathBuf> = if mem.is_dir() {
        vec![mem, proj_dir.to_path_buf()]
    } else {
        vec![proj_dir.to_path_buf()]
    };
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let Some(file_name) = p.file_name().and_then(|n| n.to_str()) else { continue };
            if !file_name.ends_with(".md") || file_name.eq_ignore_ascii_case("MEMORY.md") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&p) else { continue };
            let (fm_name, desc) = parse_name_description(&raw);
            let base = file_name.strip_suffix(".md").unwrap_or(file_name);
            let name = fm_name.unwrap_or_else(|| base.to_string());
            files.push(MemoryFile {
                file_name: file_name.to_string(),
                title: titles.get(base).cloned().unwrap_or(name),
                description: desc.unwrap_or_default(),
                body: render_wikilinks(&raw),
            });
        }
    }
    files.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    files
}

/// 索引行摘要截断：索引的目的是召回不是全文，超长描述（折叠拼接产物）截断。
fn shorten_desc(s: &str) -> String {
    if s.chars().count() > 160 {
        let t: String = s.chars().take(160).collect();
        format!("{t}…")
    } else {
        s.to_string()
    }
}

fn index_line(title: &str, link: &str, desc: &str) -> String {
    if desc.is_empty() {
        format!("- [{title}]({link})\n")
    } else {
        format!("- [{title}]({link}) — {}\n", shorten_desc(desc))
    }
}

/// 生成项目内索引（memories/<项目>/MEMORY.md，结构与源同构）。
fn render_memory_index(proj: &ProjectMemories) -> String {
    let mut out = format!("# {} 记忆索引\n\n迁自 Zcode；标题行括号内为同目录文件。\n\n", proj.dir_name);
    for f in &proj.files {
        out.push_str(&index_line(&f.title, &f.file_name, &f.description));
    }
    out
}

/// 渲染 AGENTS.md 标记段：全项目总索引常驻（预算 64KiB，真机 94 条实测约 9KB）。
fn render_agents_segment(projects: &[ProjectMemories]) -> String {
    let mut out = String::new();
    out.push_str(SEGMENT_START);
    out.push_str("\n## 从 Zcode 迁移的长期记忆\n\n");
    out.push_str("以下索引由 Otter 自动维护（重新迁移会整段替换）。与当前工作无关的项目直接忽略；相关条目需要完整背景时，用文件工具读取对应 md（路径相对本文件所在目录）。\n");
    for p in projects {
        out.push_str(&format!("\n### {}\n\n", p.dir_name));
        for f in &p.files {
            out.push_str(&index_line(&f.title, &format!("memories/{}/{}", p.dir_name, f.file_name), &f.description));
        }
    }
    out.push_str(SEGMENT_END);
    out.push('\n');
    out
}

/// 把标记段并入 AGENTS.md：已有标记段则原位替换（幂等），没有则文末追加，
/// 文件不存在则以段为全文创建。
fn splice_agents_md(existing: Option<&str>, segment: &str) -> String {
    let Some(text) = existing else {
        return segment.to_string();
    };
    if let (Some(s), Some(e)) = (text.find(SEGMENT_START), text.find(SEGMENT_END)) {
        if s < e {
            let mut after = &text[e + SEGMENT_END.len()..];
            // 已统一补过行尾 \n：吃掉原段后的第一个换行，避免幂等重导每次多一个空行
            if after.starts_with('\n') {
                after = &after[1..];
            }
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..s]);
            out.push_str(segment.trim_end());
            out.push('\n');
            out.push_str(after);
            return out;
        }
    }
    let mut out = text.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(segment.trim_end());
    out.push('\n');
    out
}

fn readme_text(dest_root: &Path) -> String {
    format!(
        "# 迁移自 Zcode 的长期记忆\n\n本目录由 Otter「迁移工具」生成：正文按原 Zcode 项目分组，各项目 `MEMORY.md` 为项目内索引；`AGENTS.md` 的「从 Zcode 迁移的长期记忆」标记段常驻总索引。重新执行迁移会删除并重建本目录，请勿在此存放自有文件。\n\n生成位置：{}\n",
        dest_root.display()
    )
}

/// 主流程（与 Tauri 命令解耦，便于用真实源 + 临时目标目录做落盘级测试）。
fn run_migration(projects_root: &Path, dsh_root: &Path) -> Result<String, String> {
    let mut projects: Vec<ProjectMemories> = Vec::new();
    let entries = std::fs::read_dir(projects_root).map_err(|e| format!("读取记忆库失败：{e}"))?;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let Some(slug) = p.file_name().and_then(|n| n.to_str()) else { continue };
        let files = collect_project_files(&p);
        if files.is_empty() {
            continue; // 建过项目但从未写入记忆
        }
        projects.push(ProjectMemories {
            slug: slug.to_string(),
            dir_name: strip_slug_hash(slug),
            files,
        });
    }
    if projects.is_empty() {
        return Err("Zcode 记忆库为空（没有任何项目写过记忆）".into());
    }
    projects.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));

    // 同名项目目录（同项目名不同路径去哈希后同名）会让输出互相覆盖：重名者保留完整 slug
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in &projects {
        *counts.entry(p.dir_name.clone()).or_default() += 1;
    }
    for p in projects.iter_mut() {
        if counts[&p.dir_name] > 1 {
            p.dir_name = p.slug.clone();
        }
    }

    let dest_root = dsh_root.join("memories");
    if dest_root.exists() {
        std::fs::remove_dir_all(&dest_root)
            .map_err(|e| format!("清理旧目录（{}）失败：{e}", dest_root.display()))?;
    }
    std::fs::create_dir_all(&dest_root).map_err(|e| format!("创建 {} 失败：{e}", dest_root.display()))?;

    let mut total = 0usize;
    for proj in &projects {
        let dir = dest_root.join(&proj.dir_name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
        for f in &proj.files {
            std::fs::write(dir.join(&f.file_name), &f.body)
                .map_err(|e| format!("写入 {} 失败：{e}", f.file_name))?;
        }
        std::fs::write(dir.join("MEMORY.md"), render_memory_index(proj))
            .map_err(|e| format!("写入 {} 索引失败：{e}", proj.dir_name))?;
        total += proj.files.len();
    }
    std::fs::write(dest_root.join("README.md"), readme_text(&dest_root))
        .map_err(|e| format!("写入 README 失败：{e}"))?;

    let segment = render_agents_segment(&projects);
    let agents_path = dsh_root.join("AGENTS.md");
    let existing = std::fs::read_to_string(&agents_path).ok();
    let merged = splice_agents_md(existing.as_deref(), &segment);
    std::fs::create_dir_all(dsh_root).map_err(|e| format!("创建 dsh 目录失败：{e}"))?;
    std::fs::write(&agents_path, merged).map_err(|e| format!("更新 AGENTS.md 失败：{e}"))?;

    // 64KiB 注入预算的保险线：段体超 32KiB 说明记忆库异常膨胀，不阻断但提示
    let warn = if segment.len() > 32 * 1024 {
        "（注意：索引段已超 32KiB，接近 dsh 全局指令 64KiB 注入预算，建议精简记忆库）"
    } else {
        ""
    };
    Ok(format!(
        "已迁移 {} 个项目共 {} 条记忆 → {}，AGENTS.md 已更新总索引{}",
        projects.len(),
        total,
        dest_root.display(),
        warn
    ))
}

#[tauri::command]
pub fn migrate_zcode_memories(app: tauri::AppHandle) -> Result<String, String> {
    // ~/.zcode/cli 可能是 junction（数据实际在别处），fs 层透明，直接读
    let home = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
    let projects_root = home.join(".zcode").join("cli").join("memories").join("projects");
    if !projects_root.is_dir() {
        return Err("未找到 Zcode 记忆库（~/.zcode/cli/memories/projects）".into());
    }
    run_migration(&projects_root, &dsh_home(&app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_hash_stripped() {
        assert_eq!(strip_slug_hash("deepseekotter-baff7144118f87f0"), "deepseekotter");
        assert_eq!(strip_slug_hash("100library-6b3a7da5abbfbf65"), "100library");
        // 尾段不是 16 位十六进制（哈希长度不足）不剥
        assert_eq!(strip_slug_hash("a-1234"), "a-1234");
        assert_eq!(strip_slug_hash("nosuffix"), "nosuffix");
    }

    #[test]
    fn wikilinks_converted_and_plain_links_kept() {
        let src = "见 [[otter-product-roadmap]] 与 [[scf-gitee-sync-runbook]]，普通 [链接](x.md) 与孤立 [[ 不动";
        assert_eq!(
            render_wikilinks(src),
            "见 [otter-product-roadmap](otter-product-roadmap.md) 与 [scf-gitee-sync-runbook](scf-gitee-sync-runbook.md)，普通 [链接](x.md) 与孤立 [[ 不动"
        );
    }

    #[test]
    fn frontmatter_single_line_and_folded_description() {
        let single = "---\nname: a\ndescription: \"一句话\"\nmetadata:\n  type: project\n---\n正文";
        assert_eq!(
            parse_name_description(single),
            (Some("a".into()), Some("一句话".into()))
        );
        let folded = "---\nname: b\ndescription: >-\n  第一行\n  续行\nmetadata:\n  type: project\n---\n正文";
        assert_eq!(
            parse_name_description(folded),
            (Some("b".into()), Some("第一行 续行".into()))
        );
    }

    #[test]
    fn source_index_titles_parsed() {
        let idx = "# MEMORY.md\n\n- [Otter 产品路线](otter-product-roadmap.md) — 摘要\n- [其他](./sub/other.md) — x\n普通行";
        let map = parse_source_index(idx);
        assert_eq!(map.get("otter-product-roadmap").map(String::as_str), Some("Otter 产品路线"));
        assert_eq!(map.get("other").map(String::as_str), Some("其他"));
    }

    fn sample_project() -> ProjectMemories {
        ProjectMemories {
            slug: "sample-0123456789abcdef".into(),
            dir_name: "sample".into(),
            files: vec![MemoryFile {
                file_name: "my-memory.md".into(),
                title: "我的记忆".into(),
                description: "描述".into(),
                body: "---\nname: my-memory\n---\n正文 [[other]]".into(),
            }],
        }
    }

    #[test]
    fn agents_segment_splice_append_and_idempotent() {
        let proj = sample_project();
        let seg = render_agents_segment(&[proj]);
        assert!(seg.starts_with(SEGMENT_START) && seg.ends_with(&format!("{SEGMENT_END}\n")));
        assert!(seg.contains("- [我的记忆](memories/sample/my-memory.md) — 描述"));

        // 无标记段的既有文件：追加且保留原内容
        let first = splice_agents_md(Some("# 用户指令\n\n正文"), &seg);
        assert!(first.starts_with("# 用户指令"));
        assert!(first.contains(SEGMENT_START));
        // 再次并入（幂等）：结果不变
        let second = splice_agents_md(Some(first.as_str()), &seg);
        assert_eq!(first, second);
    }

    #[test]
    fn splice_creates_file_content_when_missing() {
        let seg = format!("{SEGMENT_START}\n段内容\n{SEGMENT_END}\n");
        assert_eq!(splice_agents_md(None, &seg), seg);
    }

    /// 真机环境测试：装有 Zcode 记忆库时验证扫描可用、渲染段低于注入预算保险线；
    /// CI 或未装环境自动跳过。
    #[test]
    fn zcode_memories_scanable_when_present() {
        let root = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
            .join(".zcode")
            .join("cli")
            .join("memories")
            .join("projects");
        if !root.is_dir() {
            return;
        }
        let mut projects = Vec::new();
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let files = collect_project_files(&p);
            if files.is_empty() {
                continue;
            }
            for f in &files {
                assert!(!f.title.is_empty(), "title 不应为空：{}", f.file_name);
                assert!(!f.body.is_empty(), "正文不应为空：{}", f.file_name);
            }
            projects.push(ProjectMemories {
                slug: p.file_name().unwrap().to_string_lossy().into_owned(),
                dir_name: strip_slug_hash(&p.file_name().unwrap().to_string_lossy()),
                files,
            });
        }
        if projects.is_empty() {
            return; // 库存在但全空（从未写过记忆）
        }
        let segment = render_agents_segment(&projects);
        assert!(
            segment.len() < 32 * 1024,
            "索引段 {} 字节超保险线",
            segment.len()
        );
    }

    /// 真机环境测试：真实 Zcode 记忆库 → 临时目录完整落盘并重跑（幂等），
    /// 验证目录结构与 AGENTS.md 标记段替换；CI 或未装环境自动跳过。
    /// 生产隔离：目标固定在系统临时目录，绝不触碰真实 ~/.dsh。
    #[test]
    fn zcode_memories_migration_writes_and_idempotent_when_present() {
        let src = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
            .join(".zcode")
            .join("cli")
            .join("memories")
            .join("projects");
        if !src.is_dir() {
            return;
        }
        let dest_root = std::env::temp_dir().join("otter-mem-test");
        let _ = std::fs::remove_dir_all(&dest_root);
        // 库存在但全空（从未写过记忆）时无落盘可验，跳过
        let Ok(msg1) = run_migration(&src, &dest_root) else {
            return;
        };
        assert!(msg1.contains("已迁移"), "{msg1}");

        let agents = dest_root.join("AGENTS.md");
        let text1 = std::fs::read_to_string(&agents).unwrap();
        assert!(text1.contains(SEGMENT_START) && text1.contains(SEGMENT_END));
        assert!(text1.contains("(memories/"), "索引段应含 memories/ 相对链接");
        assert!(dest_root.join("memories").join("README.md").is_file());
        // 各项目目录应有正文与 MEMORY.md 索引
        for dir in std::fs::read_dir(dest_root.join("memories")).unwrap().flatten() {
            let p = dir.path();
            if !p.is_dir() {
                continue;
            }
            assert!(p.join("MEMORY.md").is_file(), "{} 缺索引", p.display());
        }

        // 幂等：重跑后 AGENTS.md 原样（标记段替换而非重复追加）
        run_migration(&src, &dest_root).unwrap();
        let text2 = std::fs::read_to_string(&agents).unwrap();
        assert_eq!(text1, text2, "重导后 AGENTS.md 应幂等不变");

        std::fs::remove_dir_all(&dest_root).unwrap();
    }
}
