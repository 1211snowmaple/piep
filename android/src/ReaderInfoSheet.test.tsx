import { clearMocks, mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { PcMetadata } from "./pcApi";
import { ReaderInfoSheet } from "./ReaderInfoSheet";
import { readerMetadataForVersion } from "./readerVersion";
import { adoptShelf } from "./shelf";

vi.mock("./appHistory", () => ({ useAppNavigation: () => ({ canBrowseServer: false }) }));

afterEach(() => {
  cleanup();
  adoptShelf(null);
  clearMocks();
  vi.unstubAllGlobals();
});

it("最新版に表紙がなくても、作品情報は選んだ版の写しの表紙を引く", () => {
  vi.stubGlobal("isTauri", true);
  mockConvertFileSrc("windows");
  adoptShelf({
    paired: true, reachable: true, url: null, deviceName: null, issue: null, capabilities: [],
    shelf: { readerBundle: true, scope: { libraryId: "library", subjectId: "reader", syncEpoch: "1", connectionGeneration: 1 } },
  });
  const metadata: PcMetadata = {
    work: {
      readingScope: { libraryId: "library", subjectId: "reader" }, id: "work", snapshotId: "current", source: "pixiv", sourceId: "1", title: "作品", authorName: "作者",
      authorId: "", contentType: "novel", tags: [], excerpt: null, textLength: 200, assetCount: 0, hasCover: false,
      currentVersion: 2, edited: false, editId: null, favorite: false, favoriteGeneration: "0", seriesTitle: null,
      seriesId: null, sourceCreatedAt: null, savedAt: "",
    },
    versions: [{ snapshotId: "old", version: 1, textLength: 100, savedAt: "" }, { snapshotId: "current", version: 2, textLength: 200, savedAt: "" }],
    people: [], series: [],
  };
  const sheet = (work: PcMetadata["work"], versions: PcMetadata["versions"]) => <ReaderInfoSheet
    work={work} versions={versions} selectedVersion={1} latestEdited={false} tab="info"
    isEdited={false} offline={false} offlineAuthorCover={false} outline={[]} collections={[]}
    currentPage={0} pageCount={1} progress={0} saving={false} saveMessage={null}
    onVersion={vi.fn()} onTabChange={vi.fn()} onClose={vi.fn()} onSave={vi.fn()} onOutline={vi.fn()}
  />;
  const view = render(sheet({ ...metadata.work, hasCover: true }, []));
  expect(view.queryByAltText("作品の表紙")).toBeNull();

  view.rerender(sheet(readerMetadataForVersion(metadata, 1).work, metadata.versions));
  const src = view.getByAltText("作品の表紙").getAttribute("src");
  expect(decodeURIComponent(new URL(src ?? "").pathname)).toBe("/shelf/library/work/old/large");
});
