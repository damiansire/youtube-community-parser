"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const { flattenThread, qs } = require("./youtube");
const { mapComments } = require("./mapper");

// Item tal como lo devuelve commentThreads (part=snippet) de la YouTube Data API v3.
const THREAD = {
  snippet: {
    topLevelComment: {
      id: "Ugx-comment-1",
      snippet: {
        videoId: "vid1",
        textDisplay: "hola",
        textOriginal: "hola",
        authorDisplayName: "Ana",
        authorProfileImageUrl: "http://img/ana.png",
        authorChannelUrl: "http://yt/ana",
        authorChannelId: { value: "UCana" }, // OJO: objeto, no string
        likeCount: 5,
        publishedAt: "2021-09-27T03:00:00Z",
      },
    },
  },
};

test("flattenThread aplana un thread a la forma cruda del mapper", () => {
  assert.deepEqual(flattenThread(THREAD), {
    id: "Ugx-comment-1",
    videoId: "vid1",
    authorChannelId: "UCana",
    authorDisplayName: "Ana",
    authorProfileImageUrl: "http://img/ana.png",
    authorChannelUrl: "http://yt/ana",
    textDisplay: "hola",
    textOriginal: "hola",
    likeCount: 5,
    publishedAt: "2021-09-27T03:00:00Z",
  });
});

test("flattenThread extrae authorChannelId.value (no '[object Object]')", () => {
  assert.equal(flattenThread(THREAD).authorChannelId, "UCana");
});

test("flattenThread -> mapComments produce la forma del core", () => {
  const { commenters, comments } = mapComments([flattenThread(THREAD)]);
  assert.equal(comments.length, 1);
  assert.equal(commenters.length, 1);
  assert.equal(comments[0].author_channel_id, "UCana");
  assert.equal(comments[0].published_at, "2021-09-27T03:00:00.000Z");
  assert.equal(commenters[0].channel_id, "UCana");
});

test("qs saltea vacíos/nulos y encodea", () => {
  assert.equal(qs({ a: "1", b: undefined, c: null, d: "", e: "x y" }), "a=1&e=x%20y");
});
