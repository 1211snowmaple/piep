import { describe, expect, it } from "vitest";
import { activeShelf } from "@/app/libraryShelves";

const shelf = (search: string) => activeShelf("/library", new URLSearchParams(search));

describe("active shelf detection", () => {
  it("recognises each shelf from the library URL", () => {
    expect(shelf("")).toBe("all");
    expect(shelf("favorite=1")).toBe("favorite");
    expect(shelf("shelf=reading")).toBe("reading");
    expect(shelf("revised=1")).toBe("revised");
  });

  it("claims no shelf once the view has been narrowed further", () => {
    // Highlighting "すべての作品" while a search is running would say the
    // sidebar is showing something it is not.
    expect(shelf("q=%E7%89%A9%E8%AA%9E")).toBeNull();
    expect(shelf("saved=3")).toBeNull();
    expect(shelf("favorite=1&watch=watched")).toBeNull();
    expect(shelf("shelf=reading&favorite=1")).toBeNull();
    expect(shelf("watch=unwatched")).toBeNull();
    expect(shelf("revised=1&favorite=1")).toBeNull();
    expect(shelf("shelf=reading&revised=1")).toBeNull();
    // 更新監視は棚を降りて絞り込みになった。絞り込まれた一覧はどの棚でもない。
    expect(shelf("watch=watched")).toBeNull();
    expect(shelf("tab=people")).toBeNull();
  });

  it("ignores parameters that do not change which works are listed", () => {
    expect(shelf("sort=title")).toBe("all");
    expect(shelf("favorite=1&sort=title")).toBe("favorite");
    expect(shelf("tab=works")).toBe("all");
  });

  it("is a library-only idea", () => {
    expect(activeShelf("/epub", new URLSearchParams("favorite=1"))).toBeNull();
    expect(activeShelf("/", new URLSearchParams(""))).toBeNull();
  });
});
