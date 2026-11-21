import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import type { PcWork } from "./pcApi";
import { AuthorAvatar } from "./WorkCard";

afterEach(cleanup);

const work = (over: Partial<PcWork>) => ({ id: "w", source: "fanbox", authorId: "creator", authorName: "作者", ...over }) as PcWork;
const noImage = (view: ReturnType<typeof render>) => view.container.querySelector("img.no-profile-image");

it("アイコンを置いていない作者には、デスクトップと同じ「画像なし」の絵を出す", () => {
  // 画像を引けない（ここでは画像の URL が作れない）作者は、pixiv も FANBOX も「画像なし」。
  expect(noImage(render(<AuthorAvatar work={work({})} offline={false} offlineAuthorCover={false} />))?.getAttribute("src")).toBe("/no-profile.png");
  cleanup();
  expect(noImage(render(<AuthorAvatar work={work({ source: "pixiv", authorId: "7" })} offline offlineAuthorCover={false} />))).not.toBeNull();
});

it("作者の分からない作品と、プロフィール画像の無い取得元は人型のまま", () => {
  expect(noImage(render(<AuthorAvatar work={work({ authorId: "" })} offline={false} offlineAuthorCover={false} />))).toBeNull();
  cleanup();
  expect(noImage(render(<AuthorAvatar work={work({ source: "local" })} offline={false} offlineAuthorCover={false} />))).toBeNull();
});
