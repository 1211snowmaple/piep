/**
 * 保存の画面での、取得元ごとの違い。
 *
 * 画面と保存の流れは取得元によらず一つで、違うのはここに並べた分だけである。
 * 以前は取得元の分岐が画面のあちこちに散っていて、pixiv で足した機能が
 * FANBOX には無いまま残りやすかった。取得元を足すときも、直すのはこの表になる。
 */
import type {
  DownloadTargetKind,
  FanboxPost,
  PixivNovel,
  SidebarDownloadType,
  SidebarItem,
} from "@/features/browser/downloadCandidates";
import { normalizeFanboxPostPayload } from "@/features/browser/downloadMetadata";
import {
  fetchFanboxCreatorPosts,
  fetchFanboxPost,
  fetchPixivNovelByUrl,
  fetchPixivSeriesNovels,
  fetchPixivUserNovels,
} from "@/services/downloadApi";
import type { SaveJobWork } from "@/services/updateJobApi";
import type { SaveSource } from "./saveDraft";

/** URL から全件を取れるページの取り方。`id` は作品・シリーズ・作者・クリエイターの ID。 */
type CandidateFetcher = (id: string, url: string) => Promise<SidebarItem[]>;

export interface SaveSourceProfile {
  /** 開いている一覧から読み取った候補の種類。 */
  pageType: Extract<SidebarDownloadType, "pixiv_page" | "fanbox_page">;
  /** 候補が無いときの案内に出す、対応しているページ。 */
  supportedPages: string;
  /** 作者にあたる人の呼び方。 */
  person: string;
  /** フォロー一覧を読むボタンの名前。 */
  followingAction: string;
  /** URL から全件を取れるページの説明と取り方。 */
  targets: Partial<Record<DownloadTargetKind, { label: string; fetch: CandidateFetcher }>>;
}

const row = (
  id: string | number,
  title: string,
  subtitle: string | undefined,
  originalData: SidebarItem["originalData"],
): SidebarItem => ({ id: String(id), title, subtitle, selected: true, originalData });

const novelRow = (novel: PixivNovel) => row(novel.id, novel.title, novel.user?.name, novel);
const postRow = (post: FanboxPost) => row(post.id, post.title, post.user?.name, post);

export const SAVE_SOURCES: Record<SaveSource, SaveSourceProfile> = {
  pixiv: {
    pageType: "pixiv_page",
    supportedPages: "小説、シリーズ、作者、一覧のページ",
    person: "作者",
    followingAction: "フォローユーザーを読み取る",
    targets: {
      pixiv_single: {
        label: "小説を保存できます",
        fetch: async (_id, url) => {
          const data = await fetchPixivNovelByUrl<PixivNovel>(url);
          return [row(
            data.id ?? data.detail?.id ?? "",
            data.title || data.detail?.title || "題名のない小説",
            data.user?.name || data.detail?.user?.name,
            data,
          )];
        },
      },
      pixiv_series: {
        label: "シリーズ内の小説を選択できます",
        fetch: async (id) => (await fetchPixivSeriesNovels<PixivNovel[]>(id)).map(novelRow),
      },
      pixiv_user: {
        label: "作者の小説を選択できます",
        fetch: async (id) => (await fetchPixivUserNovels<PixivNovel[]>(id)).map(novelRow),
      },
    },
  },
  fanbox: {
    pageType: "fanbox_page",
    supportedPages: "投稿、クリエイター、一覧のページ",
    person: "クリエイター",
    followingAction: "クリエイターを読み取る",
    targets: {
      fanbox_single: {
        label: "投稿を保存できます",
        fetch: async (id) => {
          const data = normalizeFanboxPostPayload<FanboxPost>(await fetchFanboxPost<unknown>(id));
          return [row(id, data.title || "題名のない投稿", data.user?.name, data)];
        },
      },
      fanbox_creator: {
        label: "クリエイターの投稿を選択できます",
        fetch: async (id) => (await fetchFanboxCreatorPosts<FanboxPost[]>(id)).map(postRow),
      },
    },
  },
};

export const SAVE_SOURCE_ORDER: SaveSource[] = ["pixiv", "fanbox"];

/** 開いているページについての一行の説明。 */
export function targetLabel(source: SaveSource, kind: DownloadTargetKind): string {
  return SAVE_SOURCES[source].targets[kind]?.label ?? "作品・シリーズ・作者・一覧のページを開いてください";
}

/** 候補の行を保存の依頼にする。FANBOX は一覧で分かった権限を渡し、取り直しを省く。 */
export function saveJobWork(source: SaveSource, item: SidebarItem): SaveJobWork {
  const work: SaveJobWork = { source, sourceId: item.id, title: item.title };
  const data = item.originalData;
  if (source === "fanbox" && "isRestricted" in data) {
    work.fanboxAccess = {
      isRestricted: data.isRestricted === true,
      feeRequired: data.feeRequired,
      creatorId: data.creatorId,
    };
  }
  return work;
}
