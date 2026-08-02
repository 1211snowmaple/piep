import { Suspense, lazy } from "react";
import { Center, Loader } from "@mantine/core";
import { AppErrorBoundary } from "@/app/AppErrorBoundary";
import { AppFrame } from "@/app/AppFrame";
import { AppRouter, matchPath, useAppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";

const DashboardPage = lazy(() => import("@/features/dashboard/DashboardPage"));
const LibraryPage = lazy(() => import("@/features/library/LibraryPage"));
const WorkPage = lazy(() => import("@/features/library/WorkPage"));
const EntityPage = lazy(() => import("@/features/library/EntityPage"));
const ReaderPage = lazy(() => import("@/features/reader/ReaderPage"));
const EditorPage = lazy(() => import("@/features/editor/EditorPage"));
const SavePage = lazy(() => import("@/features/save/SavePage"));
const EpubPage = lazy(() => import("@/features/epub/EpubPage"));
const UpdatesPage = lazy(() => import("@/features/updates/UpdatesPage"));
const SettingsPage = lazy(() => import("@/features/settings/SettingsPage"));

function RouteFallback() { return <Center h="100%" aria-label="画面を読み込んでいます"><Loader size="sm" /></Center>; }

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
  if (pathname === "/updates") return <UpdatesPage />;
  if (pathname === "/settings") return <SettingsPage />;
  return <DashboardPage />;
}

function AppContent() {
  const { pathname } = useAppRouter();
  return (
    <WorkspaceProvider>
      <Suspense fallback={<RouteFallback />}>
        <AppFrame>
          <AppErrorBoundary key={pathname}><CurrentRoute /></AppErrorBoundary>
        </AppFrame>
      </Suspense>
    </WorkspaceProvider>
  );
}

export default function App() {
  return <AppRouter><AppContent /></AppRouter>;
}
