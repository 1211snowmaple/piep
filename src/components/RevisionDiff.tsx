import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Alert, Badge, Box, Group, SegmentedControl, Stack, Text } from "@mantine/core";
import { RevisionMetadataDiff } from "./RevisionMetadataDiff";
import type { RevisionMetadata } from "@/services/workInspectionApi";
import { compactTextDiff, createTextDiff } from "@/lib/textDiff";

interface RevisionDiffProps {
  beforeMetadata?: RevisionMetadata;
  afterMetadata?: RevisionMetadata;
  beforeVersion: number | null;
  afterVersion: number;
  beforeText: string | null;
  afterText: string;
  beforeLabel?: string;
  afterLabel?: string;
  actions?: ReactNode;
}

export function RevisionDiff({ beforeMetadata, afterMetadata, beforeVersion, afterVersion, beforeText, afterText, beforeLabel, afterLabel, actions }: RevisionDiffProps) {
  const [view, setView] = useState<"changes" | "text">("changes");
  const diff = useMemo(() => beforeText === null ? null : createTextDiff(beforeText, afterText), [afterText, beforeText]);
  const displayParts = useMemo(() => diff ? compactTextDiff(diff.parts) : [], [diff]);
  const resolvedBeforeLabel = beforeLabel ?? (beforeVersion === null ? null : `v${beforeVersion}`);
  const resolvedAfterLabel = afterLabel ?? `v${afterVersion}`;

  useEffect(() => setView("changes"), [afterVersion, beforeVersion, resolvedAfterLabel, resolvedBeforeLabel]);

  return <Stack gap="md">
    <Group justify="space-between" align="flex-start" gap="sm">
      <Box>
        <Text fw={700}>{resolvedBeforeLabel === null ? `${resolvedAfterLabel}の本文` : `${resolvedBeforeLabel} → ${resolvedAfterLabel} の変更`}</Text>
        {diff?.changed && <Group gap={6} mt={6} aria-label={`追加${diff.addedCharacters}字、削除${diff.removedCharacters}字`}>
          <Badge size="sm" color="green" variant="light">+{diff.addedCharacters.toLocaleString("ja-JP")}字</Badge>
          <Badge size="sm" color="red" variant="light">−{diff.removedCharacters.toLocaleString("ja-JP")}字</Badge>
          {diff.granularity === "range" && <Badge size="sm" color="gray" variant="light">大きな変更</Badge>}
        </Group>}
      </Box>
      <Group gap="xs" justify="flex-end">
        {beforeVersion !== null && <SegmentedControl
          size="xs"
          aria-label="履歴の表示方法"
          value={view}
          onChange={(value) => setView(value as "changes" | "text")}
          data={[{ value: "changes", label: "差分" }, { value: "text", label: `${resolvedAfterLabel}全文` }]}
        />}
        {actions}
      </Group>
    </Group>

    {beforeMetadata && afterMetadata && <RevisionMetadataDiff before={beforeMetadata} after={afterMetadata} />}

    {view === "text" || beforeVersion === null
      ? <div className="version-preview__text">{afterText || "本文がありません"}</div>
      : !diff?.changed
        ? <Alert color="gray">本文の変更はありません。</Alert>
        : <div className="revision-diff__content" aria-label={`${resolvedBeforeLabel}から${resolvedAfterLabel}への本文差分`}>
            {displayParts.map((part, index) => {
              if (part.kind === "added") return <ins key={index} className="revision-diff__part" data-kind="added">{part.value}</ins>;
              if (part.kind === "removed") return <del key={index} className="revision-diff__part" data-kind="removed">{part.value}</del>;
              if (part.kind === "omitted") return <span key={index} className="revision-diff__omission">{part.value}</span>;
              return <span key={index}>{part.value}</span>;
            })}
          </div>}
  </Stack>;
}
