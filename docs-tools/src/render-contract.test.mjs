import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { test } from "node:test";

test("contract check rejects stale reference without overwriting it", () => {
  const repo = resolve(import.meta.dirname, "../..");
  const output = mkdtempSync(resolve(tmpdir(), "piep-contract-check-"));
  try {
    for (const name of ["ipc.md", "events.md", "schema.md"]) {
      copyFileSync(resolve(repo, "docs/reference", name), resolve(output, name));
    }
    const stale = resolve(output, "ipc.md");
    writeFileSync(stale, "stale contract\n");

    const result = spawnSync(process.execPath, [
      resolve(import.meta.dirname, "render-contract.mjs"),
      "--build", resolve(repo, ".docs-build"),
      "--out", output,
      "--check",
    ], { encoding: "utf8" });

    assert.equal(result.status, 1, result.stderr || result.stdout);
    assert.match(result.stdout, /generated-reference-stale/);
    assert.equal(readFileSync(stale, "utf8"), "stale contract\n");
  } finally {
    rmSync(output, { recursive: true, force: true });
  }
});
