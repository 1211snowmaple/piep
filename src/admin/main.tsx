import React from "react";
import { createRoot } from "react-dom/client";
import { createTheme, MantineProvider, mergeThemeOverrides, rem } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { Notifications } from "@mantine/notifications";
import { QueryClientProvider } from "@tanstack/react-query";
import "@mantine/core/styles.css";
import "@mantine/notifications/styles.css";
import { theme } from "@/theme";
import "@/styles/app.css";
import "@/styles/server.css";
import "./admin.css";
import { adminQueryClient } from "./queryClient";
import { AdminApp } from "./AdminApp";

/** ブラウザで読む画面なので、本体より文字を一段大きくする。 */
const adminTheme = mergeThemeOverrides(theme, createTheme({
  fontSizes: { xs: rem(13), sm: rem(15), md: rem(16), lg: rem(18), xl: rem(20) },
}));

const container = document.getElementById("root");
if (!container) throw new Error("#root element was not found");

createRoot(container).render(
  <React.StrictMode>
    <MantineProvider theme={adminTheme} defaultColorScheme="auto">
      <QueryClientProvider client={adminQueryClient}>
        <ModalsProvider labels={{ confirm: "実行", cancel: "キャンセル" }}>
          <AdminApp />
          <Notifications position="top-right" containerWidth={360} limit={3} zIndex={3000} />
        </ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>
  </React.StrictMode>,
);
