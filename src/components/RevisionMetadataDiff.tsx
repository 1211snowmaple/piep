import { Alert, Badge, Box, Group, Image, SimpleGrid, Stack, Text } from "@mantine/core";
import { useMemo } from "react";
import { compareRevisionMetadata, type MetadataChange } from "@/lib/revisionMetadataDiff";
import { formatDate } from "@/lib/format";
import { createTextDiff } from "@/lib/textDiff";
import { getAssetUrl } from "@/services/dbApi";
import type { RevisionMetadata } from "@/services/workInspectionApi";

function MetadataField({ field }: { field: MetadataChange }) {
  const before = field.label.endsWith("日時") ? formatDate(field.before, true) : field.before;
  const after = field.label.endsWith("日時") ? formatDate(field.after, true) : field.after;
  const parts = useMemo(() => createTextDiff(before, after).parts, [before, after]);
  return <Box>
    <Text size="sm" fw={650} mb={4}>{field.label}</Text>
    <SimpleGrid cols={2} spacing="sm">
      {[before, after].map((value, index) => <Text key={index} size="sm" className="revision-metadata-value">
        {!value ? "（なし）" : parts
          .filter((part) => part.kind !== (index ? "removed" : "added"))
          .map((part, partIndex) => part.kind === "added" ? <ins className="revision-diff__part" data-kind="added" key={partIndex}>{part.value}</ins>
            : part.kind === "removed" ? <del className="revision-diff__part" data-kind="removed" key={partIndex}>{part.value}</del>
              : <span key={partIndex}>{part.value}</span>)}
      </Text>)}
    </SimpleGrid>
  </Box>;
}

export function RevisionMetadataDiff({ before, after }: { before: RevisionMetadata; after: RevisionMetadata }) {
  const changes = useMemo(() => compareRevisionMetadata(before, after), [before, after]);
  return <Stack gap="sm" aria-label="作品情報の差分">
    {(changes.fields.length > 0 || changes.assets.length > 0) && <SimpleGrid cols={2} spacing="sm"><Text size="xs" c="dimmed">変更前</Text><Text size="xs" c="dimmed">変更後</Text></SimpleGrid>}
    {changes.fields.map((field) => <MetadataField key={field.label} field={field} />)}
    {changes.assets.map((asset) => <Box key={asset.key}>
      <Group gap="xs"><Text size="sm" fw={650}>{asset.kind === "cover" ? "表紙" : asset.kind === "files" ? "添付ファイル" : "挿絵"}</Text><Badge size="xs" color={asset.change === "removed" ? "red" : "green"}>{asset.change === "added" ? "追加" : asset.change === "removed" ? "削除" : asset.change === "availability" ? "保存状態" : "変更"}</Badge></Group>
      <SimpleGrid cols={2} spacing="sm" mt={4}>
        {[asset.before, asset.after].map((item, index) => <Box key={index} className="revision-metadata-value" data-kind={index ? "added" : "removed"}>
          {item?.localPath && asset.kind !== "files" && <Image src={getAssetUrl(item.localPath)} alt={`${index ? "変更後" : "変更前"}の${asset.kind === "cover" ? "表紙" : "挿絵"}`} h={120} fit="contain" />}
          <Text size="xs" style={{ overflowWrap: "anywhere" }}>{item?.label ?? "（なし）"}{item?.available === false ? " · 未取得" : item?.available === true ? " · 保存済み" : ""}</Text>
        </Box>)}
      </SimpleGrid>
    </Box>)}
    {changes.unknown.length > 0 && <Alert color="gray" py="xs">{changes.unknown.join("・")}は、一方の版に記録がなく比較できません。</Alert>}
    {!changes.fields.length && !changes.assets.length && !changes.unknown.length && <Text size="xs" c="dimmed">記録されている作品情報・画像・添付に差分はありません。</Text>}
  </Stack>;
}
