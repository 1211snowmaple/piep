import { useState, useEffect, useMemo } from "react";
import { BookOpen, Edit3 } from "lucide-react";
import { ExportIcon, FileIcon, ImageIcon, FolderIcon, SyncIcon, RefreshIcon, LinkIcon, TrashIcon, BookIcon } from "../icons/Icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Slider } from "@/components/ui/slider";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { startUpdateJob, waitForUpdateJob } from "@/features/updates/updateJobs";
import {
  deleteDownload,
  deleteVersion,
  getAssetUrl,
  getAssets,
  getDownload,
  getDownloadHtml,
  getVersions,
  openLocalAsset,
  readFileContent,
  setFavorite as setFavoriteDb,
  setWatchUpdates as setWatchUpdatesDb,
} from "@/services/dbApi";
import { exportSingle } from "@/services/archiveApi";
import { askDialog, openSingleDialog } from "@/services/dialogApi";
import { openExternalUrl, openFilesystemPath, revealPathInFileManager } from "@/services/openerApi";
import { cn } from "@/lib/utils";

interface DownloadEntry {
  id: number; source: string; sourceId: string; title: string;
  authorName: string; authorId: string; contentType: string;
  tags: string[]; coverPath: string | null; jsonPath: string;
  originalJsonPath: string | null; assetCount: number; fileSizeBytes: number;
  downloadedAt: string; sourceCreatedAt: string | null;
  contentHash: string | null;
  textLength: number;
  sourceUpdatedAt: string | null;
  watchUpdates: boolean;
  currentVersion: number;
  excerpt?: string | null;
  favorite?: boolean;
  personId?: string | null;
  personName?: string | null;
  seriesId?: string | null;
  seriesTitle?: string | null;
}

interface DownloadVersion {
  id: number;
  downloadId: number;
  version: number;
  contentHash: string | null;
  textLength: number;
  jsonPath: string;
  originalJsonPath: string | null;
  assetCount: number;
  fileSizeBytes: number;
  createdAt: string;
  changeSummary: string | null;
}

interface AssetEntry {
  id: number; downloadId: number; assetType: string; filename: string;
  localPath: string; originalUrl: string | null; mimeType: string | null;
  fileSizeBytes: number;
}

interface Props {
  downloadId: number;
  showToast: (msg: string, type: "success" | "error" | "info") => void;
  onSelectTagFilter?: (tag: string) => void;
  onViewPerson?: (source: string, sourceKey: string) => void;
  onViewSeries?: (source: string, sourceKey: string) => void;
  onExportEpub?: (download: DownloadEntry) => void;
  onRead?: () => void;
  onEdit?: () => void;
  isEpubActive?: boolean;
  onNavigateInternalUrl?: (url: string) => void;
  onOpenSourceUrl?: (url: string) => void;
  onDeleted?: () => void;
}

type Tab = "overview" | "json" | "assets" | "text" | "diff";

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#x27;");
}

function formatDateTime(value: string | null | undefined): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString("ja-JP", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function firstString(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) return value;
  }
  return null;
}

function escapeHtmlAttribute(text: string): string {
  return escapeHtml(text).replace(/`/g, "&#x60;");
}

function normalizeAppUrl(url: string): string {
  return url
    .replace(/pixiv:\/\/illusts?\/(\d+)/g, "https://www.pixiv.net/artworks/$1")
    .replace(/pixiv:\/\/novels?\/(\d+)/g, "https://www.pixiv.net/novel/show.php?id=$1")
    .replace(/pixiv:\/\/users?\/(\d+)/g, "https://www.pixiv.net/users/$1");
}

function renderSafeRichText(text: string): string {
  const normalized = normalizeAppUrl(text);
  if (typeof window === "undefined" || typeof DOMParser === "undefined") {
    return escapeHtml(normalized).replace(/\n/g, "<br>");
  }

  const parser = new DOMParser();
  const doc = parser.parseFromString(`<div>${normalized}</div>`, "text/html");
  const root = doc.body.firstElementChild;

  const renderText = (raw: string): string => {
    const parts: string[] = [];
    const urlRegex = /(https?:\/\/[^\s<]+)/g;
    let lastIndex = 0;
    let match: RegExpExecArray | null;

    while ((match = urlRegex.exec(raw)) !== null) {
      const url = match[0];
      parts.push(escapeHtml(raw.slice(lastIndex, match.index)));
      parts.push(`<a href="${escapeHtmlAttribute(url)}" target="_blank" rel="noopener noreferrer">${escapeHtml(url)}</a>`);
      lastIndex = match.index + url.length;
    }

    parts.push(escapeHtml(raw.slice(lastIndex)));
    return parts.join("").replace(/\n/g, "<br>");
  };

  const renderNode = (node: Node): string => {
    if (node.nodeType === Node.TEXT_NODE) {
      return renderText(node.textContent || "");
    }

    if (node.nodeType !== Node.ELEMENT_NODE) return "";
    const element = node as HTMLElement;
    const tagName = element.tagName.toLowerCase();
    const children = Array.from(element.childNodes).map(renderNode).join("");

    if (tagName === "br") return "<br>";
    if (tagName === "strong" || tagName === "b") return `<strong>${children}</strong>`;
    if (tagName === "em" || tagName === "i") return `<em>${children}</em>`;
    if (tagName === "p") return `<p>${children}</p>`;
    if (tagName === "a") {
      const href = normalizeAppUrl(element.getAttribute("href") || "");
      if (/^https?:\/\//.test(href)) {
        const label = element.textContent ? escapeHtml(element.textContent).replace(/\n/g, "<br>") : escapeHtml(href);
        return `<a href="${escapeHtmlAttribute(href)}" target="_blank" rel="noopener noreferrer">${label}</a>`;
      }
      return children;
    }

    return children;
  };

  return root ? Array.from(root.childNodes).map(renderNode).join("") : renderText(normalized);
}

export function ContentViewer({ downloadId, showToast, onSelectTagFilter, onViewPerson, onViewSeries, onExportEpub, onRead, onEdit, isEpubActive, onNavigateInternalUrl, onOpenSourceUrl, onDeleted }: Props) {
  const [dl, setDl] = useState<DownloadEntry | null>(null);
  const [assets, setAssets] = useState<AssetEntry[]>([]);
  const [jsonContent, setJsonContent] = useState("");
  const [textContent, setTextContent] = useState("");
  const [parsedHtml, setParsedHtml] = useState("");
  const [tab, setTab] = useState<Tab>("overview");
  const [selectedImage, setSelectedImage] = useState<string | null>(null);

  const [versions, setVersions] = useState<DownloadVersion[]>([]);
  const [selectedVersion, setSelectedVersion] = useState<number | null>(null);
  const [watchUpdates, setWatchUpdates] = useState(false);
  const [favorite, setFavorite] = useState(false);
  const [isDescriptionExpanded, setIsDescriptionExpanded] = useState(false);

  const handleLinkClick = (e: React.MouseEvent) => {
    const anchor = (e.target as HTMLElement).closest("a");
    if (anchor) {
      const href = anchor.getAttribute("href");
      if (href && href !== "#" && !href.startsWith("javascript:")) {
        e.preventDefault();
        e.stopPropagation();
        
        const isPixiv = href.includes("pixiv.net") || href.startsWith("pixiv://");
        const isFanbox = href.includes("fanbox.cc");
        
        if (isPixiv || isFanbox) {
          let targetUrl = href;
          if (href.startsWith("pixiv://")) {
            targetUrl = href.replace(/pixiv:\/\/illusts?\/(\d+)/g, 'https://www.pixiv.net/artworks/$1')
                           .replace(/pixiv:\/\/novels?\/(\d+)/g, 'https://www.pixiv.net/novel/show.php?id=$1')
                           .replace(/pixiv:\/\/users?\/(\d+)/g, 'https://www.pixiv.net/users/$1');
          }
          
          onNavigateInternalUrl?.(targetUrl);
        } else {
          // Open in system default browser
          openExternalUrl(href).catch(err => {
            console.error("Failed to open external URL in system browser:", err);
          });
        }
      }
    }
  };

  const getSourceUrl = (download: DownloadEntry): string | null => {
    if (download.source === "pixiv") {
      return `https://www.pixiv.net/novel/show.php?id=${download.sourceId}`;
    }
    if (download.source === "fanbox") {
      return download.authorId
        ? `https://${download.authorId}.fanbox.cc/posts/${download.sourceId}`
        : `https://www.fanbox.cc/posts/${download.sourceId}`;
    }
    return null;
  };

  const handleOpenSourceInBrowser = () => {
    if (!dl) return;
    const sourceUrl = getSourceUrl(dl);
    if (!sourceUrl) {
      showToast("保存元ページを開けませんでした", "error");
      return;
    }
    onOpenSourceUrl?.(sourceUrl);
  };

  // 読書表示設定用ステート (Aaパネル用)
  const [theme, setTheme] = useState(() => localStorage.getItem("piep-reader-theme") || "white"); // デフォルト背景は標準 (white)
  const [fontSize, setFontSize] = useState(() => Number(localStorage.getItem("piep-reader-fontSize")) || 17);
  const [fontFamily, setFontFamily] = useState(() => localStorage.getItem("piep-reader-fontFamily") || "serif");
  const [lineHeight, setLineHeight] = useState(() => Number(localStorage.getItem("piep-reader-lineHeight")) || 1.7); // デフォルト行間 1.7
  const [showAaPopup, setShowAaPopup] = useState(false);

  // 改ページ・ページネーション用ステート
  const [currentPage, setCurrentPage] = useState(1);

  const [isCheckingUpdate, setIsCheckingUpdate] = useState(false);
  const [isDeletingVersion, setIsDeletingVersion] = useState(false);

  const reloadMetadata = async (nextSelectedVersion?: number | null) => {
    const [download, assetList, versionList] = await Promise.all([
      getDownload<DownloadEntry>(downloadId),
      getAssets<AssetEntry[]>(downloadId),
      getVersions<DownloadVersion[]>(downloadId),
    ]);
    setDl(download);
    setAssets(assetList);
    setVersions(versionList);
    setWatchUpdates(download.watchUpdates);
    setFavorite(!!download.favorite);
    setSelectedVersion(nextSelectedVersion ?? download.currentVersion);
  };

  const resolvedExcerpt = useMemo(() => {
    let raw = "";
    if (dl?.excerpt) {
      raw = dl.excerpt;
    } else if (jsonContent) {
      try {
        const parsed = JSON.parse(jsonContent);
        if (dl?.source === "pixiv") {
          raw = parsed.detail?.caption || parsed.caption || "";
        } else if (dl?.source === "fanbox") {
          raw = parsed.excerpt || parsed.body?.excerpt || "";
        }
      } catch (e) {
        // Ignore JSON parse errors
      }
    }
    if (raw) return renderSafeRichText(raw);
    return "";
  }, [dl?.excerpt, dl?.source, jsonContent]);

  const sourcePublishedAt = useMemo(() => {
    if (dl?.sourceCreatedAt) return dl.sourceCreatedAt;
    if (!jsonContent) return null;

    try {
      const parsed = JSON.parse(jsonContent);
      if (dl?.source === "pixiv") {
        return firstString(
          parsed.detail?.create_date,
          parsed.detail?.createDate,
          parsed.create_date,
          parsed.createDate
        );
      }
      if (dl?.source === "fanbox") {
        return firstString(
          parsed.publishedDatetime,
          parsed.publishedDateTime,
          parsed.body?.publishedDatetime
        );
      }
    } catch {
      return null;
    }

    return null;
  }, [dl?.source, dl?.sourceCreatedAt, jsonContent]);

  const sourceUpdatedAt = useMemo(() => {
    if (dl?.sourceUpdatedAt) return dl.sourceUpdatedAt;
    if (!jsonContent) return null;

    try {
      const parsed = JSON.parse(jsonContent);
      if (dl?.source === "pixiv") {
        return firstString(
          parsed.detail?.update_date,
          parsed.detail?.updateDate,
          parsed.update_date,
          parsed.updateDate
        );
      }
      if (dl?.source === "fanbox") {
        return firstString(
          parsed.updatedDatetime,
          parsed.updatedDateTime,
          parsed.body?.updatedDatetime
        );
      }
    } catch {
      return null;
    }

    return null;
  }, [dl?.source, dl?.sourceUpdatedAt, jsonContent]);

  useEffect(() => {
    localStorage.setItem("piep-reader-theme", theme);
  }, [theme]);

  useEffect(() => {
    localStorage.setItem("piep-reader-fontSize", String(fontSize));
  }, [fontSize]);

  useEffect(() => {
    localStorage.setItem("piep-reader-fontFamily", fontFamily);
  }, [fontFamily]);

  useEffect(() => {
    localStorage.setItem("piep-reader-lineHeight", String(lineHeight));
  }, [lineHeight]);

  // 1. 基本メタデータとバージョン履歴をロード
  useEffect(() => {
    const loadMetadata = async () => {
      try {
        const [download, assetList, versionList] = await Promise.all([
          getDownload<DownloadEntry>(downloadId),
          getAssets<AssetEntry[]>(downloadId),
          getVersions<DownloadVersion[]>(downloadId),
        ]);
        setDl(download);
        setAssets(assetList);
        setVersions(versionList);
        setWatchUpdates(download.watchUpdates);
        setFavorite(!!download.favorite);
        setSelectedVersion(download.currentVersion);
      } catch (e) {
        console.error("Failed to load metadata:", e);
      }
    };
    loadMetadata();
  }, [downloadId]);

  // 差分（Diff）用ステート
  const [diffBeforeVersion, setDiffBeforeVersion] = useState<number | null>(null);
  const [diffAfterVersion, setDiffAfterVersion] = useState<number | null>(null);
  const [diffBeforeContent, setDiffBeforeContent] = useState("");
  const [diffAfterContent, setDiffAfterContent] = useState("");
  const [loadingDiff, setLoadingDiff] = useState(false);

  // 差分比較のデフォルト初期バージョン設定
  useEffect(() => {
    if (versions.length > 1 && dl) {
      const sorted = [...versions].sort((a, b) => b.version - a.version);
      const latest = sorted[0].version;
      const prev = sorted[1]?.version || latest;
      setDiffAfterVersion(latest);
      setDiffBeforeVersion(prev);
    }
  }, [versions, dl]);

  // 差分表示用の比較元と比較先テキストを非同期ロード
  useEffect(() => {
    if (!dl || diffBeforeVersion === null || diffAfterVersion === null) return;

    const loadDiffTexts = async () => {
      setLoadingDiff(true);
      try {
        const getPathForVersion = (verNum: number) => {
          if (verNum === dl.currentVersion) return dl.jsonPath;
          const v = versions.find(x => x.version === verNum);
          return v ? v.jsonPath : dl.jsonPath;
        };

        const beforeJsonPath = getPathForVersion(diffBeforeVersion);
        const afterJsonPath = getPathForVersion(diffAfterVersion);

        const [beforeJson, afterJson] = await Promise.all([
          readFileContent(beforeJsonPath),
          readFileContent(afterJsonPath)
        ]);

        const extractText = (jsonStr: string) => {
          try {
            const parsed = JSON.parse(jsonStr);
            if (dl.source === "pixiv") {
              return parsed.text || parsed.detail?.text || "";
            } else if (dl.source === "fanbox") {
              if (parsed.body) {
                if (Array.isArray(parsed.body.blocks)) {
                  const textParts = parsed.body.blocks
                    .map((block: any) => {
                      if (block.type === "p" || block.type === "header" || block.type === "paragraph") {
                        return block.text || "";
                      }
                      return "";
                    })
                    .filter((t: string) => t.trim() !== "");
                  return textParts.join("\n\n");
                } else if (typeof parsed.body.text === "string") {
                  return parsed.body.text;
                }
              }
            }
            return "";
          } catch {
            return "";
          }
        };

        setDiffBeforeContent(extractText(beforeJson));
        setDiffAfterContent(extractText(afterJson));
      } catch (e) {
        console.error("Failed to load texts for diff:", e);
      } finally {
        setLoadingDiff(false);
      }
    };

    loadDiffTexts();
  }, [diffBeforeVersion, diffAfterVersion, dl, versions]);

  // タブやバージョンが変更された際にページを1にリセット
  useEffect(() => {
    setCurrentPage(1);
  }, [selectedVersion, tab]);

  // ページ、タブ、バージョン遷移時に小説リーダーのスクロール位置を最上部にリセット（スクロール引きずりバグ修正）
  useEffect(() => {
    const reader = document.querySelector(".text-reader");
    if (reader) {
      reader.scrollTop = 0;
    }
  }, [currentPage, tab, selectedVersion]);

  // html 文字列を改ページアンカーで分割する
  const pages = useMemo(() => {
    if (!parsedHtml) return [];
    
    // 1. 本文内に [newpage] 由来の '<!-- newpage -->' コメントが含まれる場合は、それで正確に分割する (Pixiv標準改ページ)
    if (parsedHtml.includes('<!-- newpage -->')) {
      const parts = parsedHtml.split('<!-- newpage -->');
      return parts.filter(p => p.trim() !== "");
    }
    
    // 2. もし '<!-- newpage -->' がないが、全体の文字数が長い場合（FANBOXや改ページなしの長編小説）
    // HTMLタグ構造を壊さないように段落単位（<div class="parsed-html-text-block"> または <p> ）で美しくスプリット
    if (parsedHtml.length > 6000) {
      // parsed-html-text-block または p タグの開始位置で分割を試みる
      const delimiter = parsedHtml.includes("parsed-html-text-block")
        ? /(?=<div\s+key=[^>]+className="parsed-html-text-block")/gi
        : /(?=<p)/gi;

      const blocks = parsedHtml.split(delimiter);
      const autoPages: string[] = [];
      let currentPageText = "";
      
      // 目安：1ページあたり約3500〜4500文字程度
      const maxCharCount = 4000;

      for (const block of blocks) {
        if (currentPageText.length + block.length > maxCharCount) {
          if (currentPageText) {
            autoPages.push(currentPageText);
            currentPageText = block;
          } else {
            autoPages.push(block);
          }
        } else {
          currentPageText += block;
        }
      }
      if (currentPageText) {
        autoPages.push(currentPageText);
      }
      
      if (autoPages.length > 1) {
        return autoPages;
      }
    }
    
    // それ以外（短編や改ページなし）は1ページとして扱う
    return [parsedHtml];
  }, [parsedHtml]);

  // 2. 選択されたバージョンに基づいて本文、動的HTML、およびオリジナルJSONをロード
  useEffect(() => {
    if (!dl || selectedVersion === null) return;
    
    const loadContent = async () => {
      try {
        // A. オリジナルJSONのパスを特定してロード (JSONタブ用)
        let targetOrigPath = dl.originalJsonPath || dl.jsonPath;
        if (selectedVersion !== dl.currentVersion) {
          const ver = versions.find(v => v.version === selectedVersion);
          if (ver && ver.originalJsonPath) {
            targetOrigPath = ver.originalJsonPath;
          }
        }

        try {
          const rawOrigContent = await readFileContent(targetOrigPath);
          setJsonContent(rawOrigContent);
        } catch (err) {
          console.warn("Failed to load original JSON, falling back to modified data.json:", err);
          // 失敗した場合は加工済み data.json をロード
          let fallbackPath = dl.jsonPath;
          if (selectedVersion !== dl.currentVersion) {
            const ver = versions.find(v => v.version === selectedVersion);
            if (ver) fallbackPath = ver.jsonPath;
          }
          const content = await readFileContent(fallbackPath);
          setJsonContent(content);
        }

        // B. オンザフライで動的HTMLをパース取得 (本文タブ用)
        try {
          const html = await getDownloadHtml(dl.id, selectedVersion);
          setParsedHtml(html);
        } catch (err) {
          console.error("Failed to parse dynamic HTML:", err);
          setParsedHtml("");
        }

        // C. フォールバック用のプレーンテキスト抽出 (data.jsonをソースとする)
        try {
          let targetPath = dl.jsonPath;
          if (selectedVersion !== dl.currentVersion) {
            const ver = versions.find(v => v.version === selectedVersion);
            if (ver) targetPath = ver.jsonPath;
          }
          const content = await readFileContent(targetPath);
          const parsed = JSON.parse(content);
          let text = "";

          if (dl.source === "pixiv") {
            text = parsed.text || parsed.detail?.text || "";
          } else if (dl.source === "fanbox") {
            if (parsed.body) {
              if (Array.isArray(parsed.body.blocks)) {
                const textParts = parsed.body.blocks
                  .map((block: any) => {
                    if (block.type === "p" || block.type === "header" || block.type === "paragraph") {
                      return block.text || "";
                    }
                    return "";
                  })
                  .filter((t: string) => t.trim() !== "");
                text = textParts.join("\n\n");
              } else if (typeof parsed.body.text === "string") {
                text = parsed.body.text;
              }
            }
          }
          setTextContent(text);
        } catch { /* JSONパース失敗 */ }
      } catch (e) {
        console.error("Content load error:", e);
      }
    };
    loadContent();
  }, [selectedVersion, dl, versions]);

  const handleExport = async () => {
    try {
      const dir = await openSingleDialog({
        title: "保存先フォルダを選択",
        directory: true,
      });
      if (!dir) return;
      const outputDir = await exportSingle(downloadId, dir);
      showToast(`保存しました: ${outputDir}`, "success");
    } catch (e: any) { showToast(`ファイル保存エラー: ${e}`, "error"); }
  };

  const handleToggleWatch = async () => {
    if (!dl) return;
    try {
      const nextWatch = !watchUpdates;
      await setWatchUpdatesDb(downloadId, nextWatch);
      setWatchUpdates(nextWatch);
      showToast(nextWatch ? "この作品の更新確認を有効にしました" : "この作品の更新確認を解除しました", "success");
    } catch (e: any) {
      showToast(`確認設定エラー: ${e}`, "error");
    }
  };

  const handleToggleFavorite = async () => {
    if (!dl) return;
    try {
      const nextFav = !favorite;
      await setFavoriteDb(downloadId, nextFav);
      setFavorite(nextFav);
      showToast(nextFav ? "お気に入りに追加しました" : "お気に入りから削除しました", "success");
    } catch (e: any) {
      showToast(`お気に入り設定エラー: ${e}`, "error");
    }
  };

  const handleOpenFolder = async () => {
    if (!dl?.jsonPath) return;
    try {
      const dirPath = dl.jsonPath.replace(/[/\\][^/\\]+$/, "");
      try {
        await revealPathInFileManager(dl.jsonPath);
      } catch {
        await openFilesystemPath(dirPath);
      }
      showToast("ローカルフォルダを開きました", "success");
    } catch (e) {
      console.error("Failed to open local folder:", e);
      showToast(`フォルダを開けませんでした: ${e}`, "error");
    }
  };

  const handleOpenAsset = async (path: string) => {
    try {
      await openLocalAsset(path);
    } catch (e) {
      console.error("Failed to open asset file:", e);
      showToast(`ファイルを開けませんでした: ${e}`, "error");
    }
  };

  const handleCheckUpdate = async () => {
    if (!dl) return;
    setIsCheckingUpdate(true);
    showToast("この作品の更新ジョブを開始します...", "info");
    try {
      const started = await startUpdateJob({ scope: "work", mode: "auto_save", workIds: [dl.id] });
      const completed = await waitForUpdateJob(started.jobId);
      if (completed.status === "completed" && completed.savedCount > 0) {
        showToast("アップデートを保存しました", "success");
        await reloadMetadata();
      } else if (completed.status === "completed") {
        showToast("すでに最新バージョンです（更新はありませんでした）", "success");
      } else if (completed.status === "auth_required") {
        showToast("認証情報が必要です。設定を確認して更新管理から再開してください。", "error");
      } else if (completed.status === "failed") {
        showToast(`更新ジョブで ${completed.errorCount} 件のエラーが発生しました`, "error");
      }
    } catch (e: any) {
      console.error("Update check failed:", e);
      showToast(`更新確認に失敗しました: ${e}`, "error");
    } finally {
      setIsCheckingUpdate(false);
    }
  };

  const handleDeleteSelectedVersion = async () => {
    if (!dl || selectedVersion === null) return;

    const deletingWork = versions.length <= 1;
    const isCurrent = selectedVersion === dl.currentVersion;
    const message = deletingWork
      ? `「${dl.title}」を完全に削除します。保存済みJSONとアセットファイルも削除されます。`
      : `v${selectedVersion} を削除します。保存済みJSONとアセットフォルダも削除されます。${isCurrent ? "\n最新バージョンを削除するため、直前のバージョンを最新として復元します。" : ""}`;
    const ok = await askDialog(message, {
      title: deletingWork ? "作品削除の確認" : "バージョン削除の確認",
      kind: "warning",
      okLabel: "削除する",
      cancelLabel: "キャンセル"
    });
    if (!ok) return;

    setIsDeletingVersion(true);
    try {
      if (deletingWork) {
        await deleteDownload(dl.id);
        showToast("作品を削除しました", "success");
        onDeleted?.();
      } else {
        await deleteVersion(dl.id, selectedVersion);
        showToast(`v${selectedVersion} を削除しました`, "success");
        await reloadMetadata();
      }
    } catch (e: any) {
      showToast(`削除に失敗しました: ${e}`, "error");
    } finally {
      setIsDeletingVersion(false);
    }
  };

  if (!dl) return <div className="content-viewer-loading"><div className="spinner" /></div>;

  const tags = Array.isArray(dl.tags) ? dl.tags : [];
  const imageAssets = assets.filter(a => a.mimeType?.startsWith("image/"));
  const fileAssets = assets.filter(a => !a.mimeType?.startsWith("image/"));

  return (
    <div className="content-viewer">
      {/* Header */}
      <div className="viewer-header">
        <div className="viewer-actions">
          <div className="viewer-action-group version-update-group">
            {versions.length > 0 && (
              <div className="version-selector">
                <span className="text-xs text-muted-foreground">Ver</span>
                <Select
                  value={String(selectedVersion || dl.currentVersion)}
                  onValueChange={(value) => setSelectedVersion(Number(value))}
                >
                  <SelectTrigger className="version-select-box h-8 w-auto min-w-40">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                  {versions.map(v => (
                    <SelectItem key={v.id} value={String(v.version)}>
                      v{v.version} ({new Date(v.createdAt).toLocaleDateString("ja-JP")}) {v.version === dl.currentVersion ? "(最新)" : ""}
                    </SelectItem>
                  ))}
                  </SelectContent>
                </Select>
              </div>
            )}
            <Button
              type="button"
              variant="outline"
              size="icon"
              onClick={handleCheckUpdate}
              disabled={isCheckingUpdate}
              title="最新情報を取得して更新確認を行います"
            >
              {isCheckingUpdate ? (
                <span className="spinner-mini inline-block" />
              ) : (
                <RefreshIcon />
              )}
            </Button>
            <Button
              type="button"
              variant="outline"
              size="icon"
              onClick={handleOpenSourceInBrowser}
              title="保存元ページをアプリ内ブラウザで開きます"
              disabled={!onOpenSourceUrl}
            >
              <LinkIcon />
            </Button>
          </div>
          <div className="viewer-action-group file-action-group">
            <Button
              type="button"
              variant="outline"
              size="icon"
              onClick={handleDeleteSelectedVersion}
              disabled={isDeletingVersion || selectedVersion === null}
              title={versions.length <= 1 ? "作品を完全に削除します" : "閲覧中のバージョンを削除します"}
            >
              {isDeletingVersion ? (
                <span className="spinner-mini inline-block" />
              ) : (
                <TrashIcon />
              )}
            </Button>
            <Button
              type="button"
              variant="outline"
              size="icon"
              onClick={handleOpenFolder}
              title="保存先のフォルダを開きます"
            >
              <FolderIcon />
            </Button>
            <Button
              type="button"
              variant="outline"
              size="icon"
              onClick={handleExport}
              title="この作品のテキストや画像などの物理ファイルを指定したフォルダに保存します"
            >
              <ExportIcon />
            </Button>
          </div>
          {onExportEpub && (
            <div className="viewer-action-group epub-action-group">
              <Button
                type="button"
                variant="secondary"
                size="icon"
                onClick={onRead}
                title="読書に特化した画面で開きます"
                disabled={!onRead}
              >
                <BookOpen size={17} />
              </Button>
              <Button
                type="button"
                variant="secondary"
                size="icon"
                onClick={onEdit}
                title="本文や挿絵を編集します"
                disabled={!onEdit}
              >
                <Edit3 size={17} />
              </Button>
              <Button
                type="button"
                variant={isEpubActive ? "default" : "secondary"}
                size="icon"
                className={cn(isEpubActive && "shadow-md")}
                onClick={() => onExportEpub(dl)}
                title={isEpubActive ? "EPUB設定サイドバーを閉じます" : "EPUB設定サイドバーを開きます"}
              >
                <BookIcon />
              </Button>
            </div>
          )}
        </div>
      </div>

      {/* Hero / Cover & Metadata Area */}
      <div className={`viewer-hero-area ${dl.coverPath ? "has-cover" : "no-cover"}`}>
        {dl.coverPath && (
          <div className="viewer-cover-wrapper">
            <div className="viewer-cover-container" onClick={() => setSelectedImage(dl.coverPath)}>
              <LazyImage localPath={dl.coverPath} alt={dl.title} className="viewer-cover-image" />
            </div>
            
            <div className="viewer-cover-badges-row">
              {/* Watch updates badge */}
              <div 
                className={`card-watch-badge-wrapper ${watchUpdates ? "active" : ""}`}
                onClick={(e) => {
                  e.stopPropagation();
                  handleToggleWatch();
                }}
                title={watchUpdates ? "更新確認を解除する" : "更新確認を有効にする"}
              >
                <SyncIcon active={watchUpdates} className={!watchUpdates ? "update-muted-icon" : ""} />
              </div>

              {/* Favorite badge */}
              <div 
                className={`card-favorite-badge-wrapper ${favorite ? "active" : ""}`}
                onClick={(e) => {
                  e.stopPropagation();
                  handleToggleFavorite();
                }}
                title={favorite ? "お気に入り解除" : "お気に入りに追加"}
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
                </svg>
              </div>
            </div>
          </div>
        )}
        <div className="viewer-metadata-container">
          <Badge className={cn("source-tag", dl.source)}>{dl.source === "pixiv" ? "Pixiv" : "FANBOX"}</Badge>
          {dl.seriesId && dl.seriesTitle && (
            <Button
              type="button"
              variant="link"
              size="sm"
              className="viewer-series-link"
              onClick={() => onViewSeries?.(dl.source, dl.seriesId!)}
              title="シリーズページを開く"
            >
              {dl.seriesTitle}
            </Button>
          )}
          <h2>{dl.title}</h2>
          <p 
            className="viewer-author clickable-meta"
            onClick={() => onViewPerson?.(dl.source, dl.personId || dl.authorId)}
            title="作者ページを開く"
          >
            {dl.authorName}
          </p>
          {tags.length > 0 && (
            <div className="tag-list">
              {tags.map((t, i) => (
                <Badge
                  key={i} 
                  variant="outline"
                  className="tag clickable-meta"
                  onClick={() => onSelectTagFilter?.(t)}
                  title={`タグ #${t} でライブラリを検索`}
                >
                  #{t}
                </Badge>
              ))}
            </div>
          )}
          
          {/* Novel Description / Caption HTML under Tags */}
          {resolvedExcerpt && (
            <div className="viewer-description-wrapper" onClick={handleLinkClick}>
              <div 
                className={`viewer-description-box ${isDescriptionExpanded ? "expanded" : "collapsed"}`}
                dangerouslySetInnerHTML={{ __html: resolvedExcerpt }}
              />
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="description-toggle-btn"
                onClick={() => setIsDescriptionExpanded(!isDescriptionExpanded)}
              >
                {isDescriptionExpanded ? "閉じる ▲" : "続きを読む ▼"}
              </Button>
            </div>
          )}
        </div>
      </div>

      {/* Tabs */}
      <Tabs value={tab} onValueChange={value => setTab(value as Tab)}>
        <TabsList className="viewer-tabs">
          <TabsTrigger value="overview">概要</TabsTrigger>
          {textContent && <TabsTrigger value="text">本文</TabsTrigger>}
          <TabsTrigger value="assets">アセット ({assets.length})</TabsTrigger>
          <TabsTrigger value="json">JSON</TabsTrigger>
          <TabsTrigger value="diff">差分</TabsTrigger>
        </TabsList>
      </Tabs>

      {/* Tab content */}
      <div className={`viewer-content tab-${tab} ${
        tab === "text" || tab === "json" ? "scroll-contained" : "scroll-allowed"
      }`}>
        {tab === "overview" && (
          <div className="overview-grid">
            <Card className="info-table">
              <div className="info-row"><span>ソース</span><span>{dl.source}</span></div>
              <div className="info-row"><span>ID</span><span>{dl.sourceId}</span></div>
              <div className="info-row">
                <span>著者</span>
                <span 
                  className="clickable-meta-link"
                  onClick={() => onViewPerson?.(dl.source, dl.personId || dl.authorId)}
                  title="作者ページを開く"
                >
                  {dl.authorName} (ID: {dl.authorId})
                </span>
              </div>
              {dl.seriesId && dl.seriesTitle && (
                <div className="info-row">
                  <span>シリーズ</span>
                  <span
                    className="clickable-meta-link"
                    onClick={() => onViewSeries?.(dl.source, dl.seriesId!)}
                    title="シリーズページを開く"
                  >
                    {dl.seriesTitle} (ID: {dl.seriesId})
                  </span>
                </div>
              )}
              <div className="info-row"><span>種類</span><span>{dl.contentType}</span></div>
              <div className="info-row"><span>アセット数</span><span>{dl.assetCount}</span></div>
              <div className="info-row"><span>最新バージョン</span><span>v{dl.currentVersion}</span></div>
              <div className="info-row"><span>閲覧中バージョン</span><span>v{selectedVersion}</span></div>
              <div className="info-row"><span>本文文字数</span><span>{dl.textLength ? `${dl.textLength.toLocaleString()} 文字` : "不明"}</span></div>
              <div className="info-row"><span>コンテンツハッシュ</span><span className="break-all font-mono text-[11px]">{dl.contentHash || "未算出"}</span></div>
              <div className="info-row"><span>ダウンロード日</span><span>{formatDateTime(dl.downloadedAt)}</span></div>
              {sourcePublishedAt && <div className="info-row"><span>投稿日</span><span>{formatDateTime(sourcePublishedAt)}</span></div>}
              {sourceUpdatedAt && <div className="info-row"><span>更新日</span><span>{formatDateTime(sourceUpdatedAt)}</span></div>}
            </Card>
          </div>
        )}

        {tab === "text" && (
          <div className="reader-tab-container">
            <div 
              className={`text-reader reader-theme-${theme} reader-font-${fontFamily}`} 
              style={{ fontSize: `${fontSize}px`, lineHeight: lineHeight }}
            >
              {/* 本文プレビュー領域 */}
              <div className="novel-text" onClick={handleLinkClick}>
                {pages.length > 0 ? (
                  <ParsedHtmlRenderer 
                    html={pages[currentPage - 1] || ""} 
                    onJump={(p) => setCurrentPage(Math.min(pages.length, Math.max(1, p)))}
                    onLinkClick={handleLinkClick}
                  />
                ) : (
                  textContent.split("\n").map((line, i) => <p key={i}>{line || "\u00A0"}</p>)
                )}
              </div>
            </div>

            {/* 枠の下側に綺麗に配置されるページネーション＆Aaトグル統合コントロール */}
            <div className="reader-pagination-container">
              {/* 左側の均衡用スペーサー */}
              <div className="pagination-left-spacer" />

              {/* 中央のページネーション (2ページ以上あるときのみ) */}
              <div className="reader-pagination">
                {pages.length > 1 ? (
                  <>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      className="pagination-btn"
                      disabled={currentPage === 1}
                      onClick={() => {
                        setCurrentPage(p => Math.max(1, p - 1));
                      }}
                    >
                      前へ
                    </Button>
                    
                    {Array.from({ length: pages.length }).map((_, idx) => {
                      const pNum = idx + 1;
                      if (pNum === 1 || pNum === pages.length || Math.abs(pNum - currentPage) <= 2) {
                        return (
                          <Button
                            type="button"
                            variant={currentPage === pNum ? "secondary" : "outline"}
                            size="sm"
                            key={pNum}
                            className={cn("pagination-btn", currentPage === pNum && "active")}
                            onClick={() => {
                              setCurrentPage(pNum);
                            }}
                          >
                            {pNum}
                          </Button>
                        );
                      } else if (pNum === 2 || pNum === pages.length - 1) {
                        return <span key={`ellipsis-${pNum}`} className="pagination-ellipsis">...</span>;
                      }
                      return null;
                    })}

                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      className="pagination-btn"
                      disabled={currentPage === pages.length}
                      onClick={() => {
                        setCurrentPage(p => Math.min(pages.length, p + 1));
                      }}
                    >
                      次へ
                    </Button>
                  </>
                ) : (
                  <span className="pagination-single-page">1 / 1 ページ</span>
                )}
              </div>

              {/* 右側のAa設定アクション (ポップオーバー内包) */}
              <div className="pagination-right-actions">
                <Button
                  type="button"
                  variant={showAaPopup ? "secondary" : "outline"}
                  size="sm"
                  className={cn("aa-toggle-btn", showAaPopup && "active")}
                  onClick={() => setShowAaPopup(!showAaPopup)}
                  title="表示設定 (Aa)"
                >
                  Aa
                </Button>

                {showAaPopup && (
                  <div className="aa-popover-panel">
                    <div className="popover-arrow" />
                    
                    {/* 1. 背景 */}
                    <div className="popover-section">
                      <span className="popover-label">背景</span>
                      <div className="popover-btn-group">
                        <Button
                          type="button"
                          variant={theme === "white" ? "secondary" : "outline"}
                          size="sm"
                          className={cn("popover-btn", theme === "white" && "active")}
                          onClick={() => setTheme("white")}
                        >
                          白
                        </Button>
                        <Button
                          type="button"
                          variant={theme === "sepia" ? "secondary" : "outline"}
                          size="sm"
                          className={cn("popover-btn", theme === "sepia" && "active")}
                          onClick={() => setTheme("sepia")}
                        >
                          茶
                        </Button>
                        <Button
                          type="button"
                          variant={theme === "dark" ? "secondary" : "outline"}
                          size="sm"
                          className={cn("popover-btn", theme === "dark" && "active")}
                          onClick={() => setTheme("dark")}
                        >
                          黒
                        </Button>
                      </div>
                    </div>

                    {/* 2. 書体 */}
                    <div className="popover-section">
                      <span className="popover-label">書体</span>
                      <div className="popover-btn-group">
                        <Button
                          type="button"
                          variant={fontFamily === "serif" ? "secondary" : "outline"}
                          size="sm"
                          className={cn("popover-btn", fontFamily === "serif" && "active")}
                          onClick={() => setFontFamily("serif")}
                        >
                          明朝
                        </Button>
                        <Button
                          type="button"
                          variant={fontFamily === "sans" ? "secondary" : "outline"}
                          size="sm"
                          className={cn("popover-btn", fontFamily === "sans" && "active")}
                          onClick={() => setFontFamily("sans")}
                        >
                          ゴシック
                        </Button>
                      </div>
                    </div>

                    {/* 3. 文字サイズ (スライダー形式へ進化) */}
                    <div className="popover-section">
                      <div className="mb-1 flex items-center justify-between gap-3">
                        <span className="popover-label mb-0">文字サイズ</span>
                        <span className="popover-val">{fontSize}px</span>
                      </div>
                      <Slider
                        min={13}
                        max={24}
                        step={1}
                        value={[fontSize]}
                        onValueChange={([next]) => setFontSize(next)}
                        className="popover-slider"
                      />
                    </div>

                    {/* 4. 行間 (スライダー形式) */}
                    <div className="popover-section mb-0 border-b-0 pb-0">
                      <div className="mb-1 flex items-center justify-between gap-3">
                        <span className="popover-label mb-0">行間</span>
                        <span className="popover-val">{lineHeight.toFixed(2)}</span>
                      </div>
                      <Slider
                        min={1.4}
                        max={2.2}
                        step={0.05}
                        value={[lineHeight]}
                        onValueChange={([next]) => setLineHeight(next)}
                        className="popover-slider"
                      />
                    </div>
                  </div>
                )}
              </div>
            </div>
          </div>
        )}

        {tab === "assets" && (
          <div className="asset-list">
            {imageAssets.length > 0 && (
              <div className="asset-section">
                <h4><ImageIcon /> 画像 ({imageAssets.length})</h4>
                <div className="asset-grid">
                  {imageAssets.map(a => (
                    <Card key={a.id} className="asset-item image" onClick={() => setSelectedImage(a.localPath)}>
                      <LazyImage localPath={a.localPath} alt={a.filename} />
                      <span>{a.filename}</span>
                    </Card>
                  ))}
                </div>
              </div>
            )}
            {fileAssets.length > 0 && (
              <div className="asset-section">
                <h4><FileIcon /> ファイル ({fileAssets.length})</h4>
                {fileAssets.map(a => (
                  <Card
                    key={a.id} 
                    className="asset-item file clickable-file"
                    onClick={() => handleOpenAsset(a.localPath)}
                    title="クリックしてファイルを開く"
                  >
                    <FolderIcon />
                    <span className="asset-name">{a.filename}</span>
                    <span className="asset-size">{(a.fileSizeBytes / 1024).toFixed(1)} KB</span>
                  </Card>
                ))}
              </div>
            )}
          </div>
        )}

        {tab === "json" && (
          <div className="json-viewer">
            <pre><code>{jsonContent}</code></pre>
          </div>
        )}

        {tab === "diff" && (
          <DiffViewer
            versions={versions}
            currentVersion={dl.currentVersion}
            beforeVersion={diffBeforeVersion}
            afterVersion={diffAfterVersion}
            beforeContent={diffBeforeContent}
            afterContent={diffAfterContent}
            loading={loadingDiff}
            onBeforeVersionChange={setDiffBeforeVersion}
            onAfterVersionChange={setDiffAfterVersion}
          />
        )}
      </div>

      {/* Image lightbox */}
      {selectedImage && (
        <Lightbox localPath={selectedImage} onClose={() => setSelectedImage(null)} />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sub-components for lazy loading local assets
// ---------------------------------------------------------------------------

function LazyImage({ localPath, alt, className }: { localPath: string; alt?: string; className?: string }) {
  const [orientation, setOrientation] = useState<"wide" | "tall" | "square">("square");
  const src = useMemo(() => getAssetUrl(localPath), [localPath]);

  if (!src) {
    return <div className="thumb-placeholder"><ImageIcon /></div>;
  }

  return (
    <img
      src={src}
      alt={alt}
      className={[className, `image-${orientation}`].filter(Boolean).join(" ")}
      onLoad={e => {
        const img = e.currentTarget;
        const ratio = img.naturalWidth / Math.max(1, img.naturalHeight);
        setOrientation(ratio > 1.2 ? "wide" : ratio < 0.82 ? "tall" : "square");
      }}
    />
  );
}

function Lightbox({ localPath, onClose }: { localPath: string; onClose: () => void }) {
  const src = useMemo(() => getAssetUrl(localPath), [localPath]);

  return (
    <div className="lightbox" onClick={onClose}>
      {src ? (
        <img src={src} alt="" onClick={e => e.stopPropagation()} />
      ) : (
        <div className="spinner" />
      )}
    </div>
  );
}

function diffLines(oldText: string, newText: string) {
  const oldLines = oldText.split("\n");
  const newLines = newText.split("\n");
  const dp: number[][] = Array(oldLines.length + 1)
    .fill(null)
    .map(() => Array(newLines.length + 1).fill(0));

  for (let i = 1; i <= oldLines.length; i++) {
    for (let j = 1; j <= newLines.length; j++) {
      if (oldLines[i - 1] === newLines[j - 1]) {
        dp[i][j] = dp[i - 1][j - 1] + 1;
      } else {
        dp[i][j] = Math.max(dp[i - 1][j], dp[i][j - 1]);
      }
    }
  }

  let i = oldLines.length;
  let j = newLines.length;
  const result: { type: "added" | "removed" | "unchanged"; text: string }[] = [];

  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && oldLines[i - 1] === newLines[j - 1]) {
      result.unshift({ type: "unchanged", text: oldLines[i - 1] });
      i--;
      j--;
    } else if (j > 0 && (i === 0 || dp[i][j - 1] >= dp[i - 1][j])) {
      result.unshift({ type: "added", text: newLines[j - 1] });
      j--;
    } else {
      result.unshift({ type: "removed", text: oldLines[i - 1] });
      i--;
    }
  }

  return result;
}

function DiffViewer({
  versions,
  currentVersion,
  beforeVersion,
  afterVersion,
  beforeContent,
  afterContent,
  loading,
  onBeforeVersionChange,
  onAfterVersionChange,
}: {
  versions: DownloadVersion[];
  currentVersion: number;
  beforeVersion: number | null;
  afterVersion: number | null;
  beforeContent: string;
  afterContent: string;
  loading: boolean;
  onBeforeVersionChange: (version: number | null) => void;
  onAfterVersionChange: (version: number | null) => void;
}) {
  const lines = useMemo(() => diffLines(beforeContent, afterContent), [beforeContent, afterContent]);
  const addedCount = lines.filter(line => line.type === "added").length;
  const removedCount = lines.filter(line => line.type === "removed").length;
  const sameVersion = beforeVersion !== null && beforeVersion === afterVersion;

  if (versions.length <= 1) {
    return (
      <div className="diff-viewer">
        <Card className="diff-empty p-10 text-center text-muted-foreground">
          <p className="text-base font-bold text-foreground">バージョンが1つのため差分はありません</p>
          <p className="mt-2 text-xs">この作品に新しいバージョン（更新）がダウンロードされると、ここに変更点の比較が表示されます。</p>
        </Card>
      </div>
    );
  }

  return (
    <div className="diff-viewer">
      <div className="diff-container flex flex-col gap-4">
        <Card className="diff-toolbar flex flex-wrap items-center gap-3 p-3">
          <div className="select-wrapper flex items-center gap-2">
            <span className="text-xs text-muted-foreground">比較元 (Before):</span>
            <Select
              value={beforeVersion == null ? "__none__" : String(beforeVersion)}
              onValueChange={(value) => onBeforeVersionChange(value === "__none__" ? null : Number(value))}
            >
              <SelectTrigger className="version-select-box h-8 w-auto min-w-24">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__none__">未選択</SelectItem>
              {versions.map(version => (
                <SelectItem key={version.id} value={String(version.version)}>v{version.version}</SelectItem>
              ))}
              </SelectContent>
            </Select>
          </div>
          <span className="text-muted-foreground">-&gt;</span>
          <div className="select-wrapper flex items-center gap-2">
            <span className="text-xs text-muted-foreground">比較先 (After):</span>
            <Select
              value={afterVersion == null ? "__none__" : String(afterVersion)}
              onValueChange={(value) => onAfterVersionChange(value === "__none__" ? null : Number(value))}
            >
              <SelectTrigger className="version-select-box h-8 w-auto min-w-24">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__none__">未選択</SelectItem>
              {versions.map(version => (
                <SelectItem key={version.id} value={String(version.version)}>
                  v{version.version} {version.version === currentVersion ? "(最新)" : ""}
                </SelectItem>
              ))}
              </SelectContent>
            </Select>
          </div>
          {sameVersion ? (
            <Badge variant="destructive" className="ml-auto">同じバージョン</Badge>
          ) : (
            <div className="ml-auto flex items-center gap-2">
              <Badge variant="outline">+{addedCount}</Badge>
              <Badge variant="outline">-{removedCount}</Badge>
            </div>
          )}
        </Card>

        {loading ? (
          <Card className="diff-loading space-y-3 p-10">
            <div className="flex flex-col items-center gap-3 text-sm text-muted-foreground">
              <div className="spinner-mini" />
              <span>差分を算出中...</span>
            </div>
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-5/6" />
            <Skeleton className="h-4 w-3/4" />
          </Card>
        ) : (
          <Card className="diff-reader max-h-[65vh] overflow-y-auto p-4">
            <div className="novel-diff-text font-[inherit] text-[15px] leading-[1.8]">
              {lines.map((line, index) => {
                const prefix = line.type === "added" ? "+ " : line.type === "removed" ? "- " : "";
                return (
                  <div
                    key={`${line.type}-${index}`}
                    className={cn(
                      "my-0.5 flex gap-1 whitespace-pre-wrap rounded px-2 py-0.5",
                      line.type === "added" && "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300",
                      line.type === "removed" && "bg-rose-500/15 text-rose-700 dark:text-rose-300",
                    )}
                  >
                    <span className="w-4 shrink-0 select-none opacity-50">{prefix}</span>
                    <span>{line.text || "\u00A0"}</span>
                  </div>
                );
              })}
            </div>
          </Card>
        )}
      </div>
    </div>
  );
}

function ParsedHtmlRenderer({ html, onJump, onLinkClick }: { html: string; onJump?: (page: number) => void; onLinkClick?: (e: React.MouseEvent) => void }) {
  if (!html) return null;

  // <img class="novel-image" data-local-path="<path>" alt="<alt>" /> を検出して分割
  const imgRegex = /<img class="novel-image" data-local-path="(?<path>[^"]+)" alt="(?<alt>[^"]+)" \/>/g;
  
  const parts: React.ReactNode[] = [];
  let lastIndex = 0;
  let match;

  // regex の global flag を活かしてループ
  while ((match = imgRegex.exec(html)) !== null) {
    const textPart = html.substring(lastIndex, match.index);
    if (textPart) {
      parts.push(
        <div 
          key={`text-${lastIndex}`} 
          className="parsed-html-text-block"
          dangerouslySetInnerHTML={{ __html: textPart }} 
        />
      );
    }

    const localPath = match.groups?.path || "";
    const alt = match.groups?.alt || "image";
    
    parts.push(
      <div key={`img-${match.index}`} className="reader-image-container my-6 text-center">
        <LazyImage localPath={localPath} alt={alt} className="novel-embedded-image" />
      </div>
    );

    lastIndex = imgRegex.lastIndex;
  }

  const remainingText = html.substring(lastIndex);
  if (remainingText) {
    parts.push(
      <div 
        key={`text-${lastIndex}`} 
        className="parsed-html-text-block"
        dangerouslySetInnerHTML={{ __html: remainingText }} 
      />
    );
  }

  const handleClick = (e: React.MouseEvent) => {
    const target = (e.target as HTMLElement).closest(".jump-link");
    if (target) {
      e.preventDefault();
      e.stopPropagation();
      const pageNum = Number(target.getAttribute("data-page"));
      if (pageNum && onJump) {
        onJump(pageNum);
      }
      return;
    }

    const anchor = (e.target as HTMLElement).closest("a");
    if (anchor) {
      onLinkClick?.(e);
      return;
    }

    const fileTarget = (e.target as HTMLElement).closest(".clickable-file");
    if (fileTarget) {
      e.preventDefault();
      e.stopPropagation();
      const localPath = fileTarget.getAttribute("data-local-path");
      if (localPath) {
        openLocalAsset(localPath).catch(err => {
          console.error("Failed to open local file:", err);
        });
      }
    }
  };

  return <div className="parsed-html-renderer" onClick={handleClick}>{parts}</div>;
}
