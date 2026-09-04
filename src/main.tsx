import React from "react";
import { createRoot } from "react-dom/client";
import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { Notifications } from "@mantine/notifications";
import { QueryClientProvider } from "@tanstack/react-query";
import "@mantine/core/styles.css";
import "@mantine/notifications/styles.css";
import "@mantine/spotlight/styles.css";
import App from "@/App";
import { applyStoredDensity } from "@/lib/density";
import { queryClient } from "@/lib/queryClient";
import { theme } from "@/theme";
import "@/styles/app.css";

const container = document.getElementById("root");

if (!container) throw new Error("#root element was not found");

applyStoredDensity();

createRoot(container).render(
  <React.StrictMode>
    <MantineProvider theme={theme} defaultColorScheme="auto">
      <QueryClientProvider client={queryClient}>
        <ModalsProvider labels={{ confirm: "実行", cancel: "キャンセル" }}>
          <App />
          <Notifications position="top-right" containerWidth={360} notificationMaxHeight="min(220px, calc(100dvh - 32px))" limit={3} zIndex={3000} />
        </ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>
  </React.StrictMode>,
);
