import { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { openPath, openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { store } from "../../store";
import { ExportIcon, FileIcon, ImageIcon, FolderIcon, SyncIcon, RefreshIcon, LinkIcon, TrashIcon, BookIcon } from "../icons/Icons";

interface DownloadEntry {
  id: number; source: string; sourceId: string; title: string;
  authorName: string; authorId: string; contentType: string;
  tags: string | null; coverPath: string | null; jsonPath: string;
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

export function ContentViewer({ downloadId, showToast, onSelectTagFilter, onViewPerson, onViewSeries, onExportEpub, isEpubActive, onNavigateInternalUrl, onOpenSourceUrl, onDeleted }: Props) {
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
          openUrl(href).catch(err => {
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
      invoke<DownloadEntry>("db_get_download", { id: downloadId }),
      invoke<AssetEntry[]>("db_get_assets", { downloadId }),
      invoke<DownloadVersion[]>("db_get_versions", { downloadId }),
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
          invoke<DownloadEntry>("db_get_download", { id: downloadId }),
          invoke<AssetEntry[]>("db_get_assets", { downloadId }),
          invoke<DownloadVersion[]>("db_get_versions", { downloadId }),
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
          invoke<string>("read_file_content", { path: beforeJsonPath }),
          invoke<string>("read_file_content", { path: afterJsonPath })
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
          const rawOrigContent = await invoke<string>("read_file_content", { path: targetOrigPath });
          setJsonContent(rawOrigContent);
        } catch (err) {
          console.warn("Failed to load original JSON, falling back to modified data.json:", err);
          // 失敗した場合は加工済み data.json をロード
          let fallbackPath = dl.jsonPath;
          if (selectedVersion !== dl.currentVersion) {
            const ver = versions.find(v => v.version === selectedVersion);
            if (ver) fallbackPath = ver.jsonPath;
          }
          const content = await invoke<string>("read_file_content", { path: fallbackPath });
          setJsonContent(content);
        }

        // B. オンザフライで動的HTMLをパース取得 (本文タブ用)
        try {
          const html = await invoke<string>("db_get_download_html", {
            downloadId: dl.id,
            version: selectedVersion
          });
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
          const content = await invoke<string>("read_file_content", { path: targetPath });
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
      const dir = await open({
        title: "保存先フォルダを選択",
        directory: true,
        multiple: false
      });
      if (!dir) return;
      const outputDir = await invoke<string>("export_single", { downloadId, destDir: dir });
      showToast(`保存しました: ${outputDir}`, "success");
    } catch (e: any) { showToast(`ファイル保存エラー: ${e}`, "error"); }
  };

  const handleToggleWatch = async () => {
    if (!dl) return;
    try {
      const nextWatch = !watchUpdates;
      await invoke("db_set_watch_updates", { downloadId, watch: nextWatch });
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
      await invoke("db_set_favorite", { downloadId, favorite: nextFav });
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
        await revealItemInDir(dl.jsonPath);
      } catch {
        await openPath(dirPath);
      }
      showToast("ローカルフォルダを開きました", "success");
    } catch (e) {
      console.error("Failed to open local folder:", e);
      showToast(`フォルダを開けませんでした: ${e}`, "error");
    }
  };

  const handleOpenAsset = async (path: string) => {
    try {
      await invoke("open_local_asset", { path });
    } catch (e) {
      console.error("Failed to open asset file:", e);
      showToast(`ファイルを開けませんでした: ${e}`, "error");
    }
  };

  const handleCheckUpdate = async () => {
    if (!dl) return;
    setIsCheckingUpdate(true);
    showToast("最新情報の取得中...", "info");
    try {
      if (dl.source === "pixiv") {
        const refreshToken = await store.get<string>("pixiv_refresh_token");
        if (!refreshToken) {
          showToast("Pixivの連携が設定されていません。設定画面でログインしてください。", "error");
          setIsCheckingUpdate(false);
          return;
        }
        const novelUrl = `https://www.pixiv.net/novel/show.php?id=${dl.sourceId}`;
        const data = await invoke<any>("fetch_pixiv_novel_by_url", { url: novelUrl, refreshToken });
        
        const title = data.detail?.title || data.title || dl.title;
        const author = data.detail?.user?.name || data.user?.name || dl.authorName;
        const authorId = String(data.detail?.user?.id || data.user?.id || dl.authorId);
        
        const extractTags = (novel: any) => {
          const directTags = novel.tags || [];
          const detailTags = novel.detail?.tags;
          if (Array.isArray(detailTags)) {
            return detailTags.map((t: any) => typeof t === "string" ? t : t.name);
          } else if (detailTags && typeof detailTags === "object" && "tags" in detailTags) {
            return (detailTags.tags as any[]).map((t: any) => t.name);
          }
          return directTags.map((t: any) => typeof t === "string" ? t : t.name);
        };
        const tagsList = extractTags(data);
        
        const resolvedExcerptText = data.caption || data.detail?.caption || null;
        
        const updated = await invoke<DownloadEntry>("download_and_save", {
          data, source: "pixiv", sourceId: dl.sourceId, title, authorName: author, authorId,
          contentType: "novel", tags: tagsList, excerpt: resolvedExcerptText,
          sourceCreatedAt: data.detail?.create_date || data.detail?.createDate || data.create_date || data.createDate || null,
          cookie: null,
          userAgent: null,
        });
        
        if (updated.currentVersion > dl.currentVersion) {
          showToast(`アップデートを検出しました！ v${dl.currentVersion} ➔ v${updated.currentVersion}`, "success");
          // Reload metadata
          await reloadMetadata();
        } else {
          showToast("すでに最新バージョンです（更新はありませんでした）", "success");
        }
      } else if (dl.source === "fanbox") {
        const cookie = await store.get<string>("fanbox_session_id");
        const ua = await store.get<string>("fanbox_user_agent");
        if (!cookie || !ua) {
          showToast("FANBOXの連携が設定されていません。設定画面でログインしてください。", "error");
          setIsCheckingUpdate(false);
          return;
        }
        const data = await invoke<any>("fetch_fanbox_post", { postId: dl.sourceId, cookie, userAgent: ua });
        
        const title = data.title || dl.title;
        const author = data.user?.name || dl.authorName;
        const authorId = data.creatorId || data.user?.userId || dl.authorId;
        const tagsList = data.tags || [];
        const resolvedExcerptText = data.excerpt || data.body?.excerpt || null;
        
        const updated = await invoke<DownloadEntry>("download_and_save", {
          data, source: "fanbox", sourceId: dl.sourceId, title, authorName: author, authorId,
          contentType: data.type || "article", tags: tagsList, excerpt: resolvedExcerptText,
          sourceCreatedAt: data.publishedDatetime || null, cookie, userAgent: ua,
        });
        
        if (updated.currentVersion > dl.currentVersion) {
          showToast(`アップデートを検出しました！ v${dl.currentVersion} ➔ v${updated.currentVersion}`, "success");
          // Reload metadata
          await reloadMetadata();
        } else {
          showToast("すでに最新バージョンです（更新はありませんでした）", "success");
        }
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
    const ok = await ask(message, {
      title: deletingWork ? "作品削除の確認" : "バージョン削除の確認",
      kind: "warning",
      okLabel: "削除する",
      cancelLabel: "キャンセル"
    });
    if (!ok) return;

    setIsDeletingVersion(true);
    try {
      if (deletingWork) {
        await invoke("db_delete_download", { id: dl.id });
        showToast("作品を削除しました", "success");
        onDeleted?.();
      } else {
        await invoke("db_delete_version", { downloadId: dl.id, version: selectedVersion });
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

  const tags = dl.tags ? JSON.parse(dl.tags) as string[] : [];
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
                <span style={{ fontSize: "12px", color: "var(--color-text-secondary)" }}>Ver</span>
                <select
                  className="version-select-box"
                  value={selectedVersion || dl.currentVersion}
                  onChange={(e) => setSelectedVersion(Number(e.target.value))}
                >
                  {versions.map(v => (
                    <option key={v.id} value={v.version}>
                      v{v.version} ({new Date(v.createdAt).toLocaleDateString("ja-JP")}) {v.version === dl.currentVersion ? "(最新)" : ""}
                    </option>
                  ))}
                </select>
              </div>
            )}
            <button
              className="icon-btn"
              onClick={handleCheckUpdate}
              disabled={isCheckingUpdate}
              title="最新情報を取得して更新確認を行います"
            >
              {isCheckingUpdate ? (
                <span className="spinner-mini" style={{ display: "inline-block" }} />
              ) : (
                <RefreshIcon />
              )}
            </button>
            <button
              className="icon-btn"
              onClick={handleOpenSourceInBrowser}
              title="保存元ページをアプリ内ブラウザで開きます"
              disabled={!onOpenSourceUrl}
            >
              <LinkIcon />
            </button>
          </div>
          <div className="viewer-action-group file-action-group">
            <button
              className="icon-btn"
              onClick={handleDeleteSelectedVersion}
              disabled={isDeletingVersion || selectedVersion === null}
              title={versions.length <= 1 ? "作品を完全に削除します" : "閲覧中のバージョンを削除します"}
            >
              {isDeletingVersion ? (
                <span className="spinner-mini" style={{ display: "inline-block" }} />
              ) : (
                <TrashIcon />
              )}
            </button>
            <button
              className="icon-btn"
              onClick={handleOpenFolder}
              title="保存先のフォルダを開きます"
            >
              <FolderIcon />
            </button>
            <button
              className="icon-btn"
              onClick={handleExport}
              title="この作品のテキストや画像などの物理ファイルを指定したフォルダに保存します"
            >
              <ExportIcon />
            </button>
          </div>
          {onExportEpub && (
            <div className="viewer-action-group epub-action-group">
              <button 
                className={`icon-btn sidebar-toggle-btn ${isEpubActive ? "primary" : ""}`} 
                onClick={() => onExportEpub(dl)}
                title={isEpubActive ? "EPUB設定サイドバーを閉じます" : "EPUB設定サイドバーを開きます"}
              >
                <BookIcon />
              </button>
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
          <span className={`source-tag ${dl.source}`}>{dl.source === "pixiv" ? "Pixiv" : "FANBOX"}</span>
          {dl.seriesId && dl.seriesTitle && (
            <button
              type="button"
              className="viewer-series-link"
              onClick={() => onViewSeries?.(dl.source, dl.seriesId!)}
              title="シリーズページを開く"
            >
              {dl.seriesTitle}
            </button>
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
                <span 
                  key={i} 
                  className="tag clickable-meta"
                  onClick={() => onSelectTagFilter?.(t)}
                  title={`タグ #${t} でライブラリを検索`}
                >
                  #{t}
                </span>
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
              <button 
                className="description-toggle-btn"
                onClick={() => setIsDescriptionExpanded(!isDescriptionExpanded)}
              >
                {isDescriptionExpanded ? "閉じる ▲" : "続きを読む ▼"}
              </button>
            </div>
          )}
        </div>
      </div>

      {/* Tabs */}
      <div className="viewer-tabs">
        <button className={`tab ${tab === "overview" ? "active" : ""}`} onClick={() => setTab("overview")}>概要</button>
        {textContent && <button className={`tab ${tab === "text" ? "active" : ""}`} onClick={() => setTab("text")}>本文</button>}
        <button className={`tab ${tab === "assets" ? "active" : ""}`} onClick={() => setTab("assets")}>アセット ({assets.length})</button>
        <button className={`tab ${tab === "json" ? "active" : ""}`} onClick={() => setTab("json")}>JSON</button>
        <button className={`tab ${tab === "diff" ? "active" : ""}`} onClick={() => setTab("diff")}>差分</button>
      </div>

      {/* Tab content */}
      <div className={`viewer-content tab-${tab} ${
        tab === "text" || tab === "json" ? "scroll-contained" : "scroll-allowed"
      }`}>
        {tab === "overview" && (
          <div className="overview-grid">
            <div className="info-table">
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
              <div className="info-row"><span>コンテンツハッシュ</span><span style={{ fontFamily: "monospace", fontSize: "11px", wordBreak: "break-all" }}>{dl.contentHash || "未算出"}</span></div>
              <div className="info-row"><span>ダウンロード日</span><span>{formatDateTime(dl.downloadedAt)}</span></div>
              {sourcePublishedAt && <div className="info-row"><span>投稿日</span><span>{formatDateTime(sourcePublishedAt)}</span></div>}
              {sourceUpdatedAt && <div className="info-row"><span>更新日</span><span>{formatDateTime(sourceUpdatedAt)}</span></div>}
            </div>
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
                    <button 
                      className="pagination-btn"
                      disabled={currentPage === 1}
                      onClick={() => {
                        setCurrentPage(p => Math.max(1, p - 1));
                      }}
                    >
                      前へ
                    </button>
                    
                    {Array.from({ length: pages.length }).map((_, idx) => {
                      const pNum = idx + 1;
                      if (pNum === 1 || pNum === pages.length || Math.abs(pNum - currentPage) <= 2) {
                        return (
                          <button 
                            key={pNum}
                            className={`pagination-btn ${currentPage === pNum ? "active" : ""}`}
                            onClick={() => {
                              setCurrentPage(pNum);
                            }}
                          >
                            {pNum}
                          </button>
                        );
                      } else if (pNum === 2 || pNum === pages.length - 1) {
                        return <span key={`ellipsis-${pNum}`} className="pagination-ellipsis">...</span>;
                      }
                      return null;
                    })}

                    <button 
                      className="pagination-btn"
                      disabled={currentPage === pages.length}
                      onClick={() => {
                        setCurrentPage(p => Math.min(pages.length, p + 1));
                      }}
                    >
                      次へ
                    </button>
                  </>
                ) : (
                  <span className="pagination-single-page">1 / 1 ページ</span>
                )}
              </div>

              {/* 右側のAa設定アクション (ポップオーバー内包) */}
              <div className="pagination-right-actions">
                <button 
                  className={`aa-toggle-btn ${showAaPopup ? "active" : ""}`}
                  onClick={() => setShowAaPopup(!showAaPopup)}
                  title="表示設定 (Aa)"
                >
                  Aa
                </button>

                {showAaPopup && (
                  <div className="aa-popover-panel">
                    <div className="popover-arrow" />
                    
                    {/* 1. 背景 */}
                    <div className="popover-section">
                      <span className="popover-label">背景</span>
                      <div className="popover-btn-group">
                        <button 
                          className={`popover-btn ${theme === "white" ? "active" : ""}`}
                          onClick={() => setTheme("white")}
                        >
                          白
                        </button>
                        <button 
                          className={`popover-btn ${theme === "sepia" ? "active" : ""}`}
                          onClick={() => setTheme("sepia")}
                        >
                          茶
                        </button>
                        <button 
                          className={`popover-btn ${theme === "dark" ? "active" : ""}`}
                          onClick={() => setTheme("dark")}
                        >
                          黒
                        </button>
                      </div>
                    </div>

                    {/* 2. 書体 */}
                    <div className="popover-section">
                      <span className="popover-label">書体</span>
                      <div className="popover-btn-group">
                        <button 
                          className={`popover-btn ${fontFamily === "serif" ? "active" : ""}`}
                          onClick={() => setFontFamily("serif")}
                        >
                          明朝
                        </button>
                        <button 
                          className={`popover-btn ${fontFamily === "sans" ? "active" : ""}`}
                          onClick={() => setFontFamily("sans")}
                        >
                          ゴシック
                        </button>
                      </div>
                    </div>

                    {/* 3. 文字サイズ (スライダー形式へ進化) */}
                    <div className="popover-section">
                      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "4px" }}>
                        <span className="popover-label" style={{ marginBottom: 0 }}>文字サイズ</span>
                        <span className="popover-val">{fontSize}px</span>
                      </div>
                      <input 
                        type="range" 
                        min="13" 
                        max="24" 
                        step="1" 
                        value={fontSize} 
                        onChange={(e) => setFontSize(parseInt(e.target.value))}
                        className="popover-slider"
                      />
                    </div>

                    {/* 4. 行間 (スライダー形式) */}
                    <div className="popover-section" style={{ borderBottom: "none", paddingBottom: 0, marginBottom: 0 }}>
                      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "4px" }}>
                        <span className="popover-label" style={{ marginBottom: 0 }}>行間</span>
                        <span className="popover-val">{lineHeight.toFixed(2)}</span>
                      </div>
                      <input 
                        type="range" 
                        min="1.4" 
                        max="2.2" 
                        step="0.05" 
                        value={lineHeight} 
                        onChange={(e) => setLineHeight(parseFloat(e.target.value))}
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
                    <div key={a.id} className="asset-item image" onClick={() => setSelectedImage(a.localPath)}>
                      <LazyImage localPath={a.localPath} alt={a.filename} />
                      <span>{a.filename}</span>
                    </div>
                  ))}
                </div>
              </div>
            )}
            {fileAssets.length > 0 && (
              <div className="asset-section">
                <h4><FileIcon /> ファイル ({fileAssets.length})</h4>
                {fileAssets.map(a => (
                  <div 
                    key={a.id} 
                    className="asset-item file clickable-file"
                    onClick={() => handleOpenAsset(a.localPath)}
                    title="クリックしてファイルを開く"
                    style={{ cursor: "pointer" }}
                  >
                    <FolderIcon />
                    <span className="asset-name">{a.filename}</span>
                    <span className="asset-size">{(a.fileSizeBytes / 1024).toFixed(1)} KB</span>
                  </div>
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
          <div className="diff-viewer">
            {versions.length <= 1 ? (
              <div className="diff-empty" style={{ textAlign: "center", padding: "40px", color: "var(--color-text-secondary)" }}>
                <p style={{ fontSize: "16px", fontWeight: "bold" }}>バージョンが1つのため差分はありません</p>
                <p style={{ fontSize: "12px", marginTop: "8px" }}>この作品に新しいバージョン（更新）がダウンロードされると、ここに変更点の比較が表示されます。</p>
              </div>
            ) : (
              <div className="diff-container" style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
                {/* 差分バージョンセレクターツールバー */}
                <div className="diff-toolbar" style={{ display: "flex", alignItems: "center", gap: "12px", padding: "8px 12px", background: "var(--color-bg-sidebar)", borderRadius: "6px", flexWrap: "wrap" }}>
                  <div className="select-wrapper" style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                    <span style={{ fontSize: "12px", color: "var(--color-text-secondary)" }}>比較元 (Before):</span>
                    <select
                      className="version-select-box"
                      value={diffBeforeVersion || ""}
                      onChange={(e) => setDiffBeforeVersion(Number(e.target.value))}
                    >
                      {versions.map(v => (
                        <option key={v.id} value={v.version}>v{v.version}</option>
                      ))}
                    </select>
                  </div>
                  <span style={{ color: "var(--color-text-secondary)" }}>➔</span>
                  <div className="select-wrapper" style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                    <span style={{ fontSize: "12px", color: "var(--color-text-secondary)" }}>比較先 (After):</span>
                    <select
                      className="version-select-box"
                      value={diffAfterVersion || ""}
                      onChange={(e) => setDiffAfterVersion(Number(e.target.value))}
                    >
                      {versions.map(v => (
                        <option key={v.id} value={v.version}>v{v.version} {v.version === dl.currentVersion ? "(最新)" : ""}</option>
                      ))}
                    </select>
                  </div>
                  {diffBeforeVersion === diffAfterVersion && (
                    <span style={{ fontSize: "11px", color: "red", marginLeft: "auto" }}>
                      ⚠️ 同じバージョンが選択されています。差分は発生しません。
                    </span>
                  )}
                </div>

                {/* 差分本文表示エリア */}
                {loadingDiff ? (
                  <div className="diff-loading" style={{ textAlign: "center", padding: "40px" }}>
                    <div className="spinner-mini" style={{ margin: "0 auto 10px" }} />
                    <span>差分を算出中...</span>
                  </div>
                ) : (
                  <div className="diff-reader" style={{ background: "var(--color-bg-main)", borderRadius: "8px", border: "1px solid var(--color-border)", padding: "16px", overflowY: "auto", maxHeight: "65vh" }}>
                    <div className="novel-diff-text" style={{ fontFamily: "inherit", lineHeight: "1.8", fontSize: "15px" }}>
                      {diffLines(diffBeforeContent, diffAfterContent).map((line, idx) => {
                        let bgColor = "transparent";
                        let textColor = "inherit";
                        let prefix = "";

                        if (line.type === "added") {
                          bgColor = "rgba(46, 160, 67, 0.15)";
                          textColor = "#2ea043";
                          prefix = "+ ";
                        } else if (line.type === "removed") {
                          bgColor = "rgba(248, 81, 198, 0.15)";
                          textColor = "#f851c6";
                          prefix = "- ";
                        }

                        return (
                          <div
                            key={idx}
                            style={{
                              backgroundColor: bgColor,
                              color: textColor,
                              padding: "2px 8px",
                              margin: "2px 0",
                              borderRadius: "4px",
                              whiteSpace: "pre-wrap",
                              display: "flex",
                              gap: "4px"
                            }}
                          >
                            <span style={{ opacity: 0.5, userSelect: "none", width: "16px", flexShrink: 0 }}>{prefix}</span>
                            <span>{line.text || "\u00A0"}</span>
                          </div>
                        );
                      })}
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
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
  const [src, setSrc] = useState<string | null>(null);
  const [orientation, setOrientation] = useState<"wide" | "tall" | "square">("square");

  useEffect(() => {
    let active = true;
    invoke<string>("read_image_base64", { path: localPath })
      .then(b64 => {
        if (active) setSrc(b64);
      })
      .catch(e => console.error("LazyImage error:", e));
    return () => { active = false; };
  }, [localPath]);

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
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    invoke<string>("read_image_base64", { path: localPath })
      .then(b64 => {
        if (active) setSrc(b64);
      })
      .catch(e => console.error("Lightbox error:", e));
    return () => { active = false; };
  }, [localPath]);

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
      <div key={`img-${match.index}`} className="reader-image-container" style={{ margin: "24px 0", textAlign: "center" }}>
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
        invoke("open_local_asset", { path: localPath }).catch(err => {
          console.error("Failed to open local file:", err);
        });
      }
    }
  };

  return <div className="parsed-html-renderer" onClick={handleClick}>{parts}</div>;
}
