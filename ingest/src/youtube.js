"use strict";

// Cliente directo de la YouTube Data API v3 (sin dependencias: `https` nativo).
//
// Reemplaza a `youtube-fast-api@2.0.2`, que estaba roto: su adapter no exportaba
// las funciones que construyen las URLs (quedaban `undefined` -> "X is not a
// function") y encima logueaba a stdout, contaminando el JSON que parsea Rust.
//
// `flattenThread` es PURO (sin red) -> testeable con fixtures. El resto pagina
// la API real.

const https = require("https");

const API = "https://www.googleapis.com/youtube/v3";

// Timeout por request (ms). Sin esto, una conexión half-open deja la Promise
// sin resolver para siempre: el proceso Node nunca termina y el lado Rust
// (Command::output) queda colgado, congelando la app. Configurable por entorno.
const REQUEST_TIMEOUT_MS = Number(process.env.SDP_HTTP_TIMEOUT_MS) || 15000;

/** Arma un query string, salteando valores vacíos/nulos y encodeando. */
function qs(params) {
  return Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== null && v !== "")
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join("&");
}

/** GET + parse JSON. Rechaza con Error enriquecido (.status, .reason) en HTTP no-2xx. */
function getJson(url) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const fail = (err) => {
      if (settled) return;
      settled = true;
      req.destroy();
      reject(err);
    };

    const req = https.get(url, (res) => {
      const chunks = [];
      res.on("error", fail);
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => {
        if (settled) return;
        settled = true;
        const text = Buffer.concat(chunks).toString();
        let body;
        try {
          body = text ? JSON.parse(text) : {};
        } catch {
          return reject(new Error(`respuesta no-JSON de la API (HTTP ${res.statusCode})`));
        }
        if (res.statusCode < 200 || res.statusCode >= 300) {
          const apiErr = body && body.error;
          const reason = apiErr && apiErr.errors && apiErr.errors[0] && apiErr.errors[0].reason;
          const err = new Error((apiErr && apiErr.message) || `HTTP ${res.statusCode}`);
          err.status = res.statusCode;
          if (reason) err.reason = reason;
          return reject(err);
        }
        resolve(body);
      });
    });

    req.on("error", fail);
    req.setTimeout(REQUEST_TIMEOUT_MS, () => {
      fail(new Error(`timeout de ${REQUEST_TIMEOUT_MS}ms esperando a la YouTube Data API`));
    });
  });
}

/**
 * Aplana un item de `commentThreads` a la forma cruda que consume `mapper.js`.
 * Clave: en la API `authorChannelId` es un objeto `{ value }`, no un string.
 * @param {object} item item de commentThreads
 */
function flattenThread(item) {
  const top = item.snippet.topLevelComment;
  const s = top.snippet;
  return {
    id: top.id,
    videoId: s.videoId,
    authorChannelId: (s.authorChannelId && s.authorChannelId.value) || "",
    authorDisplayName: s.authorDisplayName,
    authorProfileImageUrl: s.authorProfileImageUrl ?? null,
    authorChannelUrl: s.authorChannelUrl ?? null,
    textDisplay: s.textDisplay,
    textOriginal: s.textOriginal,
    likeCount: s.likeCount,
    publishedAt: s.publishedAt,
  };
}

/** Trae TODOS los comentarios top-level de un video (paginado). */
async function commentsForVideo(apiKey, videoId) {
  const out = [];
  let pageToken;
  do {
    const url =
      `${API}/commentThreads?` +
      qs({ key: apiKey, videoId, part: "snippet", maxResults: 100, textFormat: "plainText", pageToken });
    const data = await getJson(url);
    for (const item of data.items ?? []) out.push(flattenThread(item));
    pageToken = data.nextPageToken;
  } while (pageToken);
  return out;
}

/** IDs de video de un canal vía su playlist de uploads (paginado). */
async function videoIdsForChannel(apiKey, channelId) {
  const ch = await getJson(`${API}/channels?` + qs({ key: apiKey, id: channelId, part: "contentDetails" }));
  const uploads =
    ch.items && ch.items[0] && ch.items[0].contentDetails &&
    ch.items[0].contentDetails.relatedPlaylists &&
    ch.items[0].contentDetails.relatedPlaylists.uploads;
  if (!uploads) throw new Error(`canal sin playlist de uploads: ${channelId}`);

  const ids = [];
  let pageToken;
  do {
    const data = await getJson(
      `${API}/playlistItems?` + qs({ key: apiKey, playlistId: uploads, part: "contentDetails", maxResults: 50, pageToken }),
    );
    for (const item of data.items ?? []) ids.push(item.contentDetails.videoId);
    pageToken = data.nextPageToken;
  } while (pageToken);
  return ids;
}

/** Comentarios de todos los videos de un canal. Saltea videos con comentarios deshabilitados. */
async function commentsForChannel(apiKey, channelId) {
  const all = [];
  for (const videoId of await videoIdsForChannel(apiKey, channelId)) {
    try {
      all.push(...(await commentsForVideo(apiKey, videoId)));
    } catch (err) {
      if (err && err.reason === "commentsDisabled") continue;
      throw err;
    }
  }
  return all;
}

module.exports = { qs, getJson, flattenThread, commentsForVideo, videoIdsForChannel, commentsForChannel };
