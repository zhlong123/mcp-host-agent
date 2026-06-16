import { invoke } from "@tauri-apps/api/core";

let lastActivityId = 0;
let filterKey = "";
let lineCounter = 0;
let pollTimer = null;
let programmaticScroll = false;

const streamEl = () => document.getElementById("activityStream");

const KIND_BORDER = {
  create: "border-create",
  modify: "border-modify",
  read: "border-read",
  list: "border-list",
  git: "border-git",
  search: "border-list",
  shell: "border-modify",
  ping: "border-muted",
  error: "border-error",
};

function toast(msg) {
  const el = document.getElementById("toast");
  el.textContent = msg;
  el.classList.add("show");
  setTimeout(() => el.classList.remove("show"), 2400);
}

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function getFilterKey() {
  const kind = document.getElementById("activityKind").value;
  const errorsOnly = document.getElementById("activityErrorsOnly").checked;
  return `${kind}|${errorsOnly}`;
}

function setLiveStatus(live) {
  const dot = document.getElementById("streamLiveDot");
  const txt = document.getElementById("streamLive");
  if (live) {
    dot.classList.add("ok");
    txt.textContent = "跟随中";
  } else {
    dot.classList.remove("ok");
    txt.textContent = autoScrollEnabled() ? "滚底中…" : "已暂停";
  }
}

function updateLiveStatus() {
  setLiveStatus(autoScrollEnabled() && isNearBottom());
}

function autoScrollEnabled() {
  return document.getElementById("activityAutoScroll").checked;
}

function scrollToBottom(force = false) {
  const el = streamEl();
  if (!el) return;
  if (force || autoScrollEnabled()) {
    programmaticScroll = true;
    el.scrollTop = el.scrollHeight;
    requestAnimationFrame(() => {
      programmaticScroll = false;
      updateLiveStatus();
    });
  }
}

function isNearBottom() {
  const el = streamEl();
  return el.scrollHeight - el.scrollTop - el.clientHeight < 48;
}

function clearStream() {
  streamEl().innerHTML = "";
  lineCounter = 0;
}

function removePlaceholder() {
  streamEl().querySelector(".stream-placeholder")?.remove();
}

function appendHtml(html) {
  removePlaceholder();
  streamEl().insertAdjacentHTML("beforeend", html);
}

function nextLineNo() {
  lineCounter += 1;
  return lineCounter;
}

function dotClassFor(kind, ok) {
  if (!ok || kind === "error") return "dot-err";
  if (kind === "create") return "dot-add";
  if (kind === "modify") return "dot-mod";
  if (kind === "git") return "dot-git";
  if (kind === "search") return "dot-list";
  if (kind === "shell") return "dot-mod";
  if (kind === "list") return "dot-list";
  if (kind === "read") return "dot-read";
  return "dot-muted";
}

function decodeB64Preview(b64) {
  try {
    const bin = atob(b64);
    const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    if (text.length > 4096) {
      return `${text.slice(0, 4096)}\n… (${text.length} chars total, truncated)`;
    }
    return text;
  } catch {
    return `[binary · ${Math.floor((b64.length * 3) / 4)} bytes]`;
  }
}

function resolvePreview(ev) {
  const detail = ev.detail || {};
  if (detail.content_preview) return detail.content_preview;
  if (!detail.result_json) return null;
  try {
    const data = JSON.parse(detail.result_json);
    if (ev.tool === "glob" && Array.isArray(data.paths)) {
      return data.paths.join("\n");
    }
    if (ev.tool === "grep" && Array.isArray(data.matches)) {
      return data.matches.map((m) => `${m.path}:${m.line}: ${m.text}`).join("\n");
    }
    if (ev.tool === "bash") {
      let s = `exit: ${data.exit_code ?? "?"}`;
      if (data.stdout) s += `\n--- stdout ---\n${data.stdout}`;
      if (data.stderr) s += `\n--- stderr ---\n${data.stderr}`;
      return s;
    }
    if (ev.tool === "read_file") {
      if (data.content_text) return data.content_text;
      if (data.content_b64) return decodeB64Preview(data.content_b64);
    }
    if (ev.tool === "list_dir" && Array.isArray(data.entries)) {
      return data.entries
        .map((e) => {
          const suffix = e.kind === "dir" ? "/" : "";
          const size = e.kind === "dir" ? "" : `  ${e.size_bytes} B`;
          return `${e.name}${suffix}${size}`;
        })
        .join("\n");
    }
    if (ev.tool === "stat") {
      if (!data.exists) return "exists: false";
      return `kind: ${data.kind}\nsize: ${data.size_bytes} bytes\nmtime_ms: ${data.mtime_unix_ms ?? "-"}`;
    }
    if (ev.tool === "git_diff" && data.diff) {
      return data.diff.length > 4096 ? `${data.diff.slice(0, 4096)}\n… (truncated)` : data.diff;
    }
    if (ev.tool === "ping" && data.roots) {
      return data.roots.join("\n");
    }
  } catch {
    /* ignore */
  }
  return null;
}

function formatListingHtml(text) {
  return text
    .split("\n")
    .map((line) => {
      if (!line || line.startsWith("…")) {
        return `<div class="list-line list-muted">${escapeHtml(line)}</div>`;
      }
      if (line.endsWith("/")) {
        return `<div class="list-line list-dir">${escapeHtml(line)}</div>`;
      }
      return `<div class="list-line list-file">${escapeHtml(line)}</div>`;
    })
    .join("");
}

function renderContentBody(tool, kind, preview) {
  if (!preview) return "";
  const border = KIND_BORDER[kind] || KIND_BORDER.read;
  if (tool === "list_dir") {
    return `<div class="msg-content ${border} msg-listing">${formatListingHtml(preview)}</div>`;
  }
  return `<div class="msg-content ${border}">${escapeHtml(preview)}</div>`;
}

function renderDiffPanel(diff) {
  if (!diff?.preview) return "";
  let html = '<div class="diff-panel">';
  for (const raw of diff.preview.split("\n")) {
    if (!raw && raw !== "") continue;
    if (raw === "…") {
      html += `<div class="diff-ellipsis">…</div>`;
      continue;
    }
    const ln = nextLineNo();
    if (raw.startsWith("+ ")) {
      html += diffRow("add", raw.slice(2), ln);
    } else if (raw.startsWith("- ")) {
      html += diffRow("del", raw.slice(2), ln);
    } else {
      html += diffRow("ctx", raw, ln);
    }
  }
  html += "</div>";
  return html;
}

function diffRow(kind, text, ln) {
  const cls = kind === "add" ? "diff-add" : kind === "del" ? "diff-del" : "diff-ctx";
  const sign = kind === "add" ? "+" : kind === "del" ? "-" : " ";
  return `<div class="diff-row ${cls}">
    <span class="diff-ln">${ln}</span>
    <span class="diff-sign">${sign}</span>
    <span class="diff-code">${escapeHtml(text)}</span>
  </div>`;
}

function formatFileBlock(ev) {
  const kind = ev.kind || "read";
  const path = ev.path && ev.path !== "-" ? ev.path : "";
  const detail = ev.detail || {};
  const diff = detail.diff;
  const ok = ev.ok !== false;
  const ts = ev.ts || "";
  const dur = ev.duration_ms ?? 0;
  const dot = dotClassFor(kind, ok);
  const border = KIND_BORDER[kind] || KIND_BORDER.read;

  const pathHtml = path
    ? `<span class="msg-path" title="${escapeHtml(path)}">${escapeHtml(path)}</span>`
    : "";

  let html = `<section class="stream-block stream-msg" data-id="${ev.id}">`;
  html += `<div class="msg-meta">
    <span class="block-dot ${dot}"></span>
    <span class="msg-tool">${escapeHtml(ev.tool)}</span>
    ${pathHtml}
    <span class="msg-summary">${escapeHtml(ev.summary || "")}</span>
    <span class="msg-time">${escapeHtml(ts)} · ${dur}ms</span>
  </div>`;

  const preview = resolvePreview(ev);
  if (preview) {
    html += renderContentBody(ev.tool, kind, preview);
  } else if (!ok) {
    html += `<div class="msg-content ${KIND_BORDER.error}">${escapeHtml(ev.summary || "failed")}</div>`;
  }

  if (diff?.preview && (kind === "modify" || kind === "create") && ev.tool === "write_file") {
    html += renderDiffPanel(diff);
  }

  if (!ok && ev.summary && preview) {
    html += `<div class="msg-error">${escapeHtml(ev.summary)}</div>`;
  }

  html += "</section>";
  return html;
}

function appendEvents(events) {
  if (!events.length) return;
  const wasBottom = isNearBottom();
  for (const ev of events) {
    appendHtml(formatFileBlock(ev));
    lastActivityId = Math.max(lastActivityId, ev.id);
  }
  if (wasBottom || autoScrollEnabled()) {
    scrollToBottom(true);
  } else {
    updateLiveStatus();
  }
}

function buildQuery(full) {
  return {
    limit: 300,
    after_id: full ? null : lastActivityId || null,
    errors_only: document.getElementById("activityErrorsOnly").checked,
    kind: document.getElementById("activityKind").value || null,
    root_prefix: null,
  };
}

async function refreshActivity(full = false) {
  const key = getFilterKey();
  const filterChanged = key !== filterKey;
  if (full || filterChanged) {
    filterKey = key;
    lastActivityId = 0;
    clearStream();
  }

  try {
    const events = await invoke("get_activity_events", {
      query: buildQuery(full || filterChanged),
    });
    if ((full || filterChanged) && !events.length) {
      streamEl().innerHTML = '<div class="stream-placeholder">— 暂无匹配记录 —</div>';
      return;
    }
    appendEvents(events);
    if (!events.length) updateLiveStatus();
  } catch (e) {
    setLiveStatus(false);
    document.getElementById("streamLive").textContent = "离线";
    document.getElementById("streamLiveDot").classList.remove("ok");
    if (full || filterChanged || !lastActivityId) {
      streamEl().innerHTML = `<div class="stream-placeholder">加载失败: ${escapeHtml(String(e))}</div>`;
    }
  }
}

function setupScrollPause() {
  streamEl().addEventListener(
    "scroll",
    () => {
      if (programmaticScroll) return;
      const chk = document.getElementById("activityAutoScroll");
      if (!isNearBottom()) {
        chk.checked = false;
        updateLiveStatus();
      } else if (chk.checked) {
        updateLiveStatus();
      }
    },
    { passive: true }
  );
}

document.getElementById("btnActivityRefresh").onclick = () => refreshActivity(true);
document.getElementById("btnStreamClear").onclick = () => {
  clearStream();
  lastActivityId = 0;
  streamEl().innerHTML = '<div class="stream-placeholder">已清屏 · 等待新事件…</div>';
};
document.getElementById("activityKind").onchange = () => refreshActivity(true);
document.getElementById("activityErrorsOnly").onchange = () => refreshActivity(true);
document.getElementById("activityAutoScroll").onchange = (e) => {
  if (e.target.checked) {
    scrollToBottom(true);
  } else {
    updateLiveStatus();
  }
};

setupScrollPause();
refreshActivity(true);
pollTimer = setInterval(() => refreshActivity(false), 600);

window.addEventListener("beforeunload", () => clearInterval(pollTimer));
