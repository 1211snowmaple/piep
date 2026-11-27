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
import { getProvider } from "@/lib/providers";
import {
  fetchFanboxCreatorPosts,
  fetchFanboxPost,
  fetchPixivNovelByUrl,
  fetchPixivSeriesNovels,
  fetchPixivUserNovels,
  type CandidateList,
} from "@/services/downloadApi";
import type { SaveJobWork } from "@/services/updateJobApi";
import type { SaveSource } from "./saveDraft";

/** URL から候補を取る。`id` は作品・シリーズ・作者・クリエイターの ID。 */
type CandidateFetcher = (id: string, url: string) => Promise<CandidateList<SidebarItem>>;

export interface SaveSourceProfile {
  /** 開いている一覧から読み取った候補の種類。 */
  pageType: Extract<SidebarDownloadType, "pixiv_page" | "fanbox_page">;
  /** 候補が無いときの案内に出す、対応しているページ。 */
  supportedPages: string;
  /** 作者にあたる人の呼び方。 */
  person: string;
  /** フォロー一覧を読むボタンの名前。 */
  followingAction: string;
  /** URL から候補を取れるページの説明と取り方。 */
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
          return { truncated: false, items: [row(
            data.id ?? data.detail?.id ?? "",
            data.title || data.detail?.title || "題名のない小説",
            data.user?.name || data.detail?.user?.name,
            data,
          )] };
        },
      },
      pixiv_series: {
        label: "シリーズ内の小説を選択できます",
        fetch: async (id) => {
          const { items, truncated } = await fetchPixivSeriesNovels<PixivNovel>(id);
          return { items: items.map(novelRow), truncated };
        },
      },
      pixiv_user: {
        label: "作者の小説を選択できます",
        fetch: async (id) => {
          const { items, truncated } = await fetchPixivUserNovels<PixivNovel>(id);
          return { items: items.map(novelRow), truncated };
        },
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
          const result = await fetchFanboxPost<unknown>(id);
          const data = normalizeFanboxPostPayload<FanboxPost>(result.data);
          const item = row(id, data.title || "題名のない投稿", data.user?.name, data);
          return { truncated: false, items: [{ ...item, restrictionAccountKey: result.accountKey }] };
        },
      },
      fanbox_creator: {
        label: "クリエイターの投稿を選択できます",
        fetch: async (id) => {
          const { accountKey, data } = await fetchFanboxCreatorPosts<FanboxPost>(id);
          return {
            items: data.items.map((post) => ({ ...postRow(post), restrictionAccountKey: accountKey })),
            truncated: data.truncated,
          };
        },
      },
    },
  },
};

export const SAVE_SOURCE_ORDER: SaveSource[] = ["pixiv", "fanbox"];

/** 開いているページについての一行の説明。 */
export function targetLabel(source: SaveSource, kind: DownloadTargetKind): string {
  return SAVE_SOURCES[source].targets[kind]?.label ?? "作品・シリーズ・作者・一覧のページを開いてください";
}

const historyPageLabels: Record<SidebarDownloadType, string> = {
  pixiv_single: "小説",
  pixiv_series: "シリーズ",
  pixiv_user: "作者",
  pixiv_page: "一覧",
  fanbox_single: "投稿",
  fanbox_creator: "クリエイター",
  fanbox_page: "一覧",
};

export function saveHistoryDetail(source: SaveSource, kind: SidebarDownloadType, id: string | null): string {
  return [getProvider(source).label, [historyPageLabels[kind], id].filter(Boolean).join(" ")].join(" · ");
}

/**
 * 候補の行を保存の依頼にする。FANBOX は一覧で分かった権限を、それを見たアカウントと
 * 一緒に渡す。保存のときに使うアカウントが同じなら、取り直しを省ける。
 */
export function saveJobWork(source: SaveSource, item: SidebarItem): SaveJobWork {
  const work: SaveJobWork = { source, sourceId: item.id, title: item.title };
  const data = item.originalData;
  if (source === "fanbox" && item.restrictionAccountKey && "isRestricted" in data) {
    work.fanboxAccess = {
      isRestricted: data.isRestricted === true,
      accountKey: item.restrictionAccountKey,
      feeRequired: data.feeRequired,
      creatorId: data.creatorId,
    };
  }
  return work;
}
