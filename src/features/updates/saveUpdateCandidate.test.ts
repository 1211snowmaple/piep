import { beforeEach, describe, expect, it, vi } from "vitest";
import { saveUpdateCandidate, type UpdateCandidate, type UpdateCredentials } from "./updateWorkflow";

const downloadApi = vi.hoisted(() => ({
  fetchFanboxPost: vi.fn(),
  fetchPixivNovel: vi.fn(),
  downloadAndSave: vi.fn(),
}));

vi.mock("@/services/downloadApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/downloadApi")>()),
  ...downloadApi,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  getDownloadBySource: vi.fn().mockResolvedValue(null),
}));
vi.mock("@/store", () => ({ store: { get: vi.fn().mockResolvedValue(null), set: vi.fn(), save: vi.fn() } }));

const credentials: UpdateCredentials = { refreshToken: "token", fanboxCookie: "cookie", fanboxUserAgent: "UA" };

function fanboxCandidate(): UpdateCandidate {
  return {
    key: "fanbox:1",
    source: "fanbox",
    sourceId: "1",
    title: "候補の題名",
    subtitle: "作者",
    targetLabel: "作者",
    targetType: "author",
    originalData: {},
    selected: true,
  };
}

/**
 * 更新から保存するときも、Webから保存するときと同じ形に整えてから渡す。
 *
 * Rust 側は `tags: Option<Vec<String>>`、`authorId: String` を待っている。
 * FANBOX の応答はタグを `[{ name }]` の形で返すことがあり、`creatorId` が
 * 数値で来ることもある。取り込みの側にだけ整える処理があって更新の側に
 * 無かったころは、**その形が来た作品だけ自動保存が deserialize で落ちて**
 * いた。同じ整え方を両方が通ることを、ここで押さえておく。
 */
describe("更新の候補を保存するとき", () => {
  beforeEach(() => {
    downloadApi.downloadAndSave.mockReset().mockResolvedValue({ id: 1 });
  });

  it("FANBOXのタグがオブジェクトで来ても、文字列の配列にして渡す", async () => {
    downloadApi.fetchFanboxPost.mockResolvedValue({
      id: "1",
      title: "本当の題名",
      tags: [{ name: "タグA" }, { name: "タグB" }],
      creatorId: "creator",
    });

    await saveUpdateCandidate(fanboxCandidate(), credentials);

    const payload = downloadApi.downloadAndSave.mock.calls[0]?.[0];
    expect(payload.tags).toEqual(["タグA", "タグB"]);
  });

  it("FANBOXの作者IDが数値で来ても、文字列にして渡す", async () => {
    downloadApi.fetchFanboxPost.mockResolvedValue({
      id: "1",
      title: "本当の題名",
      tags: [],
      creatorId: 12345,
    });

    await saveUpdateCandidate(fanboxCandidate(), credentials);

    const payload = downloadApi.downloadAndSave.mock.calls[0]?.[0];
    expect(payload.authorId).toBe("12345");
  });

  it("FANBOXの種別が文字列でなければ、既定の article にする", async () => {
    downloadApi.fetchFanboxPost.mockResolvedValue({
      id: "1",
      title: "本当の題名",
      tags: [],
      creatorId: "creator",
      type: 7,
    });

    await saveUpdateCandidate(fanboxCandidate(), credentials);

    const payload = downloadApi.downloadAndSave.mock.calls[0]?.[0];
    expect(payload.contentType).toBe("article");
  });
});
