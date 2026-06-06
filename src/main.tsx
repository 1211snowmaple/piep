import React from "react";
import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "sonner";
import "./styles/globals.css";
import "./styles/app.css";
import App from "./App";
import { queryClient } from "./lib/queryClient";

const container = document.getElementById("root");
if (container) {
  const root = createRoot(container);
  root.render(
    <React.StrictMode>
      <QueryClientProvider client={queryClient}>
        <App />
        <Toaster richColors position="bottom-right" closeButton />
      </QueryClientProvider>
    </React.StrictMode>,
  );
}
