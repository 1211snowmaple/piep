import { Component, type ErrorInfo, type ReactNode } from "react";
import { Alert, Button, Center, Code, Paper, Stack, Text, Title } from "@mantine/core";
import { AlertTriangle, RefreshCw } from "lucide-react";

interface State { error: Error | null }

export class AppErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Uncaught piep UI error", error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <Center mih="70vh" p="xl">
        <Paper withBorder shadow="md" p="xl" maw={620} w="100%">
          <Stack>
            <Alert icon={<AlertTriangle size={18} />} color="red" title="画面を表示できませんでした">
              操作内容は保存されたままです。画面を再読み込みしてください。
            </Alert>
            <Title order={2}>piepで予期しない問題が発生しました</Title>
            <Text c="dimmed">再発する場合は、下の内容をログとして共有してください。</Text>
            <Code block>{this.state.error.message}</Code>
            <Button leftSection={<RefreshCw size={16} />} onClick={() => window.location.reload()}>
              アプリを再読み込み
            </Button>
          </Stack>
        </Paper>
      </Center>
    );
  }
}
