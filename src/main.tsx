import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { QueryClientProvider } from "@tanstack/react-query";

import App from "./App";
import "./index.css";
import "./i18n";
import { DatabaseUpgrade } from "./components/DatabaseUpgrade";
import { FrontendErrorBoundary } from "./components/FrontendErrorBoundary";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import {
  installGlobalErrorHandlers,
  reportFrontendError,
} from "@/lib/frontendLogger";
import { queryClient } from "@/lib/query";
import { initializeWindowActivity } from "@/lib/windowActivity";

interface InitErrorPayload {
  path?: string;
  error?: string;
  kind?: string;
  db_version?: number;
  supported_version?: number;
}

installGlobalErrorHandlers();

async function bootstrap() {
  let initError: InitErrorPayload | null = null;
  try {
    initError = await invoke("get_init_error");
  } catch (error) {
    reportFrontendError("get_init_error", error);
  }

  initializeWindowActivity();
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <FrontendErrorBoundary>
        <QueryClientProvider client={queryClient}>
          <ThemeProvider defaultTheme="system" storageKey="cc-switch-theme">
            {initError ? <DatabaseUpgrade payload={initError} /> : <App />}
            <Toaster />
          </ThemeProvider>
        </QueryClientProvider>
      </FrontendErrorBoundary>
    </React.StrictMode>,
  );
}

void bootstrap();
