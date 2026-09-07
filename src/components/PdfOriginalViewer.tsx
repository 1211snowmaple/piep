import { useEffect, useRef, useState } from "react";
import { Alert, Button, Group, Loader, Modal, Stack, Text } from "@mantine/core";
import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { renderPdfAttachmentPage, type PdfPageImage } from "@/services/dbApi";
import { errorMessage } from "@/lib/format";

export interface PdfOriginalTarget {
  downloadId: number;
  localPath: string;
  fileName: string;
}

/**
 * 描く幅を刻む。
 *
 * 窓の幅をそのまま渡すと、掴んで広げるあいだ1画素ごとに描き直しを頼むことに
 * なる。**200 画素に丸めれば、見た目は変わらないまま要求は数回で収まる。**
 */
function steppedWidth(available: number): number {
  const dense = available * Math.min(2, window.devicePixelRatio || 1);
  return Math.min(2400, Math.max(320, Math.ceil(dense / 200) * 200));
}

/** 描いた紙面を持っておく時間。同じ添付を行き来しても描き直さない。 */
const PAGE_CACHE_MS = 5 * 60 * 1000;

function pageKey(target: PdfOriginalTarget | null, page: number, width: number) {
  return ["pdf-page", target?.downloadId ?? 0, target?.localPath ?? "", page, width] as const;
}

/**
 * 添付 PDF の原本を、紙面のまま見る。
 *
 * 本文は取り込んで読めるようにしてあるが、それは段落を組み直したものである。
 * 図版のある PDF はそこに載らないし、組み直しを疑いたいこともある。
 * **原本と引き比べられなければ、取り込みが正しいかを利用者が確かめられない。**
 */
export function PdfOriginalViewer({ target, onClose }: { target: PdfOriginalTarget | null; onClose: () => void }) {
  const [page, setPage] = useState(0);
  const [width, setWidth] = useState(1200);
  const frameRef = useRef<HTMLDivElement | null>(null);
  const queryClient = useQueryClient();

  // 別の添付を開いたら最初のページへ戻る。前の作品の続きから始まらないように。
  useEffect(() => setPage(0), [target?.localPath]);

  useEffect(() => {
    const frame = frameRef.current;
    if (!frame || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const measured = entries[0]?.contentRect.width ?? 0;
      if (measured > 0) setWidth(steppedWidth(measured));
    });
    observer.observe(frame);
    return () => observer.disconnect();
  }, [target?.localPath]);

  const pageQuery = useQuery({
    queryKey: pageKey(target, page, width),
    queryFn: () => renderPdfAttachmentPage(target!.downloadId, target!.localPath, page, width),
    enabled: Boolean(target),
    // 送るたびに真っ白へ落とすと、紙面の位置を見失う。前のページを出したまま差し替える。
    placeholderData: keepPreviousData,
    staleTime: PAGE_CACHE_MS,
  });

  const pageCount = pageQuery.data?.pageCount ?? 0;
  const canGoBack = page > 0;
  const canGoOn = pageCount > 0 && page + 1 < pageCount;

  // 次のページを先に描いておく。1ページ 90ms 前後かかるので、押してから
  // 描き始めると送りが引っかかる。
  useEffect(() => {
    if (!target || !canGoOn) return;
    void queryClient.prefetchQuery({
      queryKey: pageKey(target, page + 1, width),
      queryFn: () => renderPdfAttachmentPage(target.downloadId, target.localPath, page + 1, width),
      staleTime: PAGE_CACHE_MS,
    });
  }, [target, canGoOn, page, width, queryClient]);

  useEffect(() => {
    if (!target) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowRight" && canGoOn) setPage((value) => value + 1);
      if (event.key === "ArrowLeft" && canGoBack) setPage((value) => Math.max(0, value - 1));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [target, canGoBack, canGoOn]);

  const shown: PdfPageImage | undefined = pageQuery.data;
  return (
    <Modal opened={Boolean(target)} onClose={onClose} size="xl" title={target?.fileName ?? ""} classNames={{ title: "pdf-original-title" }}>
      <Stack gap="sm">
        <div className="pdf-original-frame" ref={frameRef}>
          {pageQuery.error ? (
            <Alert color="red" title="このページを描けません">{errorMessage(pageQuery.error)}</Alert>
          ) : shown ? (
            <img
              className="pdf-original-page"
              src={shown.image}
              alt={`${target?.fileName ?? "添付"} の ${page + 1} ページ目`}
              width={shown.width}
              height={shown.height}
            />
          ) : (
            <Group justify="center" py="xl"><Loader size="sm" /></Group>
          )}
        </div>
        <Group justify="space-between">
          <Button variant="default" size="xs" disabled={!canGoBack} onClick={() => setPage((value) => Math.max(0, value - 1))} leftSection={<Icons.previous size={IconSize.inline} />}>前のページ</Button>
          <Text size="xs" c="dimmed">
            {pageCount > 0 ? `${page + 1} / ${pageCount} ページ` : "読み込んでいます"}
            {pageQuery.isFetching && pageCount > 0 ? "・描いています" : ""}
          </Text>
          <Button variant="default" size="xs" disabled={!canGoOn} onClick={() => setPage((value) => value + 1)} rightSection={<Icons.next size={IconSize.inline} />}>次のページ</Button>
        </Group>
      </Stack>
    </Modal>
  );
}
