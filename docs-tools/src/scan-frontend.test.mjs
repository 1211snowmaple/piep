import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { test } from "node:test";

test("generated AI requests are recognized as live IPC calls", () => {
  const repo = resolve(import.meta.dirname, "../..");
  const result = JSON.parse(execFileSync(process.execPath, [
    resolve(import.meta.dirname, "scan-frontend.mjs"), "--repo", repo,
  ], { encoding: "utf8", maxBuffer: 8 * 1024 * 1024 }));
  for (const command of [
    "assist_suggest_tags", "assist_interpret_search", "assist_describe_author",
    "assist_propose_splits", "assist_summarize_work", "assist_recap_previous",
  ]) {
    assert.ok(result.invocations.some((site) => site.name === command && !site.isTest && site.callerUsed),
      `${command} must be linked to its frontend caller`);
  }
  assert.ok(!result.invocations.some((site) => site.name === "command"),
    "the forwarding parameter is not itself an IPC command");
});
