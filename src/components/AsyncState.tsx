import type { ReactNode } from "react";
import { Alert, Button, Center, Loader, Paper, Stack, Text, ThemeIcon, Title } from "@mantine/core";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { errorMessage } from "@/lib/format";

export function LoadingState({ label = "読み込んでいます" }: { label?: string }) {
  return <Center mih={260} role="status" aria-live="polite" aria-busy="true"><Stack align="center" gap="sm"><Loader size="sm" aria-hidden /><Text c="dimmed" size="sm">{label}</Text></Stack></Center>;
}

export function ErrorState({ error, retry }: { error: unknown; retry?: () => void }) {
  return (
    <Alert color="red" icon={<Icons.error size={IconSize.nav} />} title="読み込みに失敗しました" role="alert">
      <Stack gap="sm">
        <Text size="sm" className="async-error-message">{errorMessage(error)}</Text>
        {retry && <Button variant="light" color="red" size="xs" w="fit-content" leftSection={<Icons.retry size={IconSize.menu} />} onClick={retry}>再試行</Button>}
      </Stack>
    </Alert>
  );
}

export function EmptyState({
  title,
  description,
  action,
  icon: Icon = Icons.empty,
}: {
  title: string;
  description: ReactNode;
  action?: ReactNode;
  icon?: LucideIcon;
}) {
  return (
    <Paper className="empty-state" p="xl" withBorder>
      <Stack align="center" gap="sm" ta="center">
        <ThemeIcon size={52} radius="xl" variant="light"><Icon size={24} /></ThemeIcon>
        <Title order={3}>{title}</Title>
        <Text c="dimmed" size="sm" maw={480}>{description}</Text>
        {action}
      </Stack>
    </Paper>
  );
}
