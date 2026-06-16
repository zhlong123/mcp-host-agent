import { invoke } from "@tauri-apps/api/core";

let roots = [];

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

function renderRoots() {
  const el = document.getElementById("rootsList");
  el.innerHTML = "";
  if (!roots.length) {
    el.innerHTML = '<p class="hint">（未配置 — 允许所有路径，穿透时不安全）</p>';
    return;
  }
  roots.forEach((r, i) => {
    const div = document.createElement("div");
    div.className = "root-item";
    div.innerHTML = `
      <input data-i="${i}" data-f="name" value="${escapeHtml(r.name)}" placeholder="名称" />
      <input data-i="${i}" data-f="path" value="${escapeHtml(r.path)}" placeholder="D:/Projects/foo" />
      <button class="btn secondary" data-pick="${i}">浏览…</button>
      <button class="btn danger" data-del="${i}">删除</button>`;
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

async function loadConfig() {
  const c = await invoke("get_config");
  document.getElementById("port").value = c.port;
  document.getElementById("bind").value = c.bind;
  document.getElementById("token").value = c.token || "";
  roots = (c.roots || []).map((r) => ({
    name: r.name,
    path: typeof r.path === "string" ? r.path : String(r.path),
  }));
  renderRoots();
  return c;
}

async function refresh() {
  try {
    const s = await invoke("get_status");
    document.getElementById("mcpUrl").textContent = s.mcp_url;
    document.getElementById("healthUrl").textContent = s.health_url;
    document.getElementById("statusDetail").textContent = s.detail;
    document.getElementById("configPath").textContent = s.config_path;
    const dot = document.getElementById("statusDot");
    const txt = document.getElementById("statusText");
    if (s.online) {
      dot.classList.add("ok");
      txt.textContent = "ONLINE";
    } else {
      dot.classList.remove("ok");
      txt.textContent = "OFFLINE";
    }
    const logs = await invoke("get_audit_logs");
    document.getElementById("auditLog").textContent =
      logs.join("\n") || "(暂无记录)";
  } catch (e) {
    toast("刷新失败: " + e);
  }
}

async function saveConfig() {
  try {
    collectRoots();
    const c = await invoke("get_config");
    await invoke("save_config", {
      config: {
        port: +document.getElementById("port").value,
        bind: document.getElementById("bind").value,
        token: document.getElementById("token").value || null,
        roots: roots.map((r) => ({ name: r.name, path: r.path })),
        limits: c.limits,
        audit_log: c.audit_log,
        config_path: c.config_path,
      },
    });
    toast("配置已保存");
    await refresh();
  } catch (e) {
    toast("保存失败: " + e);
  }
}

document.getElementById("mcpUrl").onclick = async () => {
  const t = document.getElementById("mcpUrl").textContent;
  if (t && t !== "—") {
    await navigator.clipboard.writeText(t);
    toast("MCP 地址已复制");
  }
};

document.getElementById("btnStart").onclick = async () => {
  try {
    const msg = await invoke("start_server");
    toast(msg);
    setTimeout(refresh, 800);
  } catch (e) {
    toast(String(e));
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

loadConfig().then(refresh);
setInterval(refresh, 4000);
