#!/usr/bin/env node
"use strict";

// Sidecar de ingesta. Lo invoca el backend Rust (Tauri) como proceso externo.
//
// Uso:
//   node src/index.js --video  <videoId>
//   node src/index.js --channel <channelId>   (todos los videos del canal)
//
// La API key sale de la env YOUTUBE_KEY_API (la setea Tauri al spawnear).
// Emite por stdout SOLO un JSON { commenters: [...], comments: [...] } y, ante
// error, escribe { error } por stderr y termina con código != 0. (Nada más debe
// ir a stdout: Rust parsea stdout como JSON.)

const { commentsForVideo, commentsForChannel } = require("./youtube");
const { mapComments } = require("./mapper");

function parseArgs(argv) {
  const args = { mode: null, id: null };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--video") {
      args.mode = "video";
      args.id = argv[++i];
    } else if (argv[i] === "--channel") {
      args.mode = "channel";
      args.id = argv[++i];
    }
  }
  return args;
}

async function main() {
  const { mode, id } = parseArgs(process.argv.slice(2));
  if (!mode || !id) {
    throw new Error("uso: --video <id> | --channel <id>");
  }

  const apiKey = process.env.YOUTUBE_KEY_API;
  if (!apiKey) throw new Error("falta YOUTUBE_KEY_API en el entorno");

  const rawComments =
    mode === "video"
      ? await commentsForVideo(apiKey, id)
      : await commentsForChannel(apiKey, id);

  process.stdout.write(JSON.stringify(mapComments(rawComments)));
}

main().catch((err) => {
  process.stderr.write(JSON.stringify({ error: String((err && err.message) || err) }));
  process.exit(1);
});
