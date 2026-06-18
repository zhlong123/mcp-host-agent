import { invoke } from "@tauri-apps/api/core";

let roots = [];

const MIB = 1024 * 1024;

function toast(msg) {
  const el = document.getElementById("toast");
  el.textContent = msg;
  el.classList.add("show");
  setTimeout(() => el.classList.remove("show"), 2800);
}

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;");
}

function bytesToMiB(bytes) {
  return Math.max(1, Math.round(bytes / MIB));
}

function miBToBytes(mib) {
  return Math.max(1, Math.round(Number(mib) * MIB));
}

async function copyText(id) {
  const el = document.getElementById(id);
  const text = el?.textContent?.trim();
  if (!text || text === "—") return;
  await navigator.clipboard.writeText(text);
  toast("已复制到剪贴板");
}

function renderRoots() {
  const el = document.getElementById("rootsList");
  el.innerHTML = "";
  if (!roots.length) {
    el.innerHTML =
      '<p class="empty-hint">未配置沙箱 — 允许所有路径（穿透场景不安全）</p>';
    return;
  }
  roots.forEach((r, i) => {
    const div = document.createElement("div");
    div.className = "root-item";
    div.innerHTML = `
      <input data-i="${i}" data-f="name" value="${escapeHtml(r.name)}" placeholder="名称" aria-label="目录名称" />
      <input data-i="${i}" data-f="path" value="${escapeHtml(r.path)}" placeholder="D:/Projects/foo" aria-label="目录路径" />
      <button class="btn btn-ghost btn-sm" data-pick="${i}" type="button">浏览</button>
      <button class="btn btn-outline btn-sm" data-del="${i}" type="button">删除</button>`;
    el.appendChild(div);
  });

  el.querySelectorAll("[data-pick]").forEach((btn) => {
    btn.onclick = async () => {
      const i = +btn.dataset.pick;
      try {
        const folder = await invoke("pick_folder");
        if (folder) {
          collectRoots();
          roots[i].path = folder;
          renderRoots();
        }
      } catch (e) {
        toast(String(e));
      }
    };
  });

  el.querySelectorAll("[data-del]").forEach((btn) => {
    btn.onclick = () => {
      collectRoots();
      roots.splice(+btn.dataset.del, 1);
      renderRoots();
    };
  });
}

function collectRoots() {
  const inputs = document.querySelectorAll("#rootsList input[data-i]");
  const map = {};
  inputs.forEach((inp) => {
    const i = inp.dataset.i;
    const f = inp.dataset.f;
    if (!map[i]) map[i] = { name: "", path: "" };
    map[i][f] = inp.value;
  });
  roots = Object.values(map).filter((r) => r.name || r.path);
}

function fillLimits(limits) {
  document.getElementById("maxReadMiB").value = bytesToMiB(limits.max_read_bytes);
  document.getElementById("maxWriteMiB").value = bytesToMiB(limits.max_write_bytes);
  document.getElementById("maxListEntries").value = limits.max_list_entries;
  document.getElementById("maxListDepth").value = limits.max_list_depth;
  document.getElementById("maxGitDiffMiB").value = bytesToMiB(limits.max_git_diff_bytes);
  document.getElementById("maxGlobResults").value = limits.max_glob_results ?? 500;
  document.getElementById("maxGrepMatches").value = limits.max_grep_matches ?? 200;
  document.getElementById("maxBashOutputMiB").value = bytesToMiB(
    limits.max_bash_output_bytes ?? 1024 * 1024
  );
  document.getElementById("bashTimeoutSecs").value = limits.bash_timeout_secs ?? 30;
}

function collectLimits() {
  return {
    max_read_bytes: miBToBytes(document.getElementById("maxReadMiB").value),
    max_write_bytes: miBToBytes(document.getElementById("maxWriteMiB").value),
    max_list_entries: +document.getElementById("maxListEntries").value,
    max_list_depth: +document.getElementById("maxListDepth").value,
    max_git_diff_bytes: miBToBytes(document.getElementById("maxGitDiffMiB").value),
    max_glob_results: +document.getElementById("maxGlobResults").value,
    max_grep_matches: +document.getElementById("maxGrepMatches").value,
    max_bash_output_bytes: miBToBytes(document.getElementById("maxBashOutputMiB").value),
    bash_timeout_secs: +document.getElementById("bashTimeoutSecs").value,
  };
}

function collectConfig(c) {
  const publicUrl = document.getElementById("publicMcpUrl").value.trim();
  const auditLog = document.getElementById("auditLogPath").value.trim();
  const activityLog = document.getElementById("activityLogPath").value.trim();
  return {
    port: +document.getElementById("port").value,
    bind: document.getElementById("bind").value,
    token: document.getElementById("token").value || null,
    public_mcp_url: publicUrl || null,
    audit_log: auditLog || null,
    activity_log: activityLog || null,
    allow_bash: document.getElementById("allowBash").checked,
    roots: roots.map((r) => ({ name: r.name, path: r.path })),
    limits: collectLimits(),
    config_path: c.config_path,
  };
}

async function loadConfig() {
  const c = await invoke("get_config");
  document.getElementById("port").value = c.port;
  document.getElementById("bind").value = c.bind;
  document.getElementById("token").value = c.token || "";
  document.getElementById("publicMcpUrl").value = c.public_mcp_url || "";
  document.getElementById("auditLogPath").value = c.audit_log || "";
  document.getElementById("activityLogPath").value = c.activity_log || "";
  document.getElementById("allowBash").checked = !!c.allow_bash;
  fillLimits(c.limits);
  roots = (c.roots || []).map((r) => ({
    name: r.name,
    path: typeof r.path === "string" ? r.path : String(r.path),
  }));
  renderRoots();
  return c;
}

async function refreshTunnel() {
  try {
    const t = await invoke("get_tunnel_status");
    const badge = document.getElementById("tunnelBadge");
    const status = document.getElementById("tunnelStatus");
    const phaseLabel = {
      idle: "未启动",
      starting: "连接中",
      running: "已连接",
      error: "失败",
    };
    badge.textContent = phaseLabel[t.phase] || t.phase;
    badge.classList.toggle("ok", t.phase === "running");
    badge.classList.toggle("err", t.phase === "error");
    status.textContent = t.mcp_url || t.message || "—";
    if (t.phase === "running" && t.mcp_url) {
      document.getElementById("publicMcpUrl").value = t.mcp_url;
    }
  } catch {
    /* ignore */
  }
}

async function refresh() {
  try {
    const s = await invoke("get_status");
    document.getElementById("mcpUrl").textContent = s.mcp_url;
    document.getElementById("localMcpUrl").textContent = s.local_mcp_url;
    document.getElementById("healthUrl").textContent = s.health_url;
    document.getElementById("statusDetail").textContent = s.detail;
    document.getElementById("configPath").textContent = s.config_path;
    const dot = document.getElementById("statusDot");
    const txt = document.getElementById("statusText");
    const badge = document.getElementById("statusBadge");
    if (s.online) {
      dot.classList.add("ok");
      txt.textContent = "Online";
      badge.style.borderColor = "rgba(46, 204, 113, 0.35)";
    } else {
      dot.classList.remove("ok");
      txt.textContent = "Offline";
      badge.style.borderColor = "";
    }
  } catch (e) {
    toast("刷新失败: " + e);
  }
  await refreshTunnel();
}

async function saveConfig() {
  try {
    collectRoots();
    const c = await invoke("get_config");
    await invoke("save_config", { config: collectConfig(c) });
    toast("配置已保存");
    await refresh();
    await loadConfig();
  } catch (e) {
    toast("保存失败: " + e);
  }
}

document.querySelectorAll("[data-copy]").forEach((btn) => {
  btn.addEventListener("click", () => copyText(btn.dataset.copy));
});

document.getElementById("mcpUrl").onclick = () => copyText("mcpUrl");

document.getElementById("btnStart").onclick = async () => {
  try {
    const msg = await invoke("start_server");
    toast(msg);
    setTimeout(refresh, 800);
  } catch (e) {
    toast(String(e));
  }
};

document.getElementById("btnRestart").onclick = async () => {
  const btn = document.getElementById("btnRestart");
  btn.disabled = true;
  try {
    const msg = await invoke("restart_server");
    toast(msg);
    setTimeout(refresh, 1200);
  } catch (e) {
    toast(String(e));
  } finally {
    btn.disabled = false;
  }
};

document.getElementById("btnStop").onclick = async () => {
  try {
    const msg = await invoke("stop_server");
    toast(msg);
    refresh();
  } catch (e) {
    toast(String(e));
  }
};

document.getElementById("btnRefresh").onclick = refresh;
document.getElementById("btnSave").onclick = saveConfig;
document.getElementById("btnAddRoot").onclick = () => {
  collectRoots();
  roots.push({ name: "project" + (roots.length + 1), path: "" });
  renderRoots();
};

document.getElementById("btnTunnelStart").onclick = async () => {
  const btn = document.getElementById("btnTunnelStart");
  btn.disabled = true;
  try {
    collectRoots();
    const c = await invoke("get_config");
    await invoke("save_config", { config: collectConfig(c) });
    const msg = await invoke("start_quick_tunnel");
    toast(msg);
    setTimeout(async () => {
      await refresh();
      await loadConfig();
    }, 800);
  } catch (e) {
    toast(String(e));
  } finally {
    btn.disabled = false;
  }
};

document.getElementById("btnTunnelStop").onclick = async () => {
  try {
    const msg = await invoke("stop_quick_tunnel");
    toast(msg);
    await refresh();
    await refreshTunnel();
  } catch (e) {
    toast(String(e));
  }
};

loadConfig().then(refresh);
setInterval(refresh, 4000);
