"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const { mapComments, toComment } = require("./mapper");

// Forma cruda tal como la produce `youtube.js` (flattenThread) desde la API.
const RAW = [
  {
    id: "c1",
    videoId: "vid1",
    authorChannelId: "ana",
    authorDisplayName: "Ana",
    authorProfileImageUrl: "http://img/ana.png",
    authorChannelUrl: "http://yt/ana",
    textDisplay: "hola",
    likeCount: 5,
    publishedAt: "2021-09-27T03:00:00Z",
  },
  {
    id: "c2",
    videoId: "vid1",
    authorChannelId: "ana",
    authorDisplayName: "Ana",
    textOriginal: "de nuevo",
    likeCount: 0,
    publishedAt: "2021-09-27T04:00:00Z",
  },
  {
    id: "c3",
    videoId: "vid1",
    authorChannelId: "beto",
    authorDisplayName: "Beto",
    textDisplay: "buenisimo",
    publishedAt: "2021-09-27T03:30:00Z",
  },
];

test("mapea un comentario a la forma del core", () => {
  const c = toComment(RAW[0]);
  assert.deepEqual(c, {
    id: "c1",
    video_id: "vid1",
    author_channel_id: "ana",
    text: "hola",
    like_count: 5,
    published_at: "2021-09-27T03:00:00.000Z",
  });
});

test("deduplica comentaristas por channel_id", () => {
  const { commenters, comments } = mapComments(RAW);
  assert.equal(comments.length, 3);
  assert.equal(commenters.length, 2);
  assert.deepEqual(
    commenters.map((c) => c.channel_id).sort(),
    ["ana", "beto"],
  );
});

test("usa textOriginal cuando falta textDisplay y likeCount default 0", () => {
  const { comments } = mapComments(RAW);
  const c2 = comments.find((c) => c.id === "c2");
  assert.equal(c2.text, "de nuevo");
  const c3 = comments.find((c) => c.id === "c3");
  assert.equal(c3.like_count, 0);
});
