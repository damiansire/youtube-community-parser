"use strict";

// Escapa texto remoto antes de interpolarlo en innerHTML. Los nombres de canal
// (display_name/channel_id), comentarios y datos derivados son datos de
// terceros: sin esto, `<img src=... onerror=...>` o `<style>` se inyectan
// tal cual. Cargado como script clásico antes de main.js (ver index.html) —
// queda como global `escapeHtml`, igual que antes de esta extracción — y
// también exportado como módulo CommonJS para poder testearlo con Node sin
// levantar un navegador (ver escape-html.test.js).
function escapeHtml(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = { escapeHtml };
}
