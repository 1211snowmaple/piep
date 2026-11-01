import { useCallback, useRef, useState } from "react";
import { Alert, Badge, Button, Drawer, Group, Loader, Paper, Progress, Stack, Text } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { errorMessage } from "@/lib/format";
import {
  PROOFREAD_BATCH_CHARS,
  PROOFREAD_BATCH_SEGMENTS,
  proofreadSegments,
  type AssistEngine,
  type ProofreadSegment,
  type ProofreadSuggestion,
} from "@/services/assistApi";

/** 校正に回すのは、文字を持つブロックだけ。URL やキャプションは送らない。 */
const TEXT_BLOCKS = new Set(["paragraph", "heading", "quote"]);

export interface ProofreadSource {
  clientId: string;
  blockType: string;
  text?: string | null;
}

/**
 * 送る単位に切り分ける。
 *
 * 一つのブロックが上限より長いときは、行の切れ目で分けて送る。どれも
 * 同じブロックの身元を名乗るので、当てるときはブロックの本文全体の中で
 * 場所を一つに決め直す。
 */
export function buildProofreadBatches(blocks: ProofreadSource[], limit = PROOFREAD_BATCH_CHARS): ProofreadSegment[][] {
  const segments: ProofreadSegment[] = [];
  for (const block of blocks) {
    if (!TEXT_BLOCKS.has(block.blockType)) continue;
    const text = String(block.text ?? "");
    if (!text.trim()) continue;
    let piece = "";
    for (const line of text.split("\n")) {
      // 一行だけで上限を超えるものは、そのまま一つの単位にする（切ると語が割れる）。
      if (piece && piece.length + line.length + 1 > limit) {
        segments.push({ id: block.clientId, text: piece });
        piece = "";
      }
      piece = piece ? `${piece}\n${line}` : line;
    }
    if (piece.trim()) segments.push({ id: block.clientId, text: piece });
  }
  const batches: ProofreadSegment[][] = [];
  let batch: ProofreadSegment[] = [];
  let size = 0;
  for (const segment of segments) {
    const length = Math.min(segment.text.length, limit);
    if (batch.length && (size + length > limit || batch.length >= PROOFREAD_BATCH_SEGMENTS)) {
      batches.push(batch);
      batch = [];
      size = 0;
    }
    batch.push({ ...segment, text: segment.text.slice(0, limit) });
    size += length;
  }
  if (batch.length) batches.push(batch);
  return batches;
}

/** `before` が本文にちょうど一度だけ現れるときだけ置き換える。決まらなければ `null`。 */
export function replaceOnce(text: string, before: string, after: string): string | null {
  const first = text.indexOf(before);
  if (!before || first < 0 || text.indexOf(before, first + 1) >= 0) return null;
  return text.slice(0, first) + after + text.slice(first + before.length);
}

export type ProofreadStatus = "open" | "applied" | "dismissed" | "stale";
export interface ProofreadItem extends ProofreadSuggestion {
  key: string;
  status: ProofreadStatus;
}

/**
 * 校正を回す。止めると、送っている途中の一回の答えは捨て、そこで終える。
 *
 * 長い作品では何十回も問い合わせる。先に返ってきた分から順に並べ、
 * 終わりを待たずに直し始められるようにする。
 */
export function useProofread() {
  const [items, setItems] = useState<ProofreadItem[]>([]);
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const runRef = useRef(0);

  const start = useCallback(async (engine: AssistEngine, batches: ProofreadSegment[][]) => {
    const run = ++runRef.current;
    setItems([]);
    setError(null);
    setProgress({ done: 0, total: batches.length });
    setRunning(true);
    try {
      for (const [index, batch] of batches.entries()) {
        const found = await proofreadSegments(engine, batch);
        if (runRef.current !== run) return;
        setItems((current) => {
          const seen = new Set(current.map((item) => `${item.segmentId}\u0001${item.before}`));
          const fresh = found
            .filter((item) => !seen.has(`${item.segmentId}\u0001${item.before}`))
            .map((item, offset) => ({ ...item, key: `${run}:${index}:${offset}`, status: "open" as const }));
          return [...current, ...fresh];
        });
        setProgress({ done: index + 1, total: batches.length });
      }
    } catch (caught) {
      if (runRef.current === run) setError(errorMessage(caught));
    } finally {
      if (runRef.current === run) setRunning(false);
    }
  }, []);
  const cancel = useCallback(() => {
    runRef.current += 1;
    setRunning(false);
  }, []);
  const mark = useCallback((keys: ReadonlySet<string>, status: ProofreadStatus) => {
    if (!keys.size) return;
    setItems((current) => current.map((item) => keys.has(item.key) ? { ...item, status } : item));
  }, []);
  const clear = useCallback(() => {
    runRef.current += 1;
    setRunning(false);
    setItems([]);
    setError(null);
    setProgress({ done: 0, total: 0 });
  }, []);
  return { items, progress, running, error, start, cancel, mark, clear };
}

/** 変わる部分だけを強めて見せる。前後が同じ長い文字列を、目で突き合わせなくて済む。 */
function DiffText({ before, after }: { before: string; after: string }) {
  let prefix = 0;
  while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix]) prefix += 1;
  let suffix = 0;
  while (suffix < before.length - prefix && suffix < after.length - prefix && before[before.length - 1 - suffix] === after[after.length - 1 - suffix]) suffix += 1;
  const head = before.slice(0, prefix);
  const tail = before.slice(before.length - suffix);
  const removed = before.slice(prefix, before.length - suffix);
  const added = after.slice(prefix, after.length - suffix);
  return (
    <Text size="sm" className="proofread-diff">
      {head}
      {removed && <del>{removed}</del>}
      {added && <ins>{added}</ins>}
      {tail}
    </Text>
  );
}

const STATUS_LABEL: Record<Exclude<ProofreadStatus, "open">, string> = {
  applied: "直しました",
  dismissed: "見送りました",
  stale: "本文が変わったため当てられません",
};

export function ProofreadPanel({
  opened,
  onClose,
  items,
  progress,
  running,
  error,
  blockNumber,
  onApply,
  onDismiss,
  onJump,
  onCancel,
}: {
  opened: boolean;
  onClose: () => void;
  items: ProofreadItem[];
  progress: { done: number; total: number };
  running: boolean;
  error: string | null;
  blockNumber: (segmentId: string) => number | null;
  onApply: (items: ProofreadItem[]) => void;
  onDismiss: (item: ProofreadItem) => void;
  onJump: (segmentId: string) => void;
  onCancel: () => void;
}) {
  const open = items.filter((item) => item.status === "open");
  const finished = !running && progress.total > 0 && progress.done === progress.total;
  return (
    <Drawer
      opened={opened}
      onClose={onClose}
      position="right"
      size={420}
      withOverlay={false}
      lockScroll={false}
      trapFocus={false}
      title={<Group gap="xs"><Icons.optimize size={IconSize.action} /><Text fw={750}>本文の校正</Text><Badge size="xs" variant="light" color="yellow" tt="none">β</Badge></Group>}
    >
      <Stack gap="sm">
        <Text size="xs" c="dimmed">誤字・脱字・変換ミスの候補です。本文は、押したものだけが変わります。直したあとも「元に戻す」で取り消せます。</Text>
        {progress.total > 0 && (
          <Stack gap={4}>
            <Group justify="space-between">
              <Group gap={6}>{running && <Loader size={12} />}<Text size="xs" c="dimmed">{running ? `読んでいます（${progress.done}/${progress.total}）` : finished ? `読み終えました（${progress.total}回）` : `途中で止めました（${progress.done}/${progress.total}）`}</Text></Group>
              {running && <Button size="compact-xs" variant="subtle" color="gray" onClick={onCancel}>止める</Button>}
            </Group>
            <Progress size="xs" value={progress.total ? (progress.done / progress.total) * 100 : 0} animated={running} />
          </Stack>
        )}
        {error && <Alert color="red" variant="light" p="xs" title="校正を続けられません"><Text size="xs">{error}</Text></Alert>}
        {open.length > 1 && <Group justify="flex-end"><Button size="xs" variant="light" leftSection={<Icons.confirm size={IconSize.menu} />} onClick={() => onApply(open)}>残り{open.length}件をすべて直す</Button></Group>}
        {finished && !items.length && !error && <Text size="sm" c="dimmed" ta="center" py="md">直す候補は見つかりませんでした。</Text>}
        {items.map((item) => {
          const number = blockNumber(item.segmentId);
          return (
            <Paper key={item.key} withBorder p="xs" className="proofread-item" data-status={item.status}>
              <Group justify="space-between" wrap="nowrap" mb={4}>
                <Group gap={6} wrap="nowrap"><Badge size="xs" variant="light" color="grape">{item.kind}</Badge>{number !== null && <Text size="10px" c="dimmed">ブロック {number}</Text>}</Group>
                <Button size="compact-xs" variant="subtle" color="gray" disabled={number === null} onClick={() => onJump(item.segmentId)}>その場所へ</Button>
              </Group>
              <DiffText before={item.before} after={item.after} />
              {item.reason && <Text size="xs" c="dimmed" mt={2}>{item.reason}</Text>}
              {item.status === "open"
                ? <Group gap="xs" mt="xs" justify="flex-end"><Button size="compact-xs" variant="default" onClick={() => onDismiss(item)}>見送る</Button><Button size="compact-xs" onClick={() => onApply([item])}>直す</Button></Group>
                : <Text size="xs" mt="xs" c={item.status === "stale" ? "yellow.8" : "dimmed"} ta="right">{STATUS_LABEL[item.status]}</Text>}
            </Paper>
          );
        })}
      </Stack>
    </Drawer>
  );
}
