import { Avatar, Box, Divider, Group, Pagination, Text } from "@mantine/core";

/** 作品の画面のタブ。旧棚の作品と棚の作品で同じ。 */
export type WorkTab = "overview" | "content" | "assets" | "history" | "json";

export function parseWorkTab(value: string | null): WorkTab {
  return value === "content" || value === "assets" || value === "history" || value === "json" ? value : "overview";
}

/**
 * The same controls the reader moves between source pages with.
 *
 * It is the same act on the same document, so it should not be a different
 * shape here - two wide buttons that could only step one page at a time, in a
 * novel that has forty of them.
 */
export function ContentPagination({ current, total, onChange }: { current: number; total: number; onChange: (page: number) => void }) {
  if (total <= 1) return null;
  return <Group justify="center" gap={6} wrap="nowrap" className="content-pagination">
    <Pagination total={total} value={current} onChange={onChange} siblings={1} boundaries={1} withEdges size="sm" radius="sm" />
    <Divider orientation="vertical" h={24} />
    <Text size="sm" fw={650} ff="monospace" c="dimmed">{current} / {total}</Text>
  </Group>;
}

export function Info({ label, value, icon }: { label: string; value: string; icon: React.ReactNode }) {
  return <Group wrap="nowrap" align="flex-start"><Avatar size="sm" variant="light" color="gray">{icon}</Avatar><Box><Text size="xs" c="dimmed">{label}</Text><Text size="sm" fw={650} mt={2}>{value}</Text></Box></Group>;
}
