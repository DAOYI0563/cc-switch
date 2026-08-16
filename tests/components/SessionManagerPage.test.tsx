import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SessionManagerPage } from "@/components/sessions/SessionManagerPage";
import { sessionsApi } from "@/lib/api/sessions";
import type { SessionMessage, SessionMeta } from "@/types";

const sessions: SessionMeta[] = [
  {
    providerId: "codex",
    sessionId: "codex-session-1",
    title: "Alpha Session",
    projectDir: "/work/alpha",
    createdAt: 10,
    lastActiveAt: 20,
    resumeCommand: "codex resume codex-session-1",
  },
  {
    providerId: "claude",
    sessionId: "claude-session-1",
    title: "Beta Session",
    projectDir: "/work/beta",
    createdAt: 5,
    lastActiveAt: 15,
    resumeCommand: "claude --resume claude-session-1",
  },
];

const messages: SessionMessage[] = [
  {
    sequence: 0,
    role: "user",
    content: "read-only fixture message",
    occurredAt: 20,
  },
];

function renderPage(appId = "codex") {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <SessionManagerPage appId={appId} />
    </QueryClientProvider>,
  );
}

describe("SessionManagerPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.spyOn(sessionsApi, "search").mockResolvedValue({
      items: sessions,
      offset: 0,
      total: sessions.length,
    });
    vi.spyOn(sessionsApi, "getMessages").mockResolvedValue({
      items: messages,
      offset: 0,
      total: messages.length,
    });
    vi.spyOn(sessionsApi, "launchTerminal").mockResolvedValue(true);
  });

  it("只展示三个目标客户端并且没有删除入口", async () => {
    renderPage("codex");
    expect(await screen.findByText("Alpha Session")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /删除/ })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("combobox", { name: /客户端筛选/ }));
    expect(await screen.findByRole("option", { name: "Claude Code" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Codex" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "OpenCode" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Gemini/ })).not.toBeInTheDocument();
  });

  it("向后端提交项目、日期和关键词筛选", async () => {
    renderPage("all");
    await screen.findByText("Alpha Session");

    fireEvent.change(screen.getByRole("textbox", { name: /搜索会话/ }), {
      target: { value: "needle" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: /项目目录/ }), {
      target: { value: "/work" },
    });
    fireEvent.change(screen.getByLabelText("开始日期"), {
      target: { value: "2026-08-01" },
    });

    await waitFor(
      () => {
        expect(sessionsApi.search).toHaveBeenLastCalledWith(
          expect.objectContaining({
            providerId: "all",
            keyword: "needle",
            project: "/work",
            fromMs: expect.any(Number),
            offset: 0,
            limit: 50,
          }),
        );
      },
      { timeout: 1500 },
    );
  });

  it("读取统一事件并只用客户端和会话 ID 恢复", async () => {
    renderPage("codex");
    expect(await screen.findByText("read-only fixture message")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "恢复会话" }));
    expect(sessionsApi.launchTerminal).toHaveBeenCalledWith({
      providerId: "codex",
      sessionId: "codex-session-1",
    });
    expect(sessionsApi.getMessages).toHaveBeenCalledWith(
      "codex",
      "codex-session-1",
      0,
      200,
    );
  });
});
