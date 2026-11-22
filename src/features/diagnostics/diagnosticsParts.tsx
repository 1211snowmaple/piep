import type { ReactNode } from "react";
import { Box, Card, Group, Stack, Text, ThemeIcon } from "@mantine/core";
import { ActionRow } from "@/components/ActionRow";
import type { LucideIcon } from "@/lib/icons";

/** 診断の画面の数の札と、手入れの札。 */
export function MetricCard({ label, value, detail, icon: Icon, color = "piep" }: { label: string; value: string; detail: string; icon: LucideIcon; color?: string }) {
  return <Card p="lg"><Group wrap="nowrap" align="flex-start" gap="sm"><ThemeIcon size={38} variant="light" color={color} style={{ flex: "0 0 auto" }}><Icon size={18} /></ThemeIcon><Box miw={0} style={{ flex: "1 1 auto" }}><Text size="xs" c="dimmed" className="line-clamp-1">{label}</Text><Text fz="xl" fw={760} className="metric-card__value">{value}</Text><Text size="xs" c="dimmed" mt={4}>{detail}</Text></Box></Group></Card>;
}

export function MaintenanceTaskCard({ icon: Icon, title, description, status, action }: { icon: LucideIcon; title: string; description: string; status: ReactNode; action: ReactNode }) {
  return <Card p="lg" className="maintenance-task-card">
    <Stack gap="md" h="100%">
      <Group wrap="nowrap" align="flex-start" gap="sm">
        <ThemeIcon size={40} variant="light" color="piep" style={{ flex: "0 0 auto" }}><Icon size={19} /></ThemeIcon>
        <Box miw={0}>
          <Text fw={720}>{title}</Text>
          <Text size="xs" c="dimmed" mt={3}>{description}</Text>
        </Box>
      </Group>
      <Box>{status}</Box>
      <Box mt="auto"><ActionRow>{action}</ActionRow></Box>
    </Stack>
  </Card>;
}

