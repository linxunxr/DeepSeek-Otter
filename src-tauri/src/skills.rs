// 控制中心"Skill 与迁移工具"的后端。
//
// dsh skill 约定（dsh-skill-filesystem）：用户全局根 `~/.dsh/skills`，条目为
// `<name>/SKILL.md`（frontmatter 必需 name/description）或扁平 `<name>.md`，
// 目录被监听、免重启生效。ZCode 的 skill 同为 SKILL.md 目录格式——迁移即复制。
//
// 迁移工具：AGENTS.md（~/.zcode/AGENTS.md → ~/.dsh/AGENTS.md，dsh 用户全局指令）
// 与 skills 批量导入；会话历史迁移因两侧格式差异大暂不支持。

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::settings::dsh_home;

#[derive(Serialize, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// flat 表示扁平 <name>.md，bundle 表示目录型 <name>/SKILL.md。
    pub kind: String,
}

fn skills_root(app: &tauri::AppHandle) -> PathBuf {
    dsh_home(app).join("skills")
}

/// 解析 SKILL.md 的 YAML frontmatter（只需 name/description 两行，线性提取足够；
/// frontmatter 块外的同名字段不误读）。
fn parse_frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut desc = None;
    let mut in_fm = false;
    for line in text.lines() {
        let t = line.trim();
        if !in_fm {
            if t == "---" {
                in_fm = true;
            }
            continue;
        }
        if t == "---" || t == "..." {
            break;
        }
        if let Some(v) = t.strip_prefix("name:") {
            name = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(v) = t.strip_prefix("description:") {
            desc = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    (name, desc)
}

/// 扫描一个 skill 根目录（kind 标注条目形态；解析失败的条目以文件名为 name 兜底）。
fn scan_root(root: &Path) -> Vec<SkillInfo> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Some(file_name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            let md = p.join("SKILL.md");
            if md.exists() {
                let text = std::fs::read_to_string(&md).unwrap_or_default();
                let (name, desc) = parse_frontmatter(&text);
                out.push(SkillInfo {
                    name: name.unwrap_or_else(|| file_name.to_string()),
                    description: desc.unwrap_or_default(),
                    kind: "bundle".into(),
                });
            }
        } else if let Some(base) = file_name.strip_suffix(".md") {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let (name, desc) = parse_frontmatter(&text);
            out.push(SkillInfo {
                name: name.unwrap_or_else(|| base.to_string()),
                description: desc.unwrap_or_default(),
                kind: "flat".into(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[tauri::command]
pub fn list_skills(app: tauri::AppHandle) -> Vec<SkillInfo> {
    scan_root(&skills_root(&app))
}

/// 扫描迁移源（如 ~/.zcode/skills）；源根不存在返回空列表而非报错（未装 ZCode 正常）。
#[tauri::command]
pub fn list_source_skills(app: tauri::AppHandle, source: String) -> Vec<SkillInfo> {
    let root = if source == "zcode" {
        PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
            .join(".zcode")
            .join("skills")
    } else {
        PathBuf::from(source)
    };
    scan_root(&root)
}

/// 复制一个 skill 条目（目录递归或单文件）。目标已存在时跳过并计数，不覆盖。
fn copy_skill_entry(src_root: &Path, name: &str, dest_root: &Path) -> Result<bool, String> {
    let src = src_root.join(name);
    let dest = dest_root.join(name);
    if dest.exists() {
        return Ok(false);
    }
    if !src.exists() {
        return Err(format!("源条目不存在：{name}"));
    }
    if src.is_file() {
        std::fs::copy(&src, &dest).map_err(|e| format!("复制 {name} 失败：{e}"))?;
        return Ok(true);
    }
    copy_dir_recursive(&src, &dest).map_err(|e| format!("复制 {name} 失败：{e}"))?;
    Ok(true)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let from = e.path();
        let to = dest.join(e.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 批量导入 skill（source=zcode 或绝对路径）。返回 (导入数, 跳过数)。
#[tauri::command]
pub fn import_skills(app: tauri::AppHandle, source: String, names: Vec<String>) -> Result<(u32, u32), String> {
    let src_root = if source == "zcode" {
        PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
            .join(".zcode")
            .join("skills")
    } else {
        PathBuf::from(source)
    };
    if !src_root.exists() {
        return Err(format!("源目录不存在：{}", src_root.display()));
    }
    let dest_root = skills_root(&app);
    std::fs::create_dir_all(&dest_root).map_err(|e| format!("创建 skills 目录失败：{e}"))?;
    let mut imported = 0;
    let mut skipped = 0;
    for name in &names {
        if name.contains("..") || name.contains('/') || name.contains('\\') {
            return Err(format!("条目名 {name:?} 非法（不允许路径分隔符）"));
        }
        match copy_skill_entry(&src_root, name, &dest_root)? {
            true => imported += 1,
            false => skipped += 1,
        }
    }
    Ok((imported, skipped))
}

#[tauri::command]
pub fn delete_skill(app: tauri::AppHandle, name: String) -> Result<(), String> {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err("条目名非法".into());
    }
    let root = skills_root(&app);
    let dir = root.join(&name);
    let flat = root.join(format!("{name}.md"));
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("删除失败：{e}"))
    } else if flat.is_file() {
        std::fs::remove_file(&flat).map_err(|e| format!("删除失败：{e}"))
    } else {
        Err(format!("不存在：{name}"))
    }
}

/// 迁移 ZCode 全局指令：~/.zcode/AGENTS.md → ~/.dsh/AGENTS.md（已存在时先备份）。
#[tauri::command]
pub fn import_agents_md(app: tauri::AppHandle) -> Result<String, String> {
    let src = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
        .join(".zcode")
        .join("AGENTS.md");
    let dest = dsh_home(&app).join("AGENTS.md");
    if !src.is_file() {
        return Err("未找到 ~/.zcode/AGENTS.md（ZCode 未配置全局指令）".into());
    }
    std::fs::create_dir_all(dsh_home(&app)).map_err(|e| format!("创建 ~/.dsh 失败：{e}"))?;
    if dest.exists() {
        let backup = dsh_home(&app).join("AGENTS.md.otter-bak");
        std::fs::copy(&dest, &backup).map_err(|e| format!("备份现有指令失败：{e}"))?;
        std::fs::copy(&src, &dest).map_err(|e| format!("覆盖失败：{e}"))?;
        return Ok(format!(
            "已覆盖（原文件备份为 AGENTS.md.otter-bak）：{}",
            dest.display()
        ));
    }
    std::fs::copy(&src, &dest).map_err(|e| format!("复制失败：{e}"))?;
    Ok(format!("已迁移：{}", dest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_parses_name_and_description() {
        let text = "---\nname: my-skill\ndescription: \"做某事的技能\"\n---\n\n# Body\nname: not-frontmatter";
        let (n, d) = parse_frontmatter(text);
        assert_eq!(n.as_deref(), Some("my-skill"));
        assert_eq!(d.as_deref(), Some("做某事的技能"));
    }

    #[test]
    fn frontmatter_missing_returns_none() {
        let (n, d) = parse_frontmatter("# 没有 frontmatter");
        assert!(n.is_none() && d.is_none());
    }

    /// 真机环境测试：装有 Zcode（~/.zcode/skills 存在）时验证扫描与解析；
    /// CI 或未装环境自动跳过。
    #[test]
    fn zcode_skills_scanable_when_present() {
        let root = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
            .join(".zcode")
            .join("skills");
        if !root.is_dir() {
            return;
        }
        let list = scan_root(&root);
        assert!(!list.is_empty(), "Zcode skills 目录存在但扫描为空");
        for s in &list {
            assert!(!s.name.is_empty(), "skill name 不应为空");
        }
    }
}
