import { describe, expect, it } from "vitest";
import { normalizeFanboxPostPayload } from "./downloadMetadata";

describe("normalizeFanboxPostPayload", () => {
  it("unwraps FANBOX response envelopes without unwrapping article bodies", () => {
    const post = { id: "123", title: "投稿", body: { blocks: [{ type: "p", text: "本文" }] } };
    expect(normalizeFanboxPostPayload({ body: { body: post } })).toEqual(post);
    expect(normalizeFanboxPostPayload(post)).toEqual(post);
  });
});
