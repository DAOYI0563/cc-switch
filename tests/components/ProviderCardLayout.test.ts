import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const PROVIDER_CARD_TSX = path.resolve(
  __dirname,
  "..",
  "..",
  "src",
  "components",
  "providers",
  "ProviderCard.tsx",
);

describe("ProviderCard layout", () => {
  const source = fs.readFileSync(PROVIDER_CARD_TSX, "utf8");

  it("lets website links use available card width before truncating", () => {
    expect(source).not.toContain("max-w-[280px]");
    expect(source).toContain("flex min-w-0 items-center gap-3");
    expect(source).toContain('<div className="min-w-0">');
    expect(source).toContain(
      "mt-1 block max-w-full truncate text-left text-xs text-muted-foreground",
    );
  });
});
