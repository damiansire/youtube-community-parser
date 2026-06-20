"use strict";

// Traduce la forma cruda de `youtube-fast-api` (getAllComments) a la forma
// que consume el core Rust: { commenters: [...], comments: [...] }.
//
// Es función pura (sin red) para poder testearla con fixtures.

/** @param {object} raw comentario crudo de youtube-fast-api */
function toComment(raw) {
  return {
    id: String(raw.id),
    video_id: String(raw.videoId),
    author_channel_id: String(raw.authorChannelId),
    text: String(raw.textDisplay ?? raw.textOriginal ?? ""),
    like_count: Number(raw.likeCount ?? 0),
    // El core espera RFC3339/ISO-8601 (chrono DateTime<Utc>).
    published_at: new Date(raw.publishedAt).toISOString(),
  };
}

/** @param {object} raw comentario crudo de youtube-fast-api */
function toCommenter(raw) {
  return {
    channel_id: String(raw.authorChannelId),
    display_name: String(raw.authorDisplayName ?? ""),
    profile_image_url: raw.authorProfileImageUrl ?? null,
    channel_url: raw.authorChannelUrl ?? null,
  };
}

/**
 * Mapea una lista de comentarios crudos a la forma del core, deduplicando
 * comentaristas por channel_id (una persona comenta muchas veces).
 * @param {object[]} rawComments
 */
function mapComments(rawComments) {
  const comments = rawComments.map(toComment);

  const byChannel = new Map();
  for (const raw of rawComments) {
    const c = toCommenter(raw);
    if (!byChannel.has(c.channel_id)) byChannel.set(c.channel_id, c);
  }

  return { commenters: [...byChannel.values()], comments };
}

module.exports = { toComment, toCommenter, mapComments };
