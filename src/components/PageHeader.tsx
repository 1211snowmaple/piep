import type { ReactNode } from "react";
import { Group, Stack, Text, Title } from "@mantine/core";

export function PageHeader({
  eyebrow,
  title,
  description,
  actions,
}: {
  eyebrow?: string;
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <Group justify="space-between" align="flex-start" wrap="nowrap" className="page-header">
      <Stack gap={4} miw={0}>
        {eyebrow && <Text size="xs" fw={700} c="piep.6" tt="uppercase" lts="0.08em">{eyebrow}</Text>}
        <Title order={1}>{title}</Title>
        {description && <Text c="dimmed" size="sm" maw={760}>{description}</Text>}
      </Stack>
      {actions && <Group gap="sm" wrap="nowrap">{actions}</Group>}
    </Group>
  );
}
