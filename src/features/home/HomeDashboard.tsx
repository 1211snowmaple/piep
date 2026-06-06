import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { PieChart, Pie, Cell, ResponsiveContainer, Tooltip } from "recharts";
import {
  Activity,
  BookOpen,
  Clock3,
  Database,
  Gauge,
  Heart,
  Library,
  Palette,
  RefreshCw,
  Search,
  Settings,
  Star,
  Tags,
  Users,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";
import { getDashboardSummary, isTauriRuntime } from "@/services/dbApi";
import type { DashboardSummary, DbStats } from "@/types/library";
import { cn } from "@/lib/utils";

type LibrarySource = "" | "pixiv" | "fanbox" | "favorite";
type FeatureDestination = "epub" | "update" | "pixiv" | "fanbox" | "settings";

interface HomeDashboardProps {
  fallbackStats: DbStats | null;
  onOpenLibrary: (source?: LibrarySource) => void;
  onOpenFeature: (destination: FeatureDestination) => void;
  onOpenWork?: (downloadId: number) => void;
  onSelectTag?: (tag: string) => void;
  onSelectAuthor?: (author: string) => void;
}

function formatNumber(value: number): string {
  return value.toLocaleString("ja-JP");
}

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatMonth(bucket: string): string {
  const [, month] = bucket.split("-");
  return month ? `${Number(month)}月` : bucket;
}

function buildFallbackSummary(stats: DbStats | null): DashboardSummary {
  const safeStats = stats ?? {
    totalDownloads: 0,
    pixivCount: 0,
    fanboxCount: 0,
    totalAssets: 0,
    totalSizeBytes: 0,
  };

  return {
    stats: safeStats,
    favoriteCount: 0,
    watchedCount: 0,
    updateTargetCount: 0,
    indexedCount: 0,
    pendingIndexCount: 0,
    topTags: [],
    topAuthors: [],
    recentDownloads: [],
    sourceBreakdown: [
      { source: "pixiv", count: safeStats.pixivCount, totalSizeBytes: 0 },
      { source: "fanbox", count: safeStats.fanboxCount, totalSizeBytes: 0 },
    ].filter(item => item.count > 0),
    monthlyDownloads: [],
  };
}

export function HomeDashboard({
  fallbackStats,
  onOpenLibrary,
  onOpenFeature,
  onOpenWork,
  onSelectTag,
  onSelectAuthor,
}: HomeDashboardProps) {
  const tauriReady = isTauriRuntime();
  const dashboardQuery = useQuery({
    queryKey: ["dashboard-summary"],
    queryFn: getDashboardSummary,
    enabled: tauriReady,
  });

  const summary = dashboardQuery.data ?? buildFallbackSummary(fallbackStats);
  const indexTotal = summary.indexedCount + summary.pendingIndexCount;
  const indexProgress = indexTotal > 0 ? Math.round((summary.indexedCount / indexTotal) * 100) : 100;
  
  const chartTotal = summary.stats.pixivCount + summary.stats.fanboxCount;
  const pieData = useMemo(() => [
    { name: "Pixiv", value: summary.stats.pixivCount, color: "#0096fa" },
    { name: "FANBOX", value: summary.stats.fanboxCount, color: "#f2c624" },
  ], [summary.stats.pixivCount, summary.stats.fanboxCount]);
  const totalDownloads = summary.stats.totalDownloads;
  const hasLibrary = totalDownloads > 0;

  const monthlyData = useMemo(
    () => summary.monthlyDownloads.slice(-8).map(item => ({ ...item, label: formatMonth(item.bucket) })),
    [summary.monthlyDownloads],
  );
  const maxMonthlyCount = Math.max(1, ...monthlyData.map(item => item.count));

  const sourceRows = [
    { label: "Pixiv", source: "pixiv" as const, count: summary.stats.pixivCount, icon: Palette, color: "bg-sky-500" },
    { label: "FANBOX", source: "fanbox" as const, count: summary.stats.fanboxCount, icon: Heart, color: "bg-yellow-500" },
  ];

  const metricCards = [
    {
      label: "作品",
      value: formatNumber(totalDownloads),
      detail: `${formatNumber(summary.stats.totalAssets)} assets`,
      icon: Library,
      onClick: () => onOpenLibrary(""),
    },
    {
      label: "お気に入り",
      value: formatNumber(summary.favoriteCount),
      detail: "すぐ読み返す",
      icon: Star,
      onClick: () => onOpenLibrary("favorite"),
    },
    {
      label: "更新監視",
      value: formatNumber(summary.watchedCount),
      detail: `${formatNumber(summary.updateTargetCount)} targets`,
      icon: RefreshCw,
      onClick: () => onOpenFeature("update"),
    },
    {
      label: "容量",
      value: formatBytes(summary.stats.totalSizeBytes),
      detail: totalDownloads > 0 ? `平均 ${formatBytes(summary.stats.totalSizeBytes / totalDownloads)}` : "平均 0 B",
      icon: Database,
      onClick: () => onOpenLibrary(""),
    },
  ];

  const workflowActions = [
    { label: "一覧で探す", detail: "検索、タグ、作者、シリーズ", icon: Search, onClick: () => onOpenLibrary("") },
    { label: "EPUBにまとめる", detail: "選択とテンプレート設定", icon: BookOpen, onClick: () => onOpenFeature("epub") },
    { label: "更新を見る", detail: "監視対象と新着確認", icon: RefreshCw, onClick: () => onOpenFeature("update") },
    { label: "連携を確認", detail: "Pixiv / FANBOX 認証", icon: Settings, onClick: () => onOpenFeature("settings") },
  ];

  return (
    <div className="mx-auto flex w-full max-w-[1240px] flex-col gap-6 px-8 py-8 md:px-16 md:py-10 lg:px-24 text-foreground">
      <div className="flex min-h-0 flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-xs font-medium uppercase text-muted-foreground">
            <Gauge className="size-4" />
            Dashboard
            {dashboardQuery.isFetching ? <span className="text-primary">syncing</span> : null}
          </div>
          <h1 className="mt-0.5 truncate text-2xl font-semibold tracking-normal">ホーム</h1>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant={summary.pendingIndexCount > 0 ? "secondary" : "outline"}>
            Index {indexProgress}%
          </Badge>
          <Badge variant="outline">{formatBytes(summary.stats.totalSizeBytes)}</Badge>
          {!tauriReady ? <Badge variant="secondary">Preview</Badge> : null}
        </div>
      </div>

      {dashboardQuery.isError ? (
        <Card className="border-destructive/40 bg-destructive/5">
          <CardContent className="flex items-center gap-3 p-3 text-sm text-destructive">
            <Activity className="size-4" />
            ダッシュボード統計の読み込みに失敗しました。既存統計で表示しています。
          </CardContent>
        </Card>
      ) : null}

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1.2fr)_minmax(280px,0.8fr)]">
        <div className="grid gap-4">
          <div className="grid gap-3 grid-cols-2 md:grid-cols-4">
            {metricCards.map(metric => {
              const Icon = metric.icon;
              return (
                <Button
                  type="button"
                  key={metric.label}
                  variant="ghost"
                  onClick={metric.onClick}
                  className="group h-auto min-w-0 justify-start rounded-lg border bg-card p-3 text-left shadow-sm transition-colors hover:border-primary/50 hover:bg-accent/45 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <div className="flex w-full items-start justify-between gap-3">
                    <div className="min-w-0">
                      <p className="truncate text-xs text-muted-foreground">{metric.label}</p>
                      <p className="mt-1 truncate text-xl font-semibold tracking-normal">{metric.value}</p>
                      <p className="mt-0.5 truncate text-xs text-muted-foreground">{metric.detail}</p>
                    </div>
                    <span className="rounded-md border bg-background p-2 text-muted-foreground transition-colors group-hover:text-primary">
                      <Icon className="size-4" />
                    </span>
                  </div>
                </Button>
              );
            })}
          </div>

          <div className="grid gap-4 lg:grid-cols-[minmax(0,1.1fr)_minmax(280px,0.9fr)]">
            <Card className="overflow-hidden">
              <CardHeader className="flex-row items-center justify-between space-y-0 p-4 pb-2">
                <div>
                  <CardTitle className="text-sm">保存ペース</CardTitle>
                  <p className="text-xs text-muted-foreground">直近8か月</p>
                </div>
                <Button type="button" variant="outline" size="sm" onClick={() => onOpenLibrary("")}>
                  開く
                </Button>
              </CardHeader>
              <CardContent className="p-4 pt-2">
                {monthlyData.length > 0 ? (
                  <div className="flex h-44 items-end gap-2">
                    {monthlyData.map(item => (
                      <Button
                        type="button"
                        key={item.bucket}
                        variant="ghost"
                        className="group flex h-auto min-w-0 flex-1 flex-col items-center gap-2 p-0"
                        onClick={() => onOpenLibrary("")}
                        title={`${item.label}: ${formatNumber(item.count)}件`}
                      >
                        <div className="flex h-32 w-full items-end rounded bg-muted/50 px-1">
                          <div
                            className="w-full rounded-t bg-primary/75 transition-colors group-hover:bg-primary"
                            style={{ height: `${Math.max(8, (item.count / maxMonthlyCount) * 100)}%` }}
                          />
                        </div>
                        <span className="truncate text-[11px] text-muted-foreground">{item.label}</span>
                      </Button>
                    ))}
                  </div>
                ) : (
                  <EmptyPanel label={hasLibrary ? "月次データを集計中です" : "保存した作品がまだありません"} dense />
                )}
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="p-4 pb-2">
                <CardTitle className="text-sm">ライブラリ作業</CardTitle>
                <p className="text-xs text-muted-foreground">一覧、EPUB、更新を同じ場所から開きます</p>
              </CardHeader>
              <CardContent className="grid gap-2 p-4 pt-2">
                {workflowActions.map(action => {
                  const Icon = action.icon;
                  return (
                    <Button
                      type="button"
                      key={action.label}
                      variant="ghost"
                      onClick={action.onClick}
                      className="flex h-auto min-w-0 justify-start items-center gap-3 rounded-md border bg-background p-2.5 text-left transition-colors hover:border-primary/50 hover:bg-accent"
                    >
                      <span className="rounded-md bg-secondary p-2 text-secondary-foreground">
                        <Icon className="size-4" />
                      </span>
                      <span className="min-w-0">
                        <span className="block truncate text-sm font-medium">{action.label}</span>
                        <span className="block truncate text-xs text-muted-foreground">{action.detail}</span>
                      </span>
                    </Button>
                  );
                })}
              </CardContent>
            </Card>
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <FacetPanel
              title="人気タグ"
              emptyLabel="タグ統計はまだありません"
              icon={Tags}
              items={summary.topTags.slice(0, 100)}
              limitLabel="上位100件"
              onClickItem={onSelectTag}
            />
            <FacetPanel
              title="作者"
              emptyLabel="作者統計はまだありません"
              icon={Users}
              items={summary.topAuthors.slice(0, 100)}
              limitLabel="上位100件"
              onClickItem={onSelectAuthor}
            />
          </div>
        </div>

        <div className="grid gap-4">
          <Card>
            <CardHeader className="p-4 pb-2">
              <CardTitle className="text-sm">状態</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4 p-4 pt-2">
              <div className="flex items-center justify-between gap-4 min-h-[48px]">
                <div className="shrink-0 flex flex-col justify-center">
                  <p className="text-2xl font-bold tracking-normal leading-tight">{indexProgress}%</p>
                  <p className="text-xs text-muted-foreground mt-1 leading-normal font-semibold">
                    {formatNumber(summary.indexedCount)} / {formatNumber(indexTotal)} indexed
                  </p>
                </div>
                <div className="flex-1 px-1">
                  <Progress value={indexProgress} className="h-2.5 w-full" />
                </div>
                <Activity className={cn("size-4 text-muted-foreground shrink-0", summary.pendingIndexCount > 0 && "text-primary")} />
              </div>

              {chartTotal > 0 && (
                <div className="flex justify-center py-2">
                  <div className="relative w-28 h-28 shrink-0 flex items-center justify-center bg-card rounded-full shadow-inner border border-muted/20">
                    <ResponsiveContainer width="100%" height="100%">
                      <PieChart>
                        <Pie
                          data={pieData}
                          cx="50%"
                          cy="50%"
                          innerRadius={24}
                          outerRadius={38}
                          paddingAngle={3}
                          dataKey="value"
                        >
                          {pieData.map((entry, index) => (
                            <Cell key={`cell-${index}`} fill={entry.color} />
                          ))}
                        </Pie>
                        <Tooltip
                          contentStyle={{
                            backgroundColor: "hsl(var(--popover))",
                            borderColor: "hsl(var(--border))",
                            borderRadius: "var(--radius)",
                            fontSize: "11px",
                            color: "hsl(var(--popover-foreground))",
                          }}
                        />
                      </PieChart>
                    </ResponsiveContainer>
                    <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
                      <span className="text-[8px] text-muted-foreground uppercase leading-none font-semibold">Total</span>
                      <span className="text-xs font-bold mt-0.5 leading-none">{formatNumber(chartTotal)}</span>
                    </div>
                  </div>
                </div>
              )}

              <div className="grid grid-cols-2 gap-2">
                {sourceRows.map(row => {
                  const Icon = row.icon;
                  const pct = totalDownloads > 0 ? Math.round((row.count / totalDownloads) * 100) : 0;
                  return (
                    <Button
                      type="button"
                      key={row.source}
                      variant="ghost"
                      onClick={() => onOpenLibrary(row.source)}
                      className="h-auto justify-start rounded-md border bg-background p-2 text-left transition-colors hover:border-primary/50 hover:bg-accent"
                    >
                      <div className="mb-1 flex items-center gap-2 text-xs text-muted-foreground">
                        <Icon className="size-3.5" />
                        {row.label}
                      </div>
                      <div className="flex items-center justify-between gap-2 text-sm font-semibold">
                        <span>{formatNumber(row.count)}</span>
                        <span>{pct}%</span>
                      </div>
                      <div className="mt-2 h-1.5 rounded-full bg-secondary">
                        <div className={cn("h-full rounded-full", row.color)} style={{ width: `${pct}%` }} />
                      </div>
                    </Button>
                  );
                })}
              </div>
            </CardContent>
          </Card>

          <Card className="overflow-hidden">
            <CardHeader className="flex-row items-center justify-between space-y-0 p-4 pb-2">
              <div>
                <CardTitle className="text-sm">最近の保存</CardTitle>
                <p className="text-xs text-muted-foreground">クリックで作品へ移動</p>
              </div>
              <Clock3 className="size-4 text-muted-foreground" />
            </CardHeader>
            <CardContent className="min-h-0 p-4 pt-2">
              {dashboardQuery.isLoading ? (
                <div className="space-y-2">
                  <Skeleton className="h-11 w-full" />
                  <Skeleton className="h-11 w-full" />
                  <Skeleton className="h-11 w-full" />
                </div>
              ) : summary.recentDownloads.length > 0 ? (
                <div className="flex max-h-[320px] flex-col gap-2 overflow-y-auto pr-1 scrollbar-thin">
                  {summary.recentDownloads.slice(0, 30).map(item => (
                    <Button
                      type="button"
                      key={item.id}
                      variant="ghost"
                      onClick={() => onOpenWork ? onOpenWork(item.id) : onOpenLibrary(item.source === "pixiv" || item.source === "fanbox" ? item.source : "")}
                      className="flex h-auto w-full min-w-0 justify-between gap-3 rounded-md border bg-background px-3 py-2 text-left text-sm transition-colors hover:border-primary/50 hover:bg-accent"
                    >
                      <span className="min-w-0">
                        <span className="block truncate font-medium">{item.title}</span>
                        <span className="block truncate text-xs text-muted-foreground">{item.authorName}</span>
                      </span>
                      <Badge variant="outline" className="shrink-0">{item.source}</Badge>
                    </Button>
                  ))}
                </div>
              ) : (
                <EmptyPanel label="最近の保存はまだありません" dense />
              )}
            </CardContent>
          </Card>
        </div>
      </div>

      {!tauriReady ? (
        <Card className="border-dashed">
          <CardContent className="flex items-center gap-3 p-3 text-sm text-muted-foreground">
            <Clock3 className="size-4" />
            デスクトップアプリ実行時に保存データと統計が接続されます。
          </CardContent>
        </Card>
      ) : null}
    </div>
  );
}

function EmptyPanel({ label, dense = false }: { label: string; dense?: boolean }) {
  return (
    <div className={cn("flex items-center justify-center rounded-md border border-dashed bg-muted/30 p-4 text-sm text-muted-foreground", dense ? "min-h-24" : "min-h-32")}>
      {label}
    </div>
  );
}

function FacetPanel({
  title,
  emptyLabel,
  icon: Icon,
  items,
  limitLabel,
  onClickItem,
}: {
  title: string;
  emptyLabel: string;
  icon: typeof Tags;
  items: Array<{ name: string; count: number }>;
  limitLabel?: string;
  onClickItem?: (name: string) => void;
}) {
  return (
    <Card className="min-h-0">
      <CardHeader className="flex-row items-center justify-between space-y-0 p-4 pb-2">
        <div>
          <CardTitle className="text-sm">{title}</CardTitle>
          <p className="text-xs text-muted-foreground">{limitLabel || "上位10件"}</p>
        </div>
        <Icon className="size-4 text-muted-foreground" />
      </CardHeader>
      <CardContent className="p-4 pt-2">
        {items.length > 0 ? (
          <div className="flex max-h-[140px] flex-wrap gap-2 overflow-y-auto pr-1 scrollbar-thin">
            {items.map(item => (
              <Badge
                key={item.name}
                variant="secondary"
                className={cn(
                  "max-w-full gap-1 transition-colors hover:bg-primary/10 hover:text-primary",
                  onClickItem && "cursor-pointer"
                )}
                onClick={() => onClickItem?.(item.name)}
              >
                <span className="max-w-40 truncate">{item.name}</span>
                <span className="text-muted-foreground">{formatNumber(item.count)}</span>
              </Badge>
            ))}
          </div>
        ) : (
          <EmptyPanel label={emptyLabel} dense />
        )}
      </CardContent>
    </Card>
  );
}
