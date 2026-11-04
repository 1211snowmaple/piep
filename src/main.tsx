import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";

// 一覧を覚えておき、戻ったときに読み込み中を見せない。古くなったものは表示したまま裏で取り直す。
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 2 * 60_000,
      gcTime: 15 * 60_000,
      retry: 1,
      refetchOnWindowFocus: false,
      // 家の中だけで動くサーバーもあるので、端末の回線状態では止めない。
      networkMode: "always",
    },
  },
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>,
);
