"use strict";

// Puente con el backend Rust. Con `withGlobalTauri: true`, invoke vive acá.
const invoke = window.__TAURI__?.core?.invoke;

const $ = (id) => document.getElementById(id);
const els = {
  appRoot: $("app-root"),
  form: $("form"),
  target: $("target"),
  apikey: $("apikey"),
  run: $("run"),
  history: $("history"),
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
  tableScroll: $("table-scroll"),
};

function setStatus(msg, kind) {
  els.status.textContent = msg || "";
  if (kind) els.status.dataset.kind = kind;
  else delete els.status.dataset.kind;
}

function busy(on) {
  els.run.disabled = on;
  els.history.disabled = on;
  els.demo.disabled = on;
}

const initials = (name, id) => {
  const base = (name || id || "?").trim();
  const parts = base.split(/\s+/);
  const chars = parts.length > 1 ? parts[0][0] + parts[1][0] : base.slice(0, 2);
  return chars.toUpperCase();
};

const displayName = (s) => s.display_name || s.channel_id;

// `escapeHtml` vive en lib/escape-html.js (cargado antes que este script en
// index.html) — extraído para poder testearlo con Node sin navegador, ver
// app/lib/escape-html.test.js. Sigue siendo un global, igual que antes.

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
}

// El contenedor #spectrum-bar es persistente entre renders: registrar el
// mouseleave acá (idempotente con `onmouseleave =`) evita acumular un listener
// nuevo por cada análisis, que retenía closures sin techo (auditoría P17).
els.spectrum.onmouseleave = () => (els.spectrumTip.innerHTML = "&nbsp;");

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

// ---------- Padrón completo: tabla virtualizada (auditoría P8) ----------
//
// El ranking llega COMPLETO por IPC y puede tener miles de filas (el histórico
// crece sin techo). Antes se creaba un <tr> por persona, inyectando miles de
// nodos al DOM justo cuando hay más histórico. Acá renderizamos SOLO la ventana
// visible + un buffer, con dos filas espaciadoras (arriba/abajo) que ocupan el
// alto de lo que no se pinta para preservar la barra de scroll.

const vtable = {
  data: [], // ranking completo
  rowH: 0, // alto medido de una fila real (px)
  buffer: 8, // filas extra arriba/abajo para que el scroll no muestre huecos
};

// Construye el <tr> de la persona en la posición `i` del ranking (0-based).
function tableRow(s, i) {
  const tr = document.createElement("tr");
  tr.innerHTML = `
    <td class="num">${i + 1}</td>
    <td>${escapeHtml(displayName(s))}</td>
    <td class="num">${s.comment_count}</td>
    <td class="num">${s.total_likes}</td>
    <td>${fmtDate(s.last_seen)}</td>`;
  return tr;
}

// Fila espaciadora invisible que ocupa `h` px (para empujar el contenido visible
// y mantener la altura total del scroll sin pintar miles de filas).
function spacerRow(h) {
  const tr = document.createElement("tr");
  tr.className = "vrow-spacer";
  tr.setAttribute("aria-hidden", "true");
  tr.innerHTML = `<td colspan="5" style="height:${Math.max(0, h)}px"></td>`;
  return tr;
}

// Pinta la ventana de filas visible según el scroll actual.
function paintWindow() {
  const data = vtable.data;
  const total = data.length;
  if (!vtable.rowH || total === 0) return;

  const viewport = els.tableScroll.clientHeight || 420;
  const scrollTop = els.tableScroll.scrollTop;
  const perView = Math.ceil(viewport / vtable.rowH);

  let start = Math.floor(scrollTop / vtable.rowH) - vtable.buffer;
  start = Math.max(0, start);
  let end = start + perView + vtable.buffer * 2;
  end = Math.min(total, end);

  const frag = document.createDocumentFragment();
  frag.appendChild(spacerRow(start * vtable.rowH));
  for (let i = start; i < end; i++) frag.appendChild(tableRow(data[i], i));
  frag.appendChild(spacerRow((total - end) * vtable.rowH));

  els.rows.replaceChildren(frag);
}

function renderTable(ranking) {
  vtable.data = ranking || [];
  els.tableScroll.scrollTop = 0;

  if (vtable.data.length === 0) {
    els.rows.replaceChildren();
    return;
  }

  // Medimos el alto real de una fila renderizándola una vez (robusto ante cambios
  // de fuente/padding en CSS, sin hardcodear el alto).
  if (!vtable.rowH) {
    const probe = tableRow(vtable.data[0], 0);
    els.rows.replaceChildren(probe);
    vtable.rowH = probe.getBoundingClientRect().height || 45;
  }

  paintWindow();
}

// El listener de scroll se registra UNA vez (el contenedor es persistente entre
// análisis): así no acumulamos handlers por cada render (mismo cuidado que P17).
let vtableScrollWired = false;
function wireVirtualTable() {
  if (vtableScrollWired || !els.tableScroll) return;
  let ticking = false;
  els.tableScroll.addEventListener("scroll", () => {
    if (ticking) return;
    ticking = true;
    requestAnimationFrame(() => {
      ticking = false;
      paintWindow();
    });
  });
  vtableScrollWired = true;
}
wireVirtualTable();

function render(analysis) {
  els.statComments.textContent = analysis.total_comments;
  els.statPeople.textContent = analysis.total_commenters;
  // Mostramos los resultados ANTES de virtualizar la tabla: la virtualización
  // mide el alto real de una fila y el viewport, que valen 0 si el contenedor
  // sigue oculto (auditoría P8).
  els.empty.hidden = true;
  els.results.hidden = false;
  renderSpectrum(analysis.ranking);
  renderRosters(analysis.top, analysis.bottom);
  renderTable(analysis.ranking);
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

// Reanaliza el histórico local ya persistido (SQLite), sin pegarle a la API ni
// gastar cuota: es la razón de ser de sdp-storage (F3). El backend ya expone
// `analyze_history`; acá lo cableamos a la UI reusando `render` (el shape es el
// mismo que analyze_video/channel). Si todavía no hay histórico, avisamos en vez
// de mostrar una vista vacía.
async function analyzeHistory() {
  busy(true);
  setStatus("Leyendo el histórico acumulado…");
  try {
    const analysis = await invoke("analyze_history");
    if (!analysis.total_comments) {
      setStatus("Todavía no hay histórico: analizá un canal o video primero.", "error");
      return;
    }
    render(analysis);
    setStatus(
      `Histórico: ${analysis.total_commenters} personas en ${analysis.total_comments} comentarios acumulados.`,
    );
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
  ideasRefine: $("ideas-refine"),
  aiProvider: $("ai-provider"),
  aiKey: $("ai-key"),
  aiIdeasOut: $("ai-ideas-out"),
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
// gratis (requires_confirmation = false) resuelve { token: null } sin molestar;
// si cuesta, muestra el modal y, al confirmar, resuelve con el token de un solo
// uso que emitió el backend (auditoría P1). Cancelar resuelve { cancelled: true }.
//
// El token NO se inventa en el front: viene del estimate y el backend lo valida
// y consume, así que un `confirmed:true` crudo ya no alcanza para gastar.
function confirmCost(estimate) {
  return new Promise((resolve) => {
    if (!estimate.requires_confirmation) {
      return resolve({ token: estimate.confirmation_token ?? null });
    }
    tools.costSummary.textContent = `Esta operación cuesta ${formatCost(estimate.kind)}.`;
    tools.costBreakdown.innerHTML = "";
    estimate.breakdown.forEach((line) => {
      const li = document.createElement("li");
      li.textContent = `${line.label} · ${formatCost(line.kind)}`;
      tools.costBreakdown.appendChild(li);
    });
    openModal();
    const finish = (result) => {
      closeModal();
      tools.costConfirm.onclick = null;
      tools.costCancel.onclick = null;
      document.removeEventListener("keydown", onKeydown, true);
      resolve(result);
    };
    const onKeydown = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        finish({ cancelled: true });
      } else if (e.key === "Tab") {
        trapTab(e);
      }
    };
    document.addEventListener("keydown", onKeydown, true);
    tools.costConfirm.onclick = () =>
      finish({ token: estimate.confirmation_token ?? null });
    tools.costCancel.onclick = () => finish({ cancelled: true });
  });
}

// --- Accesibilidad del modal del gate de dinero (auditoría P11) ---------------
// Foco inicial al diálogo, focus-trap en Tab/Shift+Tab, Escape para cancelar y
// restauración del foco al cerrar. El único gate antes de gastar US$ debe ser
// operable por teclado y lector de pantalla.
let modalPrevFocus = null;
const focusableInModal = () =>
  Array.from(
    tools.modal.querySelectorAll(
      'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
    )
  ).filter((el) => !el.disabled && el.offsetParent !== null);

function openModal() {
  modalPrevFocus = document.activeElement;
  tools.modal.hidden = false;
  // El resto de la app queda inerte para lectores de pantalla mientras el gate
  // está abierto (no se puede operar el fondo antes de decidir).
  if (els.appRoot) els.appRoot.setAttribute("aria-hidden", "true");
  (focusableInModal()[0] || tools.modal).focus();
}

function closeModal() {
  tools.modal.hidden = true;
  if (els.appRoot) els.appRoot.removeAttribute("aria-hidden");
  if (modalPrevFocus && typeof modalPrevFocus.focus === "function") {
    modalPrevFocus.focus();
  }
  modalPrevFocus = null;
}

function trapTab(e) {
  const items = focusableInModal();
  if (items.length === 0) return;
  const first = items[0];
  const last = items[items.length - 1];
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first.focus();
  }
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
    const decision = await confirmCost(estimate);
    if (decision.cancelled) {
      setStatus("Cancelado: no se gastó cuota.");
      return;
    }
    setStatus("Trayendo metadata…");
    const result = await invoke("fetch_video_meta", {
      ids,
      apiKey,
      confirmationToken: decision.token,
    });
    renderVideos(result.videos);
    const n = result.videos.length;
    const missing = result.missing_ids || [];
    let msg = `Metadata de ${n} video${n === 1 ? "" : "s"}.`;
    if (missing.length) {
      // La API omite IDs inexistentes/privados: lo avisamos en vez de mostrar
      // solo videos.length (auditoría P12).
      msg += ` ${missing.length} ID${missing.length === 1 ? "" : "s"} no encontrado${missing.length === 1 ? "" : "s"}.`;
    }
    setStatus(msg, missing.length ? "error" : undefined);
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
    ${quotes}`;
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

// ---------- Refinar ideas con IA (F12: operación cara, factura en US$) ----------

// Una tarjeta de idea refinada: title destacado, hook como cita, rationale como
// cuerpo, y un badge "IA · {provider}". title/hook/rationale vienen del modelo
// (datos no confiables): escapeHtml obligatorio en los tres.
function aiIdeaItem(idea, provider) {
  const li = document.createElement("li");
  li.className = "ai-idea";
  li.innerHTML = `
    <header class="ai-idea__head">
      <h4 class="ai-idea__title">${escapeHtml(idea.title)}</h4>
      <span class="ai-idea__badge">IA · ${escapeHtml(provider)}</span>
    </header>
    <blockquote class="ai-idea__hook">${escapeHtml(idea.hook)}</blockquote>
    <p class="ai-idea__rationale">${escapeHtml(idea.rationale)}</p>`;
  return li;
}

function renderAiIdeas(ideas, provider) {
  tools.aiIdeasOut.innerHTML = "";
  ideas.forEach((idea) => tools.aiIdeasOut.appendChild(aiIdeaItem(idea, provider)));
  tools.aiIdeasOut.hidden = ideas.length === 0;
}

async function refineIdeasAi() {
  const provider = tools.aiProvider.value;
  const apiKey = tools.aiKey.value.trim();
  if (!apiKey) return setStatus("Pegá la key del proveedor de IA.", "error");

  tools.ideasRefine.disabled = true;
  try {
    // estimar -> confirmar -> ejecutar. El modal ya muestra "~US$X" para Money.
    const estimate = await invoke("estimate_ideas_ai", { provider });
    const decision = await confirmCost(estimate);
    if (decision.cancelled) {
      setStatus("Cancelado: no se gastó dinero.");
      return;
    }
    setStatus("Refinando con IA…");
    const refined = await invoke("refine_ideas_ai", {
      provider,
      apiKey,
      confirmationToken: decision.token,
    });
    if (!refined.length) {
      setStatus("La IA no devolvió ideas; generá ideas heurísticas primero.", "error");
      tools.aiIdeasOut.hidden = true;
      return;
    }
    renderAiIdeas(refined, provider);
    setStatus(`${refined.length} idea${refined.length === 1 ? "" : "s"} refinada${refined.length === 1 ? "" : "s"} con IA.`);
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    // Vuelve al estado según haya key (no re-habilita a ciegas si la vaciaron).
    syncRefineEnabled();
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

// ---------- Benchmark de competidores (F11: comparativa local, gratis) ----------

const bench = {
  mine: $("bench-mine"),
  rivals: $("bench-rivals"),
  run: $("bench-run"),
  out: $("bench-out"),
};

// Tipo de brecha -> etiqueta ES + variante de color. Mismo vocabulario visual
// que las píldoras de severidad/señal: índigo (acento) para lo medible, dorado
// para cadencia, rojo para "sin datos".
const GAP_KINDS = {
  MissingKeywords: { label: "Keywords", mod: "keywords" },
  Cadence: { label: "Cadencia", mod: "cadence" },
  Engagement: { label: "Engagement", mod: "engagement" },
  NoData: { label: "Sin datos", mod: "nodata" },
};

// Promedios enteros (vistas/likes/comentarios): "—" si null, miles con es-AR.
const fmtAvg = (n) => (n == null ? "—" : Math.round(n).toLocaleString("es-AR"));
// Cadencia: un decimal, "—" si null.
const fmtCadence = (n) => (n == null ? "—" : n.toFixed(1));

// Una fila de la tabla por canal. `mine` lleva el realce dorado (como el champ
// del roster) y la etiqueta "vos". channel_id es dato de terceros: escapeHtml.
function benchRow(profile, mine) {
  const tag = mine ? ` <span class="bench-you">vos</span>` : "";
  return `
    <tr${mine ? ' class="bench-mine"' : ""}>
      <td class="bench-chan">${escapeHtml(profile.channel_id)}${tag}</td>
      <td class="num">${profile.video_count}</td>
      <td class="num">${fmtAvg(profile.avg_views)}</td>
      <td class="num">${fmtAvg(profile.avg_likes)}</td>
      <td class="num">${fmtAvg(profile.avg_comments)}</td>
      <td class="num">${fmtCadence(profile.posting_cadence_days)}</td>
    </tr>`;
}

function renderBenchmark(report) {
  // Tabla comparativa: primero "vos", luego competidores. Reusa el estilo .table.
  const rows = [benchRow(report.mine, true)]
    .concat((report.competitors || []).map((c) => benchRow(c, false)))
    .join("");
  const table = `
    <div class="table-wrap">
      <table class="table bench-table">
        <thead>
          <tr>
            <th>Canal</th>
            <th class="num">Videos</th>
            <th class="num">Vistas prom.</th>
            <th class="num">Likes prom.</th>
            <th class="num">Coment. prom.</th>
            <th class="num">Cadencia (días)</th>
          </tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    </div>`;

  // Brechas accionables: una píldora de tipo + competidor + detalle (escapeHtml
  // en competitor_id y detail: datos de terceros).
  const gaps = report.gaps || [];
  const gapsBlock = gaps.length
    ? `<ul class="bench-gaps">${gaps
        .map((g) => {
          const k = GAP_KINDS[g.kind] || { label: g.kind, mod: "nodata" };
          return `
            <li class="bench-gap bench-gap--${k.mod}">
              <span class="bench-gap__kind">${k.label}</span>
              <span class="bench-gap__body">
                <span class="bench-gap__chan">${escapeHtml(g.competitor_id)}</span>
                <span class="bench-gap__detail">${escapeHtml(g.detail)}</span>
              </span>
            </li>`;
        })
        .join("")}</ul>`
    : `<p class="bench-clean">Vas a la par o mejor que tus competidores en lo medido.</p>`;

  bench.out.innerHTML = `
    <h4 class="corpus__sub">Comparativa</h4>
    ${table}
    <h4 class="corpus__sub">Brechas accionables</h4>
    ${gapsBlock}`;
  bench.out.hidden = false;
}

async function runBenchmark() {
  const myId = bench.mine.value.trim();
  // Competidores: parseo por coma/espacio, sin vacíos (mismo patrón que IDs/tags).
  const competitorIds = bench.rivals.value.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
  if (!myId) return setStatus("Pegá el channel ID de tu canal.", "error");
  if (!competitorIds.length) return setStatus("Pegá al menos un competidor.", "error");

  bench.run.disabled = true;
  setStatus("Comparando con la competencia…");
  try {
    const report = await invoke("benchmark_channels", { myId, competitorIds });
    renderBenchmark(report);
    const n = report.gaps ? report.gaps.length : 0;
    setStatus(n
      ? `${n} brecha${n === 1 ? "" : "s"} frente a ${competitorIds.length} competidor${competitorIds.length === 1 ? "" : "es"}.`
      : `A la par o mejor que ${competitorIds.length} competidor${competitorIds.length === 1 ? "" : "es"}.`);
  } catch (err) {
    setStatus(String(err), "error");
  } finally {
    bench.run.disabled = false;
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
    const decision = await confirmCost(estimate);
    if (decision.cancelled) {
      setStatus("Cancelado: no se gastó cuota.");
      return;
    }
    setStatus("Buscando…");
    const result = await invoke("run_search", {
      plan,
      apiKey,
      confirmationToken: decision.token,
    });
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
els.history.addEventListener("click", analyzeHistory);
els.demo.addEventListener("click", analyzeDemo);
tools.corpusRun.addEventListener("click", analyzeCorpus);
tools.metaRun.addEventListener("click", fetchMeta);
tools.ideasRun.addEventListener("click", mineIdeas);
tools.ideasRefine.addEventListener("click", refineIdeasAi);
// El refinado cuesta US$: el botón arranca apagado y se habilita solo cuando hay
// key cargada, para que el estado inicial invite a completar antes de gastar.
const syncRefineEnabled = () => {
  if (invoke) tools.ideasRefine.disabled = tools.aiKey.value.trim() === "";
};
tools.aiKey.addEventListener("input", syncRefineEnabled);
syncRefineEnabled();
seo.run.addEventListener("click", auditSeo);
bench.run.addEventListener("click", runBenchmark);
search.run.addEventListener("click", runSearch);
search.pages.addEventListener("input", updateSearchCost);
updateSearchCost();

if (!invoke) {
  setStatus("Abrí esta interfaz desde la app de escritorio.", "error");
  els.run.disabled = true;
  els.history.disabled = true;
  els.demo.disabled = true;
  tools.corpusRun.disabled = true;
  tools.metaRun.disabled = true;
  tools.ideasRun.disabled = true;
  tools.ideasRefine.disabled = true;
  seo.run.disabled = true;
  bench.run.disabled = true;
  search.run.disabled = true;
}
