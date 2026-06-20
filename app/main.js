"use strict";

// Puente con el backend Rust. Con `withGlobalTauri: true`, invoke vive acá.
const invoke = window.__TAURI__?.core?.invoke;

const $ = (id) => document.getElementById(id);
const els = {
  form: $("form"),
  target: $("target"),
  apikey: $("apikey"),
  run: $("run"),
  demo: $("demo"),
  status: $("status"),
  empty: $("empty"),
  results: $("results"),
  statComments: $("stat-comments"),
  statPeople: $("stat-people"),
  spectrum: $("spectrum-bar"),
  spectrumTip: $("spectrum-tip"),
  top: $("top"),
  bottom: $("bottom"),
  rows: $("rows"),
};

function setStatus(msg, kind) {
  els.status.textContent = msg || "";
  if (kind) els.status.dataset.kind = kind;
  else delete els.status.dataset.kind;
}

function busy(on) {
  els.run.disabled = on;
  els.demo.disabled = on;
}

const initials = (name, id) => {
  const base = (name || id || "?").trim();
  const parts = base.split(/\s+/);
  const chars = parts.length > 1 ? parts[0][0] + parts[1][0] : base.slice(0, 2);
  return chars.toUpperCase();
};

const displayName = (s) => s.display_name || s.channel_id;

const fmtDate = (iso) =>
  new Date(iso).toLocaleDateString("es-AR", { day: "2-digit", month: "short", year: "numeric" });

// Rampa índigo -> pálido para el espectro, según posición en el ranking.
function ramp(t) {
  // t en [0,1]: 0 = más activo (índigo intenso), 1 = menos activo (lavanda claro).
  const a = { h: 247, s: 100, l: 65 };
  const b = { h: 250, s: 45, l: 88 };
  const mix = (k) => Math.round(a[k] + (b[k] - a[k]) * t);
  return `hsl(${mix("h")} ${mix("s")}% ${mix("l")}%)`;
}

function renderSpectrum(ranking) {
  els.spectrum.innerHTML = "";
  const max = Math.max(1, ...ranking.map((s) => s.comment_count));
  ranking.forEach((s, i) => {
    const t = ranking.length > 1 ? i / (ranking.length - 1) : 0;
    const cell = document.createElement("div");
    cell.className = "seg-cell";
    cell.style.flexGrow = String(s.comment_count);
    cell.style.height = 38 + (s.comment_count / max) * 100 + "%";
    cell.style.background = i === 0 ? "var(--gold)" : ramp(t);
    const tip = `${displayName(s)} · ${s.comment_count} comentario${s.comment_count === 1 ? "" : "s"}`;
    cell.title = tip;
    cell.addEventListener("mouseenter", () => (els.spectrumTip.textContent = tip));
    els.spectrum.appendChild(cell);
  });
  els.spectrum.addEventListener("mouseleave", () => (els.spectrumTip.innerHTML = "&nbsp;"));
}

function rosterRow(s, rank, champ) {
  const li = document.createElement("li");
  li.className = "roster-row" + (champ ? " roster-row--champ" : "");
  li.innerHTML = `
    <span class="roster-row__rank">${rank}</span>
    <span class="roster-row__id">
      <span class="avatar">${initials(s.display_name, s.channel_id)}</span>
      <span class="roster-row__name">${displayName(s)}</span>
    </span>
    <span class="roster-row__count">${s.comment_count}<small>com.</small></span>`;
  return li;
}

function renderRosters(top, bottom) {
  els.top.innerHTML = "";
  top.forEach((s, i) => els.top.appendChild(rosterRow(s, i + 1, i === 0)));
  els.bottom.innerHTML = "";
  bottom.forEach((s, i) => els.bottom.appendChild(rosterRow(s, "·", false)));
}

function renderTable(ranking) {
  els.rows.innerHTML = "";
  ranking.forEach((s, i) => {
    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td class="num">${i + 1}</td>
      <td>${displayName(s)}</td>
      <td class="num">${s.comment_count}</td>
      <td class="num">${s.total_likes}</td>
      <td>${fmtDate(s.last_seen)}</td>`;
    els.rows.appendChild(tr);
  });
}

function render(analysis) {
  els.statComments.textContent = analysis.total_comments;
  els.statPeople.textContent = analysis.total_commenters;
  renderSpectrum(analysis.ranking);
  renderRosters(analysis.top, analysis.bottom);
  renderTable(analysis.ranking);
  els.empty.hidden = true;
  els.results.hidden = false;
}

async function analyzeReal() {
  const target = els.target.value.trim();
  const apiKey = els.apikey.value.trim();
  const mode = els.form.querySelector('input[name="mode"]:checked').value;

  if (!target) return setStatus("Pegá el ID del canal o del video.", "error");
  if (!apiKey) return setStatus("Falta la API key de YouTube Data v3.", "error");

  busy(true);
  setStatus(mode === "channel" ? "Recorriendo los videos del canal…" : "Trayendo comentarios…");
  try {
    const cmd = mode === "channel" ? "analyze_channel" : "analyze_video";
    const args = mode === "channel" ? { channelId: target, apiKey } : { videoId: target, apiKey };
    const analysis = await invoke(cmd, args);
    render(analysis);
    setStatus(`${analysis.total_commenters} personas en ${analysis.total_comments} comentarios.`);
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    busy(false);
  }
}

async function analyzeDemo() {
  busy(true);
  setStatus("Mostrando datos de ejemplo.");
  try {
    render(await invoke("analyze_demo"));
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    busy(false);
  }
}

els.form.addEventListener("submit", (e) => {
  e.preventDefault();
  analyzeReal();
});
els.demo.addEventListener("click", analyzeDemo);

if (!invoke) {
  setStatus("Abrí esta interfaz desde la app de escritorio.", "error");
  els.run.disabled = true;
  els.demo.disabled = true;
}
