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

// Escapa texto remoto antes de interpolarlo en innerHTML. Los nombres de canal
// (display_name/channel_id) son datos de terceros: sin esto, `<img src=...>` o
// `<style>` se inyectan tal cual.
const escapeHtml = (value) =>
  String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");

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
      <span class="avatar">${escapeHtml(initials(s.display_name, s.channel_id))}</span>
      <span class="roster-row__name">${escapeHtml(displayName(s))}</span>
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
      <td>${escapeHtml(displayName(s))}</td>
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
    const base = `${analysis.total_commenters} personas en ${analysis.total_comments} comentarios.`;
    if (analysis.incomplete) {
      // Resultados parciales (típicamente cuota agotada): mostramos lo traído
      // y avisamos que está incompleto, en vez de descartar el progreso (F4).
      const why = analysis.incomplete_reason ? ` (${analysis.incomplete_reason})` : "";
      setStatus(`Resultados parciales: ${base} Se cortó antes de terminar${why}.`, "error");
    } else {
      setStatus(base);
    }
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

// ---------- Herramientas de creador (F6 temas / F9 metadata) ----------

const tools = {
  corpusRun: $("corpus-run"),
  corpusOut: $("corpus-out"),
  keywords: $("keywords"),
  topics: $("topics"),
  metaRun: $("meta-run"),
  videoIds: $("video-ids"),
  videosOut: $("videos-out"),
  ideasRun: $("ideas-run"),
  ideasOut: $("ideas-out"),
  modal: $("cost-modal"),
  costSummary: $("cost-summary"),
  costBreakdown: $("cost-breakdown"),
  costConfirm: $("cost-confirm"),
  costCancel: $("cost-cancel"),
};

// Texto legible de un CostKind (enum serde con tag externo: { QuotaUnits } / { Money }).
function formatCost(kind) {
  if (kind && "QuotaUnits" in kind) {
    const u = kind.QuotaUnits;
    return u === 0 ? "gratis" : `${u} unidad${u === 1 ? "" : "es"} de cuota`;
  }
  if (kind && "Money" in kind) {
    return `~US$${(kind.Money.usd_micros / 1_000_000).toFixed(4)}`;
  }
  return "costo desconocido";
}

// Gate de costo en la UI (estimar -> confirmar -> ejecutar). Si la operación es
// gratis (requires_confirmation = false) resuelve true sin molestar; si cuesta,
// muestra el modal con el desglose y espera la decisión del usuario.
function confirmCost(estimate) {
  return new Promise((resolve) => {
    if (!estimate.requires_confirmation) return resolve(true);
    tools.costSummary.textContent = `Esta operación cuesta ${formatCost(estimate.kind)}.`;
    tools.costBreakdown.innerHTML = "";
    estimate.breakdown.forEach((line) => {
      const li = document.createElement("li");
      li.textContent = `${line.label} · ${formatCost(line.kind)}`;
      tools.costBreakdown.appendChild(li);
    });
    tools.modal.hidden = false;
    const finish = (ok) => {
      tools.modal.hidden = true;
      tools.costConfirm.onclick = null;
      tools.costCancel.onclick = null;
      resolve(ok);
    };
    tools.costConfirm.onclick = () => finish(true);
    tools.costCancel.onclick = () => finish(false);
  });
}

// Nube de keywords: el tamaño/opacidad del chip escala con el peso tf-idf.
function renderKeywords(keywords) {
  tools.keywords.innerHTML = "";
  const max = Math.max(1e-9, ...keywords.map((k) => k.weight));
  keywords.slice(0, 30).forEach((k) => {
    const t = k.weight / max; // 0..1
    const chip = document.createElement("span");
    chip.className = "chip";
    // El peso se codifica SOLO con el tamaño (a contraste pleno): bajar la
    // opacidad lavaba los términos de menor peso hasta volverlos ilegibles.
    chip.style.fontSize = (12 + t * 10).toFixed(1) + "px";
    chip.textContent = k.term; // textContent: no necesita escapeHtml
    chip.title = `${k.total_count} apariciones · en ${k.document_count} comentarios`;
    tools.keywords.appendChild(chip);
  });
}

function renderTopics(topics) {
  tools.topics.innerHTML = "";
  topics.slice(0, 12).forEach((tp) => {
    const li = document.createElement("li");
    li.className = "topic";
    li.innerHTML = `<span class="topic__terms">${escapeHtml(tp.terms.join(" + "))}</span>
      <span class="topic__count">${tp.document_count}</span>`;
    tools.topics.appendChild(li);
  });
}

async function analyzeCorpus() {
  tools.corpusRun.disabled = true;
  setStatus("Analizando temas de la comunidad…");
  try {
    const insights = await invoke("analyze_corpus");
    if (!insights.document_count) {
      setStatus("Todavía no hay histórico: analizá un canal o video primero.", "error");
      tools.corpusOut.hidden = true;
      return;
    }
    renderKeywords(insights.keywords);
    renderTopics(insights.topics);
    tools.corpusOut.hidden = false;
    setStatus(`Temas de ${insights.document_count} comentarios del histórico.`);
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    tools.corpusRun.disabled = false;
  }
}

function renderVideos(videos) {
  tools.videosOut.innerHTML = "";
  videos.forEach((v) => {
    const li = document.createElement("li");
    li.className = "video";
    const views = v.view_count == null ? "—" : v.view_count.toLocaleString("es-AR");
    li.innerHTML = `
      <span class="video__title">${escapeHtml(v.title)}</span>
      <span class="video__meta">${views} vistas · ${v.tags.length} tags</span>`;
    tools.videosOut.appendChild(li);
  });
  tools.videosOut.hidden = videos.length === 0;
}

async function fetchMeta() {
  const ids = tools.videoIds.value.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
  const apiKey = els.apikey.value.trim();
  if (!ids.length) return setStatus("Pegá al menos un ID de video.", "error");
  if (!apiKey) return setStatus("Falta la API key de YouTube Data v3.", "error");

  tools.metaRun.disabled = true;
  try {
    // estimar -> confirmar -> ejecutar (re-estimado server-side en el backend).
    const estimate = await invoke("estimate_video_meta", { ids });
    if (!(await confirmCost(estimate))) {
      setStatus("Cancelado: no se gastó cuota.");
      return;
    }
    setStatus("Trayendo metadata…");
    const videos = await invoke("fetch_video_meta", { ids, apiKey, confirmed: true });
    renderVideos(videos);
    setStatus(`Metadata de ${videos.length} video${videos.length === 1 ? "" : "s"}.`);
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    tools.metaRun.disabled = false;
  }
}

// ---------- Ideas de video (F7: señales del histórico -> semillas) ----------

// Etiqueta + variante de badge por señal. Índigo (acento) para lo que la
// comunidad pregunta/repite; dorado para lo que pide explícitamente.
const SIGNALS = {
  Question: { label: "Pregunta", mod: "question" },
  Request: { label: "Pedido", mod: "request" },
  RecurringTopic: { label: "Tema recurrente", mod: "topic" },
};

function ideaItem(idea) {
  const li = document.createElement("li");
  li.className = "idea";
  const sig = SIGNALS[idea.signal] || { label: idea.signal, mod: "topic" };
  const n = idea.supporting_comment_ids.length;
  // sample_quotes son datos de terceros: escapeHtml obligatorio.
  const quotes = idea.sample_quotes
    .map((q) => `<blockquote class="idea__quote">${escapeHtml(q)}</blockquote>`)
    .join("");
  li.innerHTML = `
    <header class="idea__head">
      <h4 class="idea__title">${escapeHtml(idea.title_seed)}</h4>
      <span class="idea-tag idea-tag--${sig.mod}">${sig.label}</span>
    </header>
    <p class="idea__meta" title="Score: relevancia estimada de la señal (mayor = más fuerte).">Score ${idea.score} · ${n} comentario${n === 1 ? "" : "s"} de respaldo</p>
    ${quotes}
    <button class="btn btn--ghost idea__refine" type="button" disabled
      title="Se activa en una próxima versión: refina la idea con IA.">Refinar con IA</button>`;
  return li;
}

function renderIdeas(ideas) {
  tools.ideasOut.innerHTML = "";
  // Vienen ordenadas por score desc; limitamos como las otras listas.
  ideas.slice(0, 12).forEach((idea) => tools.ideasOut.appendChild(ideaItem(idea)));
  tools.ideasOut.hidden = ideas.length === 0;
}

async function mineIdeas() {
  tools.ideasRun.disabled = true;
  setStatus("Buscando señales en el histórico…");
  try {
    const ideas = await invoke("mine_ideas");
    if (!ideas.length) {
      setStatus("Todavía no hay señales: analizá un canal o video primero.", "error");
      tools.ideasOut.hidden = true;
      return;
    }
    renderIdeas(ideas);
    setStatus(`${ideas.length} idea${ideas.length === 1 ? "" : "s"} a partir del histórico.`);
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    tools.ideasRun.disabled = false;
  }
}

// ---------- SEO de tu texto (F8: auditoría local, gratis) ----------

const seo = {
  title: $("seo-title"),
  tags: $("seo-tags"),
  desc: $("seo-desc"),
  run: $("seo-run"),
  out: $("seo-out"),
};

// Severidad -> etiqueta ES + variante de color (rojo / dorado / índigo-muted).
const SEVERITIES = {
  Critical: { label: "Crítico", mod: "critical" },
  Warning: { label: "Advertencia", mod: "warning" },
  Info: { label: "Sugerencia", mod: "info" },
};

// Rango del score -> variante de color del medidor (alto índigo / medio dorado /
// bajo rojo). El umbral lo decide el rango, no el backend.
function scoreTier(score) {
  if (score >= 80) return "high";
  if (score >= 50) return "mid";
  return "low";
}

function renderSeo(report) {
  const tier = scoreTier(report.score);
  // Findings: cada uno con su píldora de severidad (escapeHtml en message/area).
  const findings = report.findings
    .map((f) => {
      const sev = SEVERITIES[f.severity] || { label: f.severity, mod: "info" };
      return `
        <li class="seo-finding seo-finding--${sev.mod}">
          <span class="seo-finding__sev">${sev.label}</span>
          <span class="seo-finding__body">
            <span class="seo-finding__area">${escapeHtml(f.area)}</span>
            <span class="seo-finding__msg">${escapeHtml(f.message)}</span>
          </span>
        </li>`;
    })
    .join("");
  const findingsBlock = report.findings.length
    ? `<ul class="seo-findings">${findings}</ul>`
    : `<p class="seo-clean">Sin problemas: tu texto está bien optimizado.</p>`;

  // Keywords de comunidad que faltan: chips (escapeHtml: datos de terceros).
  const missing = report.missing_community_keywords || [];
  const missingBlock = missing.length
    ? `<div class="seo-missing">
         <h4 class="corpus__sub">Keywords de tu comunidad que faltan</h4>
         <div class="cloud">${missing
           .map((k) => `<span class="chip">${escapeHtml(k)}</span>`)
           .join("")}</div>
       </div>`
    : "";

  seo.out.innerHTML = `
    <div class="seo-score seo-score--${tier}">
      <span class="seo-score__num">${report.score}</span>
      <span class="seo-score__unit">/ 100</span>
    </div>
    ${findingsBlock}
    ${missingBlock}`;
  seo.out.hidden = false;
}

async function auditSeo() {
  const title = seo.title.value.trim();
  // Tags: parseo por coma/espacio, sin vacíos (mismo patrón que metadata IDs).
  const tags = seo.tags.value.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
  const description = seo.desc.value.trim();
  if (!title) return setStatus("Escribí al menos un título para auditar.", "error");

  seo.run.disabled = true;
  setStatus("Auditando SEO de tu texto…");
  try {
    const report = await invoke("audit_seo", { input: { title, tags, description } });
    renderSeo(report);
    const n = report.findings.length;
    setStatus(n
      ? `Score ${report.score}/100 · ${n} hallazgo${n === 1 ? "" : "s"}.`
      : `Score ${report.score}/100 · sin problemas.`);
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    seo.run.disabled = false;
  }
}

// ---------- Búsqueda / trending (F10, operación cara: 100u/página) ----------

const search = {
  q: $("search-q"),
  pages: $("search-pages"),
  trending: $("search-trending"),
  run: $("search-run"),
  out: $("search-out"),
  cost: $("search-cost"),
};

// Refleja el costo en vivo según las páginas elegidas (100u c/u): conecta la
// decisión con su consecuencia antes de llegar al modal del gate.
function updateSearchCost() {
  const p = Math.max(1, parseInt(search.pages.value, 10) || 1);
  search.cost.textContent =
    `Caro: ${p * 100} unidades de cuota (${p} página${p === 1 ? "" : "s"}). ` +
    "Vas a ver el costo y confirmar antes de gastar.";
}

function renderHits(hits) {
  search.out.innerHTML = "";
  hits.forEach((h) => {
    const li = document.createElement("li");
    li.className = "video";
    li.innerHTML = `<span class="video__title">${escapeHtml(h.title)}</span>
      <span class="video__meta">${escapeHtml(h.channel_id)} · ${fmtDate(h.published_at)}</span>`;
    search.out.appendChild(li);
  });
  search.out.hidden = hits.length === 0;
}

async function runSearch() {
  const query = search.q.value.trim();
  const apiKey = els.apikey.value.trim();
  const maxPages = Math.max(1, parseInt(search.pages.value, 10) || 1);
  if (!query) return setStatus("Escribí un término de búsqueda.", "error");
  if (!apiKey) return setStatus("Falta la API key de YouTube Data v3.", "error");

  // SearchPlan: serde usa los nombres de campo (snake_case) en structs anidados.
  const plan = { query, trending: search.trending.checked, max_pages: maxPages };
  search.run.disabled = true;
  try {
    const estimate = await invoke("estimate_search", { plan });
    if (!(await confirmCost(estimate))) {
      setStatus("Cancelado: no se gastó cuota.");
      return;
    }
    setStatus("Buscando…");
    const result = await invoke("run_search", { plan, apiKey, confirmed: true });
    renderHits(result.hits);
    const base = `${result.hits.length} resultado${result.hits.length === 1 ? "" : "s"}.`;
    if (result.incomplete) {
      setStatus(`Parcial: ${base} Se cortó por cuota.`, "error");
    } else {
      setStatus(base);
    }
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    search.run.disabled = false;
  }
}

els.form.addEventListener("submit", (e) => {
  e.preventDefault();
  analyzeReal();
});
els.demo.addEventListener("click", analyzeDemo);
tools.corpusRun.addEventListener("click", analyzeCorpus);
tools.metaRun.addEventListener("click", fetchMeta);
tools.ideasRun.addEventListener("click", mineIdeas);
seo.run.addEventListener("click", auditSeo);
search.run.addEventListener("click", runSearch);
search.pages.addEventListener("input", updateSearchCost);
updateSearchCost();

if (!invoke) {
  setStatus("Abrí esta interfaz desde la app de escritorio.", "error");
  els.run.disabled = true;
  els.demo.disabled = true;
  tools.corpusRun.disabled = true;
  tools.metaRun.disabled = true;
  tools.ideasRun.disabled = true;
  seo.run.disabled = true;
  search.run.disabled = true;
}
