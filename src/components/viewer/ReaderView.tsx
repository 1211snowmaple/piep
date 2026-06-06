import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowLeft,
  BookOpenText,
  Edit3,
  ExternalLink,
  Minus,
  Plus,
  RotateCcw,
  Settings2,
  Type,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Slider } from "@/components/ui/slider";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { getAssetUrl, getReaderDocument } from "@/services/dbApi";

interface ReaderViewProps {
  downloadId: number;
  showToast: (msg: string, type: "success" | "error" | "info") => void;
  onBack: () => void;
  onEdit: () => void;
  onOpenSourceUrl?: (url: string) => void;
}

type ReaderTheme = "paper" | "white" | "dark";
type ReaderFont = "serif" | "sans";
type ReaderWidth = "narrow" | "standard" | "wide";

const themeLabels: Record<ReaderTheme, string> = {
  paper: "紙",
  white: "白",
  dark: "黒",
};

const widthClass: Record<ReaderWidth, string> = {
  narrow: "max-w-[680px]",
  standard: "max-w-[820px]",
  wide: "max-w-[980px]",
};

const themeClass: Record<ReaderTheme, string> = {
  paper: "bg-[#f6efe3] text-[#29251f]",
  white: "bg-white text-slate-950",
  dark: "bg-[#10131a] text-slate-100",
};

function readNumberSetting(key: string, fallback: number): number {
  const value = Number(localStorage.getItem(key));
  return Number.isFinite(value) && value > 0 ? value : fallback;
}

function readSetting<T extends string>(key: string, fallback: T, allowed: readonly T[]): T {
  const value = localStorage.getItem(key) as T | null;
  return value && allowed.includes(value) ? value : fallback;
}

function sourceUrl(source: string, sourceId: string, authorId: string): string | null {
  if (source === "pixiv") return `https://www.pixiv.net/novel/show.php?id=${sourceId}`;
  if (source === "fanbox") {
    return authorId
      ? `https://${authorId}.fanbox.cc/posts/${sourceId}`
      : `https://www.fanbox.cc/posts/${sourceId}`;
  }
  return null;
}

function htmlWithLocalAssets(html: string): string {
  return html.replace(/<img([^>]*?)data-local-path="([^"]+)"([^>]*)>/g, (_match, before, path, after) => {
    const src = getAssetUrl(path);
    return src ? `<img${before}src="${src}" data-local-path="${path}"${after}>` : `<img${before}data-local-path="${path}"${after}>`;
  });
}

function estimateMinutes(text: string): number {
  return Math.max(1, Math.ceil((text?.length ?? 0) / 650));
}

export function ReaderView({ downloadId, showToast, onBack, onEdit, onOpenSourceUrl }: ReaderViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [fontSize, setFontSize] = useState(() => readNumberSetting("piep-reader-v2-font-size", 18));
  const [lineHeight, setLineHeight] = useState(() => readNumberSetting("piep-reader-v2-line-height", 1.85));
  const [theme, setTheme] = useState<ReaderTheme>(() => readSetting("piep-reader-v2-theme", "paper", ["paper", "white", "dark"]));
  const [fontFamily, setFontFamily] = useState<ReaderFont>(() => readSetting("piep-reader-v2-font", "serif", ["serif", "sans"]));
  const [measure, setMeasure] = useState<ReaderWidth>(() => readSetting("piep-reader-v2-width", "standard", ["narrow", "standard", "wide"]));
  const [showSettings, setShowSettings] = useState(false);
  const [progress, setProgress] = useState(0);

  const readerQuery = useQuery({
    queryKey: ["reader-document", downloadId],
    queryFn: () => getReaderDocument(downloadId),
  });

  const doc = readerQuery.data;
  const scrollKey = `piep-reader-v2-scroll-${downloadId}`;
  const renderedHtml = useMemo(() => htmlWithLocalAssets(doc?.html ?? ""), [doc?.html]);
  const source = doc?.download.source === "pixiv" ? "Pixiv" : "FANBOX";
  const url = doc ? sourceUrl(doc.download.source, doc.download.sourceId, doc.download.authorId) : null;
  const readingMinutes = estimateMinutes(doc?.plainText ?? "");

  useEffect(() => {
    if (!doc || !scrollRef.current) return;
    const saved = Number(localStorage.getItem(scrollKey));
    const frame = window.requestAnimationFrame(() => {
      const container = scrollRef.current;
      if (!container) return;
      if (Number.isFinite(saved) && saved > 0) {
        container.scrollTop = saved;
      }
      const max = Math.max(1, container.scrollHeight - container.clientHeight);
      setProgress(Math.round((container.scrollTop / max) * 100));
    });
    return () => window.cancelAnimationFrame(frame);
  }, [doc, scrollKey]);

  const handleScroll = () => {
    const container = scrollRef.current;
    if (!container) return;
    const max = Math.max(1, container.scrollHeight - container.clientHeight);
    const nextProgress = Math.round((container.scrollTop / max) * 100);
    setProgress(nextProgress);
    localStorage.setItem(scrollKey, String(container.scrollTop));
  };

  const updateFontSize = (value: number) => {
    const next = Math.max(14, Math.min(28, value));
    setFontSize(next);
    localStorage.setItem("piep-reader-v2-font-size", String(next));
  };

  const updateLineHeight = (value: number) => {
    const next = Math.max(1.35, Math.min(2.4, value));
    setLineHeight(next);
    localStorage.setItem("piep-reader-v2-line-height", String(next));
  };

  const updateTheme = (value: ReaderTheme) => {
    setTheme(value);
    localStorage.setItem("piep-reader-v2-theme", value);
  };

  const updateFont = (value: ReaderFont) => {
    setFontFamily(value);
    localStorage.setItem("piep-reader-v2-font", value);
  };

  const updateMeasure = (value: ReaderWidth) => {
    setMeasure(value);
    localStorage.setItem("piep-reader-v2-width", value);
  };

  const resetView = () => {
    updateFontSize(18);
    updateLineHeight(1.85);
    updateTheme("paper");
    updateFont("serif");
    updateMeasure("standard");
    showToast("読書設定を標準に戻しました", "success");
  };

  if (readerQuery.isLoading) {
    return (
      <div className="flex h-full flex-col bg-background">
        <div className="flex h-16 items-center gap-3 border-b px-4">
          <Skeleton className="size-9 rounded-md" />
          <div className="min-w-0 flex-1 space-y-2">
            <Skeleton className="h-3 w-28" />
            <Skeleton className="h-5 w-80 max-w-full" />
          </div>
          <Skeleton className="size-9 rounded-md" />
        </div>
        <div className="mx-auto w-full max-w-[820px] flex-1 space-y-4 px-8 py-10">
          <Skeleton className="h-7 w-3/4" />
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-[92%]" />
          <Skeleton className="h-4 w-[96%]" />
          <Skeleton className="h-4 w-[88%]" />
        </div>
      </div>
    );
  }

  if (readerQuery.isError || !doc) {
    return (
      <div className="flex h-full items-center justify-center bg-background p-6">
        <Card className="w-full max-w-md">
          <CardHeader>
            <CardTitle>読書データを読み込めませんでした</CardTitle>
          </CardHeader>
          <CardContent className="flex gap-2">
            <Button type="button" variant="outline" onClick={onBack}>
              <ArrowLeft className="size-4" />
              戻る
            </Button>
            <Button type="button" onClick={() => readerQuery.refetch()}>
              再読み込み
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className={cn("grid h-full min-h-0 grid-rows-[auto_1fr]", themeClass[theme])}>
      <header className="relative z-20 border-b border-black/10 bg-white/80 text-slate-950 shadow-sm backdrop-blur dark:border-white/10 dark:bg-slate-950/70 dark:text-slate-50">
        <Progress value={progress} className="h-1 rounded-none bg-transparent" />
        <div className="grid min-h-16 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 px-3 py-2 lg:px-4">
          <Button type="button" variant="ghost" size="icon" onClick={onBack} title="詳細へ戻る">
            <ArrowLeft className="size-4" />
          </Button>
          <div className="min-w-0">
            <div className="mb-1 flex min-w-0 items-center gap-2">
              <Badge variant="outline" className="shrink-0">{source}</Badge>
              {doc.isEdited ? <Badge className="shrink-0">編集版</Badge> : null}
              <span className="truncate text-xs text-muted-foreground">
                {doc.download.textLength.toLocaleString("ja-JP")} 字 / 約 {readingMinutes} 分 / {progress}%
              </span>
            </div>
            <h1 className="truncate text-sm font-semibold leading-tight tracking-normal lg:text-base">{doc.download.title}</h1>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="truncate text-xs text-muted-foreground hover:text-primary"
              onClick={() => showToast(doc.download.authorName, "info")}
            >
              {doc.download.authorName}
            </Button>
          </div>
          <div className="flex items-center gap-1.5">
            {url ? (
              <Button type="button" variant="ghost" size="icon" onClick={() => onOpenSourceUrl?.(url)} title="保存元を開く">
                <ExternalLink className="size-4" />
              </Button>
            ) : null}
            <Button type="button" variant="ghost" size="icon" onClick={onEdit} title="編集">
              <Edit3 className="size-4" />
            </Button>
            <Button
              type="button"
              variant={showSettings ? "secondary" : "ghost"}
              size="icon"
              onClick={() => setShowSettings(value => !value)}
              title="読書設定"
            >
              <Settings2 className="size-4" />
            </Button>
          </div>
        </div>
      </header>

      <div className="grid min-h-0 grid-cols-1 lg:grid-cols-[minmax(0,1fr)_auto]">
        <div ref={scrollRef} className="min-h-0 overflow-y-auto scroll-smooth" onScroll={handleScroll}>
          <article className={cn("mx-auto px-5 py-8 sm:px-8 lg:py-12", widthClass[measure])}>
            <div className="mb-8 border-b border-current/10 pb-6">
              <div className="mb-3 flex flex-wrap items-center gap-2 text-xs opacity-70">
                <BookOpenText className="size-4" />
                <span>{source}</span>
                <span>{doc.isEdited ? "編集版を表示中" : "原本を表示中"}</span>
              </div>
              <h2 className="text-balance text-2xl font-semibold leading-snug tracking-normal lg:text-3xl">
                {doc.download.title}
              </h2>
              <p className="mt-2 text-sm opacity-70">{doc.download.authorName}</p>
            </div>

            <div
              className={cn(
                "reader-prose whitespace-pre-wrap break-words",
                fontFamily === "serif" ? "font-serif" : "font-sans",
              )}
              style={{ fontSize, lineHeight }}
            >
              {renderedHtml ? (
                <div dangerouslySetInnerHTML={{ __html: renderedHtml }} />
              ) : (
                doc.plainText.split("\n").map((line, idx) => <p key={idx}>{line || "\u00A0"}</p>)
              )}
            </div>
          </article>
        </div>

        {showSettings ? (
          <aside className="min-h-0 w-full border-l border-black/10 bg-white/80 p-4 text-slate-950 shadow-xl backdrop-blur dark:border-white/10 dark:bg-slate-950/80 dark:text-slate-50 lg:w-80 lg:overflow-y-auto">
            <div className="mb-4 flex items-center justify-between gap-3">
              <div>
                <h2 className="text-sm font-semibold">読書設定</h2>
                <p className="text-xs text-muted-foreground">作品をまたいで保存されます</p>
              </div>
              <Button type="button" variant="ghost" size="icon" onClick={resetView} title="標準に戻す">
                <RotateCcw className="size-4" />
              </Button>
            </div>

            <div className="space-y-5">
              <SettingSlider
                label="文字サイズ"
                value={`${fontSize}px`}
                min={14}
                max={28}
                step={1}
                current={fontSize}
                onChange={updateFontSize}
                onMinus={() => updateFontSize(fontSize - 1)}
                onPlus={() => updateFontSize(fontSize + 1)}
              />
              <SettingSlider
                label="行間"
                value={lineHeight.toFixed(2)}
                min={1.35}
                max={2.4}
                step={0.05}
                current={lineHeight}
                onChange={updateLineHeight}
                onMinus={() => updateLineHeight(lineHeight - 0.05)}
                onPlus={() => updateLineHeight(lineHeight + 0.05)}
              />

              <SegmentedControl
                label="テーマ"
                value={theme}
                options={[
                  ["paper", themeLabels.paper],
                  ["white", themeLabels.white],
                  ["dark", themeLabels.dark],
                ]}
                onChange={value => updateTheme(value as ReaderTheme)}
              />
              <SegmentedControl
                label="書体"
                value={fontFamily}
                options={[
                  ["serif", "明朝"],
                  ["sans", "ゴシック"],
                ]}
                onChange={value => updateFont(value as ReaderFont)}
              />
              <SegmentedControl
                label="読み幅"
                value={measure}
                options={[
                  ["narrow", "狭め"],
                  ["standard", "標準"],
                  ["wide", "広め"],
                ]}
                onChange={value => updateMeasure(value as ReaderWidth)}
              />

              <Card className="bg-background/70">
                <CardContent className="space-y-2 p-3 text-xs text-muted-foreground">
                  <div className="flex items-center justify-between">
                    <span>読書進捗</span>
                    <strong className="text-foreground">{progress}%</strong>
                  </div>
                  <Progress value={progress} />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="w-full"
                    onClick={() => {
                      scrollRef.current?.scrollTo({ top: 0, behavior: "smooth" });
                    }}
                  >
                    先頭へ戻る
                  </Button>
                </CardContent>
              </Card>
            </div>
          </aside>
        ) : null}
      </div>
    </div>
  );
}

function SettingSlider({
  label,
  value,
  min,
  max,
  step,
  current,
  onChange,
  onMinus,
  onPlus,
}: {
  label: string;
  value: string;
  min: number;
  max: number;
  step: number;
  current: number;
  onChange: (value: number) => void;
  onMinus: () => void;
  onPlus: () => void;
}) {
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between text-sm">
        <span className="font-medium">{label}</span>
        <span className="text-xs text-muted-foreground">{value}</span>
      </div>
      <div className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2">
        <Button type="button" variant="outline" size="icon" className="size-8" onClick={onMinus}>
          <Minus className="size-3.5" />
        </Button>
        <Slider
          min={min}
          max={max}
          step={step}
          value={[current]}
          onValueChange={([next]) => onChange(next)}
          className="w-full"
        />
        <Button type="button" variant="outline" size="icon" className="size-8" onClick={onPlus}>
          <Plus className="size-3.5" />
        </Button>
      </div>
    </div>
  );
}

function SegmentedControl({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: Array<[string, string]>;
  onChange: (value: string) => void;
}) {
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-sm font-medium">
        <Type className="size-4 text-muted-foreground" />
        {label}
      </div>
      <div className="grid grid-cols-3 gap-1 rounded-md border bg-background p-1">
        {options.map(([optionValue, optionLabel]) => (
          <Button
            key={optionValue}
            type="button"
            variant={value === optionValue ? "default" : "ghost"}
            size="sm"
            onClick={() => onChange(optionValue)}
            className={cn(
              "min-h-8 rounded px-2 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground",
              value === optionValue && "bg-primary text-primary-foreground shadow-sm hover:bg-primary hover:text-primary-foreground",
            )}
          >
            {optionLabel}
          </Button>
        ))}
      </div>
    </div>
  );
}
