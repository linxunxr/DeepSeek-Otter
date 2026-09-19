// 控制中心页面逻辑：导航切换 + 概览与更新（复用 updater.ts）+ 模型与供应商 + 诊断导出。
// 壳本地页面之一，与加载页同源，享有 Tauri IPC 权限（见 capabilities）。

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { initUpdaterUI, doCheck } from "./updater";

// 版本徽标：顶栏 + 概览页当前版本。
getVersion()
  .then((v) => {
    const badge = document.getElementById("ver-badge");
    const cur = document.getElementById("cur-ver");
    if (badge) badge.textContent = `v${v}`;
    if (cur) cur.textContent = v;
  })
  .catch(() => {});

// 左侧导航切换内容区。
const nav = document.getElementById("nav");
if (nav) {
  nav.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>("button[data-page]");
    if (!btn) return;
    const page = btn.dataset.page ?? "";
    nav.querySelectorAll("button").forEach((b) => b.classList.toggle("active", b === btn));
    document
      .querySelectorAll("main section")
      .forEach((s) => s.classList.toggle("active", s.id === `page-${page}`));
  });
}

// 更新面板与加载页共用 updater.ts；initUpdaterUI 自带启动静默检查。
initUpdaterUI();
document.getElementById("check-now")?.addEventListener("click", () => void doCheck(false));

// ===================== 模型与供应商 =====================

interface ModelEntry {
  id: string;
  name: string;
  contextWindow: number | null;
}
interface ProviderEntry {
  name: string;
  displayName: string;
  api: string;
  baseURL: string;
  apiKeyEnv: string;
  models: ModelEntry[];
}
interface ModelConfig {
  providers: ProviderEntry[];
  defaultProvider: string | null;
  defaultModel: string | null;
}

let modelConfig: ModelConfig = { providers: [], defaultProvider: null, defaultModel: null };
let modelDirty = false;
let editingIndex = -1; // -1 = 新增

function el<T extends HTMLElement>(id: string): T {
  return document.getElementById(id) as T;
}

function markDirty(dirty: boolean): void {
  modelDirty = dirty;
  (el<HTMLButtonElement>("save-models")).disabled = !dirty;
  el<HTMLButtonElement>("restart-backend-btn").style.display = dirty ? "none" : "inline-block";
  el("models-status").textContent = dirty ? "有未保存的修改" : "";
}

function renderProviders(): void {
  const list = el("provider-list");
  list.innerHTML = "";
  el("models-empty").style.display = modelConfig.providers.length ? "none" : "block";
  modelConfig.providers.forEach((p, i) => {
    const item = document.createElement("div");
    item.className = "provider-item";
    const models = p.models.map((m) => m.id).join("、") || "（未配置模型）";
    item.innerHTML = `
      <div class="p-main">
        <div class="p-name">${p.displayName || p.name}</div>
        <div class="p-meta">${p.api} · ${p.baseURL || "（无 baseURL）"} · 模型：${models}</div>
      </div>
      <div class="p-ops">
        <button data-op="edit" data-i="${i}">编辑</button>
        <button data-op="del" data-i="${i}">删除</button>
      </div>`;
    list.appendChild(item);
  });
  renderDefaultSelects();
}

function renderDefaultSelects(): void {
  const pSel = el<HTMLSelectElement>("default-provider");
  const mSel = el<HTMLSelectElement>("default-model");
  pSel.innerHTML = `<option value="">（内置默认）</option>`;
  modelConfig.providers.forEach((p) => {
    const o = document.createElement("option");
    o.value = p.name;
    o.textContent = p.displayName || p.name;
    pSel.appendChild(o);
  });
  pSel.value = modelConfig.defaultProvider ?? "";
  const refreshModels = (): void => {
    mSel.innerHTML = "";
    const p = modelConfig.providers.find((x) => x.name === pSel.value);
    (p?.models ?? []).forEach((m) => {
      const o = document.createElement("option");
      o.value = m.id;
      o.textContent = m.name ? `${m.name}（${m.id}）` : m.id;
      mSel.appendChild(o);
    });
    mSel.value = p && p.models.some((m) => m.id === modelConfig.defaultModel)
      ? (modelConfig.defaultModel as string)
      : "";
    mSel.disabled = !p;
  };
  refreshModels();
  pSel.onchange = () => {
    modelConfig.defaultProvider = pSel.value || null;
    modelConfig.defaultModel = null;
    refreshModels();
    markDirty(true);
  };
  mSel.onchange = () => {
    modelConfig.defaultModel = mSel.value || null;
    markDirty(true);
  };
}

// ---- 编辑器 ----

// 预置供应商模板：baseURL/协议一键填充；模型 id 为常用参考，请按供应商文档核对增改。
const PROVIDER_TEMPLATES: { label: string; data: Partial<ProviderEntry> }[] = [
  {
    label: "GLM（智谱）",
    data: {
      name: "glm", displayName: "GLM（智谱）", api: "openai-completions",
      baseURL: "https://open.bigmodel.cn/api/paas/v4", apiKeyEnv: "GLM_API_KEY",
      models: [{ id: "glm-5", name: "", contextWindow: null }],
    },
  },
  {
    label: "Kimi（月之暗面）",
    data: {
      name: "kimi", displayName: "Kimi（月之暗面）", api: "openai-completions",
      baseURL: "https://api.moonshot.cn/v1", apiKeyEnv: "MOONSHOT_API_KEY",
      models: [{ id: "kimi-k2.6", name: "", contextWindow: null }],
    },
  },
  {
    label: "MiniMax",
    data: {
      name: "minimax", displayName: "MiniMax", api: "openai-completions",
      baseURL: "https://api.minimaxi.com/v1", apiKeyEnv: "MINIMAX_API_KEY",
      models: [],
    },
  },
  {
    label: "通义千问（阿里）",
    data: {
      name: "qwen", displayName: "通义千问", api: "openai-completions",
      baseURL: "https://dashscope.aliyuncs.com/compatible-mode/v1", apiKeyEnv: "DASHSCOPE_API_KEY",
      models: [],
    },
  },
  {
    label: "OpenAI",
    data: {
      name: "openai", displayName: "OpenAI", api: "openai-completions",
      baseURL: "https://api.openai.com/v1", apiKeyEnv: "OPENAI_API_KEY",
      models: [],
    },
  },
  {
    label: "Anthropic",
    data: {
      name: "anthropic", displayName: "Anthropic", api: "anthropic",
      baseURL: "https://api.anthropic.com", apiKeyEnv: "ANTHROPIC_API_KEY",
      models: [{ id: "claude-sonnet-4-5", name: "", contextWindow: 200000 }],
    },
  },
];

function renderModelRows(models: ModelEntry[]): void {
  const box = el("model-rows");
  box.innerHTML = "";
  models.forEach((m, i) => {
    const row = document.createElement("div");
    row.className = "model-row";
    row.innerHTML = `
      <input data-f="id" data-i="${i}" placeholder="模型 id，如 gpt-5" value="${m.id.replace(/"/g, "&quot;")}" />
      <input data-f="name" data-i="${i}" placeholder="显示名（可选）" value="${m.name.replace(/"/g, "&quot;")}" />
      <input data-f="cw" data-i="${i}" placeholder="上下文窗口" value="${m.contextWindow ?? ""}" />
      <button data-op="del-model" data-i="${i}" title="删除">×</button>`;
    box.appendChild(row);
  });
}

function collectEditorModels(): { models: ModelEntry[]; error: string | null } {
  const models: ModelEntry[] = [];
  const inputs = el("model-rows").querySelectorAll<HTMLInputElement>("input");
  const rows = new Map<number, Record<string, string>>();
  inputs.forEach((inp) => {
    const i = Number(inp.dataset.i);
    const r = rows.get(i) ?? {};
    r[inp.dataset.f as string] = inp.value.trim();
    rows.set(i, r);
  });
  for (const [, r] of rows) {
    if (!r.id && !r.name && !r.cw) continue; // 整行为空跳过
    if (!r.id) return { models, error: "模型 id 必填" };
    const cw = r.cw ? Number(r.cw) : null;
    if (cw !== null && (!Number.isFinite(cw) || cw <= 0)) return { models, error: "上下文窗口须为正整数" };
    models.push({ id: r.id, name: r.name ?? "", contextWindow: cw });
  }
  return { models, error: null };
}

function openEditor(index: number): void {
  editingIndex = index;
  const p = index >= 0 ? modelConfig.providers[index] : null;
  el("editor-title").textContent = p ? `编辑供应商：${p.name}` : "添加供应商";
  el<HTMLInputElement>("f-name").value = p?.name ?? "";
  el<HTMLInputElement>("f-name").disabled = !!p; // 路由名是键，编辑态禁改
  el<HTMLInputElement>("f-display").value = p?.displayName ?? "";
  el<HTMLSelectElement>("f-api").value = p?.api ?? "openai-completions";
  el<HTMLInputElement>("f-base").value = p?.baseURL ?? "";
  el<HTMLInputElement>("f-keyenv").value = p?.apiKeyEnv ?? "";
  renderModelRows(p?.models ?? []);
  el("editor-error").textContent = "";
  el("provider-editor").style.display = "block";
}

function closeEditor(): void {
  editingIndex = -2; // 关闭态
  el("provider-editor").style.display = "none";
}

function initModelsPage(): void {
  // 模板下拉：选中即以模板预填编辑器（名称冲突时自动加后缀）。
  const tplSel = el<HTMLSelectElement>("provider-template");
  PROVIDER_TEMPLATES.forEach((t, i) => {
    const o = document.createElement("option");
    o.value = String(i);
    o.textContent = t.label;
    tplSel.appendChild(o);
  });
  tplSel.onchange = () => {
    const tpl = PROVIDER_TEMPLATES[Number(tplSel.value)];
    tplSel.value = "";
    if (!tpl) return;
    openEditor(-1);
    const d = tpl.data;
    let name = d.name ?? "";
    if (modelConfig.providers.some((p) => p.name === name)) {
      name = `${name}-2`;
    }
    el<HTMLInputElement>("f-name").value = name;
    el<HTMLInputElement>("f-display").value = d.displayName ?? "";
    el<HTMLSelectElement>("f-api").value = d.api ?? "openai-completions";
    el<HTMLInputElement>("f-base").value = d.baseURL ?? "";
    el<HTMLInputElement>("f-keyenv").value = d.apiKeyEnv ?? "";
    renderModelRows(
      (d.models ?? []).map((m) => ({
        id: (m as ModelEntry).id ?? "",
        name: (m as ModelEntry).name ?? "",
        contextWindow: (m as ModelEntry).contextWindow ?? null,
      })),
    );
  };
  el("add-provider").addEventListener("click", () => openEditor(-1));
  el("editor-cancel").addEventListener("click", closeEditor);
  el("add-model-row").addEventListener("click", () => {
    // 从当前编辑器收集已有行再追加一行空行（保住未确定录入的值）。
    const { models } = collectEditorModels();
    renderModelRows([...models, { id: "", name: "", contextWindow: null }]);
  });
  el("model-rows").addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>("button[data-op='del-model']");
    if (!btn) return;
    const { models } = collectEditorModels();
    models.splice(Number(btn.dataset.i), 1);
    renderModelRows(models);
  });
  el("editor-save").addEventListener("click", () => {
    const { models, error } = collectEditorModels();
    if (error) {
      el("editor-error").textContent = error;
      return;
    }
    const name = el<HTMLInputElement>("f-name").value.trim();
    if (!name || !/^[a-zA-Z0-9_-]+$/.test(name)) {
      el("editor-error").textContent = "路由名必填，仅限字母数字、-、_";
      return;
    }
    if (modelConfig.providers.some((p, i) => i !== editingIndex && p.name === name)) {
      el("editor-error").textContent = `路由名 ${name} 已存在`;
      return;
    }
    const entry: ProviderEntry = {
      name,
      displayName: el<HTMLInputElement>("f-display").value.trim(),
      api: el<HTMLSelectElement>("f-api").value,
      baseURL: el<HTMLInputElement>("f-base").value.trim(),
      apiKeyEnv: el<HTMLInputElement>("f-keyenv").value.trim(),
      models,
    };
    if (editingIndex >= 0) modelConfig.providers[editingIndex] = entry;
    else modelConfig.providers.push(entry);
    if (modelConfig.defaultProvider && !modelConfig.providers.some((p) => p.name === modelConfig.defaultProvider)) {
      modelConfig.defaultProvider = null;
      modelConfig.defaultModel = null;
    }
    closeEditor();
    renderProviders();
    markDirty(true);
  });
  el("provider-list").addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>("button[data-op]");
    if (!btn) return;
    const i = Number(btn.dataset.i);
    if (btn.dataset.op === "edit") openEditor(i);
    else {
      modelConfig.providers.splice(i, 1);
      renderProviders();
      markDirty(true);
    }
  });
  el("save-models").addEventListener("click", async () => {
    el("models-status").textContent = "保存中…";
    try {
      await invoke("set_model_config", { config: JSON.stringify(modelConfig) });
      markDirty(false);
      el("models-status").textContent = "已保存，重启后端后生效";
    } catch (e) {
      el("models-status").textContent = `保存失败：${e}`;
    }
  });
  el("restart-backend-btn").addEventListener("click", async () => {
    el("models-status").textContent = "正在重启后端…";
    try {
      await invoke("restart_backend");
      el("models-status").textContent = "后端重启中，稍后打开主窗口即可";
    } catch (e) {
      el("models-status").textContent = `重启失败：${e}`;
    }
  });
  // 加载已存配置。
  void invoke<string | null>("get_model_config")
    .then((raw) => {
      if (raw) {
        const parsed = JSON.parse(raw) as ModelConfig;
        modelConfig = {
          providers: parsed.providers ?? [],
          defaultProvider: parsed.defaultProvider ?? null,
          defaultModel: parsed.defaultModel ?? null,
        };
      }
    })
    .catch(() => {})
    .finally(() => {
      renderProviders();
      markDirty(false);
    });
}
closeEditor(); // 初始关闭态
initModelsPage();

// ===================== 插件市场 =====================

interface PluginInfo {
  name: string;
  version: string;
}

function renderPlugins(list: PluginInfo[]): void {
  const box = el("plugin-list");
  box.innerHTML = "";
  if (!list.length) {
    box.innerHTML = `<p style="font-size:12px;color:var(--muted);margin:6px 0">还没有安装任何插件。</p>`;
    return;
  }
  list.forEach((p) => {
    const item = document.createElement("div");
    item.className = "provider-item";
    item.innerHTML = `
      <div class="p-main">
        <div class="p-name">${p.name}</div>
        <div class="p-meta">${p.version}</div>
      </div>
      <div class="p-ops"><button data-op="del" data-pkg="${p.name}">卸载</button></div>`;
    box.appendChild(item);
  });
}

function loadPlugins(): void {
  void invoke<PluginInfo[]>("list_plugins")
    .then(renderPlugins)
    .catch((e) => (el("plugins-status").textContent = `读取失败：${e}`));
}

el("install-plugin").addEventListener("click", async () => {
  const pkg = el<HTMLInputElement>("plugin-pkg").value.trim();
  if (!pkg) return;
  const btn = el<HTMLButtonElement>("install-plugin");
  btn.disabled = true;
  el("plugins-status").textContent = `正在安装 ${pkg}（联网执行 pnpm，请稍候）…`;
  try {
    await invoke("install_plugin", { package: pkg });
    el<HTMLInputElement>("plugin-pkg").value = "";
    el("plugins-status").textContent = `已安装 ${pkg}`;
    loadPlugins();
  } catch (e) {
    el("plugins-status").textContent = `安装失败：${e}`;
  } finally {
    btn.disabled = false;
  }
});
el("plugin-list").addEventListener("click", async (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-op='del']");
  if (!btn) return;
  const pkg = btn.dataset.pkg ?? "";
  btn.disabled = true;
  el("plugins-status").textContent = `正在卸载 ${pkg}…`;
  try {
    await invoke("uninstall_plugin", { package: pkg });
    el("plugins-status").textContent = `已卸载 ${pkg}`;
    loadPlugins();
  } catch (err) {
    el("plugins-status").textContent = `卸载失败：${err}`;
    btn.disabled = false;
  }
});
loadPlugins();

// ===================== Skill 与迁移 =====================

interface SkillInfo {
  name: string;
  description: string;
  kind: string;
}

function renderSkills(list: SkillInfo[]): void {
  const box = el("skill-list");
  box.innerHTML = "";
  if (!list.length) {
    box.innerHTML = `<p style="font-size:12px;color:var(--muted);margin:6px 0">还没有 Skill。点"从 Zcode 导入"或手动放入 ~/.dsh/skills。</p>`;
    return;
  }
  list.forEach((s) => {
    const item = document.createElement("div");
    item.className = "provider-item";
    const desc = s.description.length > 80 ? `${s.description.slice(0, 80)}…` : s.description;
    item.innerHTML = `
      <div class="p-main">
        <div class="p-name">${s.name} <span style="font-weight:400;color:var(--muted);font-size:11px">${s.kind === "flat" ? "单文件" : "目录"}</span></div>
        <div class="p-meta">${desc || "（无描述）"}</div>
      </div>
      <div class="p-ops"><button data-op="del" data-name="${s.name}">删除</button></div>`;
    box.appendChild(item);
  });
}

function loadSkills(): void {
  void invoke<SkillInfo[]>("list_skills")
    .then(renderSkills)
    .catch((e) => (el("skills-status").textContent = `读取失败：${e}`));
}

el("skill-list").addEventListener("click", async (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLElement>("button[data-op='del']");
  if (!btn) return;
  const name = btn.dataset.name ?? "";
  try {
    await invoke("delete_skill", { name });
    el("skills-status").textContent = `已删除 ${name}`;
    loadSkills();
  } catch (err) {
    el("skills-status").textContent = `删除失败：${err}`;
  }
});

// ZCode 导入选择器
el("import-zcode-skills").addEventListener("click", () => {
  void invoke<SkillInfo[]>("list_source_skills", { source: "zcode" })
    .then((list) => {
      const box = el("zcode-skill-list");
      box.innerHTML = "";
      if (!list.length) {
        box.innerHTML = `<p style="font-size:12px;color:var(--muted)">未在 ~/.zcode/skills 发现 Skill（未装 Zcode 或没有 skill）。</p>`;
      }
      list.forEach((s) => {
        const label = document.createElement("label");
        label.className = "provider-item";
        label.style.cursor = "pointer";
        const desc = s.description.length > 70 ? `${s.description.slice(0, 70)}…` : s.description;
        label.innerHTML = `
          <input type="checkbox" value="${s.name}" checked style="flex-shrink:0" />
          <div class="p-main">
            <div class="p-name">${s.name}</div>
            <div class="p-meta">${desc || "（无描述）"}</div>
          </div>`;
        box.appendChild(label);
      });
      el("zcode-importer").style.display = "block";
    })
    .catch((e) => (el("skills-status").textContent = `读取 Zcode skill 失败：${e}`));
});
el("cancel-import-skills").addEventListener("click", () => {
  el("zcode-importer").style.display = "none";
});
el("do-import-skills").addEventListener("click", async () => {
  const checked = [...el("zcode-skill-list").querySelectorAll<HTMLInputElement>("input:checked")].map(
    (c) => c.value,
  );
  if (!checked.length) return;
  el("skills-status").textContent = "导入中…";
  try {
    const [imported, skipped] = await invoke<[number, number]>("import_skills", {
      source: "zcode",
      names: checked,
    });
    el("skills-status").textContent = `导入完成：${imported} 个${skipped ? `，跳过已存在 ${skipped} 个` : ""}`;
    el("zcode-importer").style.display = "none";
    loadSkills();
  } catch (e) {
    el("skills-status").textContent = `导入失败：${e}`;
  }
});
loadSkills();

// 迁移工具
el("migrate-agents").addEventListener("click", async () => {
  el("migrate-status").textContent = "迁移中…";
  try {
    const msg = await invoke<string>("import_agents_md");
    el("migrate-status").textContent = msg;
  } catch (e) {
    el("migrate-status").textContent = `失败：${e}`;
  }
});
el("goto-skills-import").addEventListener("click", () => {
  document.querySelector('button[data-page="skills"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  el("import-zcode-skills").click();
});

// ===================== 数据目录 =====================

void invoke<string>("get_otter_settings")
  .then((raw) => {
    const s = JSON.parse(raw) as { dshHome?: string };
    if (s.dshHome) el<HTMLInputElement>("dsh-home-input").value = s.dshHome;
  })
  .catch(() => {});
el("migrate-home").addEventListener("click", async () => {
  const path = el<HTMLInputElement>("dsh-home-input").value.trim();
  if (!path) {
    el("home-status").textContent = "请输入目标目录（如 D:\\dsh-data）";
    return;
  }
  const btn = el<HTMLButtonElement>("migrate-home");
  btn.disabled = true;
  el("home-status").textContent = "迁移中（复制数据并重启后端，可能需要数分钟）…";
  try {
    const msg = await invoke<string>("migrate_dsh_home", { newHome: path });
    el("home-status").textContent = msg;
  } catch (e) {
    el("home-status").textContent = `失败：${e}`;
  } finally {
    btn.disabled = false;
  }
});

// 诊断导出（IPC 命令在 lib.rs）。
document.getElementById("export-diag")?.addEventListener("click", async () => {
  const result = document.getElementById("diag-result");
  if (!result) return;
  result.textContent = "正在导出…";
  try {
    const path = await invoke<string>("export_diagnostics");
    result.textContent = `已导出：${path}`;
  } catch (e) {
    result.textContent = `导出失败：${e}`;
  }
});
