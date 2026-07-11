"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const { escapeHtml } = require("./escape-html.js");

test("escapa los 5 caracteres peligrosos de HTML", () => {
  assert.equal(escapeHtml(`& < > " '`), "&amp; &lt; &gt; &quot; &#39;");
});

test("neutraliza un payload de inyeccion via atributo img/onerror", () => {
  const payload = `<img src=x onerror=alert(1)>`;
  const escaped = escapeHtml(payload);
  assert.ok(!escaped.includes("<img"), "no debe sobrevivir el tag <img literal");
  assert.equal(escaped, "&lt;img src=x onerror=alert(1)&gt;");
});

test("neutraliza un payload que intenta cerrar un bloque <style>", () => {
  const payload = `</style><script>alert(1)</script>`;
  const escaped = escapeHtml(payload);
  assert.ok(!escaped.includes("</style>"));
  assert.ok(!escaped.includes("<script>"));
});

test("texto normal (nombre de canal tipico) pasa sin cambios", () => {
  assert.equal(escapeHtml("Damian Sire"), "Damian Sire");
});

test("convierte valores no-string a string antes de escapar", () => {
  assert.equal(escapeHtml(123), "123");
  assert.equal(escapeHtml(null), "null");
  assert.equal(escapeHtml(undefined), "undefined");
});
