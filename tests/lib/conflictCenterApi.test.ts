import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";

import {
  conflictCenterApi,
  type ConflictCenterItem,
} from "@/lib/api/conflict-center";
import { server } from "../msw/server";

const TAURI_ENDPOINT = "http://tauri.local";

const item: ConflictCenterItem = {
  schemaVersion: 1,
  itemId: "local_provider_claude_alpha",
  source: "local_scan",
  domain: "provider",
  clientId: "claude",
  recordId: "alpha",
  displayName: "alpha",
  disposition: { type: "difference", kind: "modified" },
  baselineDigest: "a".repeat(64),
  localDigest: "b".repeat(64),
  externalDigest: "c".repeat(64),
  actions: ["accept_external", "keep_local"],
};

describe("conflictCenterApi", () => {
  it("invokes the list command without arguments", async () => {
    let body: unknown;
    server.use(
      http.post(
        `${TAURI_ENDPOINT}/list_conflict_center_items_command`,
        async ({ request }) => {
          body = await request.json();
          return HttpResponse.json([item]);
        },
      ),
    );

    await expect(conflictCenterApi.list()).resolves.toEqual([item]);
    expect(body).toEqual({});
  });

  it("wraps the resolution payload under the Tauri request argument", async () => {
    let body: unknown;
    server.use(
      http.post(
        `${TAURI_ENDPOINT}/resolve_conflict_center_item_command`,
        async ({ request }) => {
          body = await request.json();
          return HttpResponse.json(null);
        },
      ),
    );

    await expect(
      conflictCenterApi.resolve({
        itemId: item.itemId,
        action: "accept_external",
      }),
    ).resolves.toBeUndefined();
    expect(body).toEqual({
      request: {
        itemId: item.itemId,
        action: "accept_external",
      },
    });
  });
});
