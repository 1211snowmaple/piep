import { describe, expect, it } from "vitest";
import { normalizeFanboxPostPayload, normalizeFanboxSaveMetadata, normalizePixivSaveMetadata } from "./downloadMetadata";

describe("save metadata normalization", () => {
  it("reads Pixiv tags, caption and publication date from nested detail", () => {
    const metadata = normalizePixivSaveMetadata({ detail: { title: "作品", caption: "概要", create_date: "2026-07-01T12:00:00+09:00", user: { id: 42, name: "作者" }, tags: [{ name: "創作" }, { name: "小説" }] } });
    expect(metadata).toMatchObject({ title: "作品", authorId: "42", tags: ["創作", "小説"], excerpt: "概要", sourceCreatedAt: "2026-07-01T12:00:00+09:00" });
  });

  it("accepts FANBOX snake_case API fields", () => {
    const metadata = normalizeFanboxSaveMetadata({ title: "投稿", creator_id: "alice", published_datetime: "2026-07-02T00:00:00Z", user: { name: "Alice" }, tags: ["制作記録"] });
    expect(metadata).toMatchObject({ authorId: "alice", tags: ["制作記録"], sourceCreatedAt: "2026-07-02T00:00:00Z" });
  });

  it("unwraps FANBOX response envelopes without unwrapping article bodies", () => {
    const post = { id: "123", title: "投稿", body: { blocks: [{ type: "p", text: "本文" }] } };
    expect(normalizeFanboxPostPayload({ body: { body: post } })).toEqual(post);
    expect(normalizeFanboxPostPayload(post)).toEqual(post);
  });
});
