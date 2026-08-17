import { Suspense, lazy, useEffect } from "react";
import { Center, Loader, Text } from "@mantine/core";
import { modals } from "@mantine/modals";
import { AppErrorBoundary } from "@/app/AppErrorBoundary";
import { AppFrame } from "@/app/AppFrame";
import { AppRouter, matchPath, useAppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { installCloseGuard } from "@/lib/unsavedGuard";

const DashboardPage = lazy(() => import("@/features/dashboard/DashboardPage"));
const LibraryPage = lazy(() => import("@/features/library/LibraryPage"));
const WorkPage = lazy(() => import("@/features/library/WorkPage"));
const EntityPage = lazy(() => import("@/features/library/EntityPage"));
const ReaderPage = lazy(() => import("@/features/reader/ReaderPage"));
const EditorPage = lazy(() => import("@/features/editor/EditorPage"));
const SavePage = lazy(() => import("@/features/save/SavePage"));
const EpubPage = lazy(() => import("@/features/epub/EpubPage"));
const TemplateStudioPage = lazy(() => import("@/features/epub/TemplateStudioPage"));
const UpdatesPage = lazy(() => import("@/features/updates/UpdatesPage"));
const OperationsPage = lazy(() => import("@/features/jobs/OperationsPage"));
const DiagnosticsPage = lazy(() => import("@/features/diagnostics/DiagnosticsPage"));
const SettingsPage = lazy(() => import("@/features/settings/SettingsPage"));

function RouteFallback() {
  return <Center h="100%" role="status" aria-live="polite"><Loader size="sm" aria-hidden /><Text className="visually-hidden">画面を読み込んでいます</Text></Center>;
}

function CurrentRoute() {
  const { pathname } = useAppRouter();
  if (pathname === "/") return <DashboardPage />;
  if (pathname === "/library") return <LibraryPage />;
  if (matchPath("/works/:workId", pathname)) return <WorkPage />;
  if (matchPath("/people/:source/:sourceKey", pathname)) return <EntityPage kind="person" />;
  if (matchPath("/series/:source/:sourceKey", pathname)) return <EntityPage kind="series" />;
  if (matchPath("/reader/:workId", pathname)) return <ReaderPage />;
  if (matchPath("/editor/:workId", pathname)) return <EditorPage />;
  if (matchPath("/save/:source?", pathname)) return <SavePage />;
  if (pathname === "/epub") return <EpubPage />;
  if (pathname === "/epub/templates") return <TemplateStudioPage />;
  if (pathname === "/updates") return <UpdatesPage />;
  if (pathname === "/operations") return <OperationsPage />;
  if (pathname === "/diagnostics") return <DiagnosticsPage />;
  if (pathname === "/settings") return <SettingsPage />;
  return <DashboardPage />;
}

function confirmDiscardOnClose(): Promise<boolean> {
  return new Promise((resolve) => {
    modals.openConfirmModal({
      title: "保存していない変更があります",
      children: <Text size="sm">終了すると編集中の内容は失われます。閉じてよろしいですか？</Text>,
      labels: { confirm: "保存せずに終了", cancel: "アプリに戻る" },
      confirmProps: { color: "red" },
      onConfirm: () => resolve(true),
      onCancel: () => resolve(false),
      onClose: () => resolve(false),
    });
  });
}

function confirmDiscardOnNavigation(): Promise<boolean> {
  return new Promise((resolve) => {
    modals.openConfirmModal({
      title: "この画面を離れますか？",
      children: <Text size="sm">処理中の作業または保存していない変更があります。移動すると現在の作業内容が失われる可能性があります。</Text>,
      labels: { confirm: "破棄して移動", cancel: "この画面に残る" },
      confirmProps: { color: "red" },
      onConfirm: () => resolve(true),
      onCancel: () => resolve(false),
      onClose: () => resolve(false),
    });
  });
}

function AppContent() {
  const { pathname } = useAppRouter();
  useEffect(() => {
    let dispose: (() => void) | undefined;
    installCloseGuard(confirmDiscardOnClose).then((fn) => { dispose = fn; }).catch(() => undefined);
    return () => dispose?.();
  }, []);
  return (
    <WorkspaceProvider>
      <AppFrame>
        {/* Suspense sits inside the frame so loading a route chunk swaps the
            content area only, instead of blanking the whole shell. */}
        <AppErrorBoundary key={pathname}>
          <Suspense fallback={<RouteFallback />}><CurrentRoute /></Suspense>
        </AppErrorBoundary>
      </AppFrame>
    </WorkspaceProvider>
  );
}

export default function App() {
  return <AppRouter confirmNavigation={confirmDiscardOnNavigation}><AppContent /></AppRouter>;
}
