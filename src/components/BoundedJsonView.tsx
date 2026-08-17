import { useEffect, useMemo, useState } from "react";
import { Button, CopyButton, Group, Paper, Stack, Text } from "@mantine/core";
import { formatNumber } from "@/lib/format";

export const JSON_INITIAL_RENDER_CHARS = 50_000;
export const JSON_RENDER_STEP_CHARS = 50_000;
export const JSON_MAX_RENDER_CHARS = 300_000;

export function stringifyJson(value: unknown): string {
  try {
    return JSON.stringify(value ?? {}, null, 2) ?? "{}";
  } catch (error) {
    return `JSONを表示できません: ${error instanceof Error ? error.message : String(error)}`;
  }
}

/**
 * Stringifies once, then places only a bounded prefix in the DOM. The full
 * value stays available through copy without making the browser lay out a
 * multi-megabyte <pre> node.
 */
export function BoundedJsonView({ value }: { value: unknown }) {
  const json = useMemo(() => stringifyJson(value), [value]);
  const [renderLimit, setRenderLimit] = useState(JSON_INITIAL_RENDER_CHARS);

  useEffect(() => setRenderLimit(JSON_INITIAL_RENDER_CHARS), [json]);

  const visibleLength = Math.min(json.length, renderLimit, JSON_MAX_RENDER_CHARS);
  const visibleJson = json.slice(0, visibleLength);
  const canShowMore = visibleLength < json.length && visibleLength < JSON_MAX_RENDER_CHARS;
  const capped = json.length > JSON_MAX_RENDER_CHARS && visibleLength >= JSON_MAX_RENDER_CHARS;

  return (
    <Stack gap="sm">
      <Group justify="space-between" align="flex-start">
        <Text size="sm" c="dimmed">取得したプロフィール情報を折り返して表示します（{formatNumber(json.length)}文字）。</Text>
        <CopyButton value={json}>{({ copied, copy }) => <Button size="xs" variant="light" onClick={copy}>{copied ? "コピー済み" : "全体をコピー"}</Button>}</CopyButton>
      </Group>
      <Paper className="json-view" withBorder>
        <pre>{visibleJson}{visibleLength < json.length ? `\n\n… 残り${formatNumber(json.length - visibleLength)}文字は省略されています` : ""}</pre>
      </Paper>
      {canShowMore && (
        <Group justify="center">
          <Button variant="default" size="xs" onClick={() => setRenderLimit((current) => Math.min(current + JSON_RENDER_STEP_CHARS, JSON_MAX_RENDER_CHARS))}>
            さらに表示
          </Button>
        </Group>
      )}
      {capped && <Text size="xs" c="dimmed" ta="center">表示性能を保つため先頭{formatNumber(JSON_MAX_RENDER_CHARS)}文字まで表示しています。全体はコピーできます。</Text>}
    </Stack>
  );
}
