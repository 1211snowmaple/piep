import type { ReactNode } from "react";
import { Alert, Button, Center, Loader, Paper, Stack, Text, ThemeIcon, Title } from "@mantine/core";
import { AlertTriangle, Inbox, RefreshCw, type LucideIcon } from "lucide-react";
import { errorMessage } from "@/lib/format";

export function LoadingState({ label = "読み込んでいます" }: { label?: string }) {
  return <Center mih={260}><Stack align="center" gap="sm"><Loader size="sm" /><Text c="dimmed" size="sm">{label}</Text></Stack></Center>;
}

export function ErrorState({ error, retry }: { error: unknown; retry?: () => void }) {
  return (
    <Alert color="red" icon={<AlertTriangle size={18} />} title="読み込みに失敗しました">
      <Stack gap="sm">
        <Text size="sm">{errorMessage(error)}</Text>
        {retry && <Button variant="light" color="red" size="xs" w="fit-content" leftSection={<RefreshCw size={14} />} onClick={retry}>再試行</Button>}
      </Stack>
    </Alert>
  );
}

export function EmptyState({
  title,
  description,
  action,
  icon: Icon = Inbox,
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
