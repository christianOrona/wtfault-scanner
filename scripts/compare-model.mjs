#!/usr/bin/env node
// Run one pre-purchase inspection against a named model and report how it did.
//
// Model choice is the biggest open question in this project and the only honest
// way to answer it is to run the same scan through each candidate. Doing that by
// hand is slow and easy to get wrong - twice during development a stale server
// on port 8787 silently answered instead of the one under test, which produced
// confident nonsense. This script pins the setup and prints what actually
// happened.
//
//   node scripts/compare-model.mjs --model qwen3:8b
//   node scripts/compare-model.mjs --model gpt-oss:20b --steps 14 --speed quality
//   node scripts/compare-model.mjs --model claude-opus-5 --kind anthropic --key sk-ant-...
//
// It needs a server already running against the simulator, e.g.
//   cargo run -p aim-api -- --simulator --scenario dpf-regen --settings ./scratch.json

import { spawn } from 'node:child_process';
import fs from 'node:fs';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, arr) => {
    if (a.startsWith("--")) acc.push([a.slice(2), arr[i + 1]?.startsWith("--") ? true : arr[i + 1]]);
    return acc;
  }, []),
);

const BASE = args.base ?? "http://127.0.0.1:8787/api/v1";
const MODEL = args.model;
const KIND = args.kind ?? "ollama";
const OLLAMA = args.ollama ?? "http://192.168.1.207:11434";
const SPEED = args.speed ?? "quality";
const STEPS = args.steps ? Number(args.steps) : null;
const SCENARIO = args.scenario ?? "dpf-regen";

if (!MODEL) {
  console.error("usage: node scripts/compare-model.mjs --model <name> [--kind ollama|anthropic|xai]");
  console.error("                                      [--speed quality|fast] [--steps N] [--key ...]");
  process.exit(2);
}

const j = async (path, init) => {
  const res = await fetch(`${BASE}${path}`, {
    ...init,
    headers: init?.body ? { "content-type": "application/json" } : {},
  });
  const text = await res.text();
  try {
    return JSON.parse(text);
  } catch {
    throw new Error(`${path} -> HTTP ${res.status}: ${text.slice(0, 200)}`);
  }
};

/** A POST with no time limit, for calls that run for many minutes. */
const postLong = (path) =>
  new Promise((resolve, reject) => {
    const p = spawn(
      "curl",
      ["-s", "--max-time", "3600", "-X", "POST", `${BASE}${path}`, "-H", "content-type: application/json", "-d", "{}"],
      { shell: false },
    );
    let out = "";
    let err = "";
    p.stdout.on("data", (d) => (out += d));
    p.stderr.on("data", (d) => (err += d));
    p.on("close", (code) => {
      if (code !== 0) return reject(new Error(`curl exited ${code}: ${err.slice(0, 200)}`));
      try {
        resolve(JSON.parse(out));
      } catch {
        reject(new Error(`non-JSON reply: ${out.slice(0, 200)}`));
      }
    });
  });

/** Codes the simulator is known to be carrying, to catch invented ones. */
const truthFor = (scenario) =>
  scenario === "dpf-regen" ? new Set(["P2463", "P242F", "P2002"]) : new Set();

const codesIn = (text) => {
  const out = new Set();
  for (const m of String(text).matchAll(/\b([PBCU][0-9A-F]{4})\b/gi)) out.add(m[1].toUpperCase());
  return out;
};

(async () => {
  // Guard against the failure that wasted an hour: something else on 8787.
  const health = await j("/health");
  if (health.service !== "ai-mechanic") throw new Error("that is not the diagnostic core");
  console.log(`server: ${health.service} ${health.build_version} on ${health.bind}`);

  // Replace any existing providers so the run is unambiguous.
  const existing = await j("/settings/providers");
  for (const p of existing.providers) {
    await j(`/settings/providers/${encodeURIComponent(p.id)}`, { method: "DELETE" });
  }

  const body = {
    kind: KIND,
    label: `bench ${MODEL}`,
    model: MODEL,
    speed: SPEED,
    select: true,
    ...(STEPS ? { max_steps: STEPS } : {}),
    ...(KIND === "ollama" ? { base_url: OLLAMA } : {}),
    ...(args.key ? { api_key: args.key } : {}),
  };
  await j("/settings/providers", { method: "POST", body: JSON.stringify(body) });

  const status = await j("/agent");
  if (!status.ready) throw new Error(`agent not ready: ${status.reason}`);
  console.log(`model : ${MODEL} (${KIND}, ${SPEED}, ${STEPS ?? "default"} steps)`);

  await j("/adapter/disconnect", { method: "POST" }).catch(() => {});
  await j("/adapter/connect", {
    method: "POST",
    body: JSON.stringify({ transport: "simulator", scenario: SCENARIO, label: `bench ${MODEL}` }),
  });
  const adapter = await j("/adapter");
  console.log(`truck : ${adapter.descriptor}\n`);

  const t0 = Date.now();
  // Not `fetch`: the server sends no headers until the whole inspection is
  // finished, and undici gives up waiting for the first byte after 300s. A
  // local 8B model routinely takes longer than that, so the run would "fail"
  // while it was still working perfectly. curl has no such limit.
  const r = await postLong("/agent/inspect");
  const secs = ((Date.now() - t0) / 1000).toFixed(0);

  const tools = (r.trace ?? []).filter((e) => e.type === "tool");
  const distinct = new Set(tools.map((e) => e.name + JSON.stringify(e.arguments))).size;
  const rep = r.report;

  console.log(`time   : ${secs}s`);
  console.log(`steps  : ${r.steps}${r.truncated ? "  (ran out - report assembled from evidence)" : ""}`);
  console.log(`tokens : ${r.usage.input_tokens} in / ${r.usage.output_tokens} out`);
  console.log(`reads  : ${tools.length} calls, ${distinct} distinct`);

  if (!rep) {
    console.log("\nRESULT : no report at all");
    process.exit(0);
  }

  console.log(`\nverdict : ${rep.verdict}`);
  console.log(`headline: ${rep.headline}`);
  console.log(`findings: ${(rep.findings ?? []).length}`);
  for (const f of rep.findings ?? []) {
    console.log(`  [${f.severity}] (${f.source}) ${f.title}`);
  }

  // The checks that matter: did it invent codes, and did it report the real ones?
  const truth = truthFor(SCENARIO);
  const cited = codesIn(JSON.stringify(rep));
  const invented = [...cited].filter((c) => !truth.has(c));
  const missed = [...truth].filter((c) => !cited.has(c));

  console.log("");
  console.log(`invented codes : ${invented.length ? invented.join(", ") + "  <-- WRONG" : "none"}`);
  console.log(`missed codes   : ${missed.length ? missed.join(", ") : "none"}`);
  console.log(
    `model authored : ${r.truncated ? "no - this is the evidence-only fallback" : "yes"}`,
  );

  // Keep the whole thing. A run costs many minutes, and the plain-English text
  // is the actual product - printing only the finding titles throws away the
  // part worth reading and comparing between models.
  const out = args.out ?? `bench-${MODEL.replace(/[^a-z0-9]+/gi, "-")}.json`;
  fs.writeFileSync(out, JSON.stringify(r, null, 2));
  console.log(`\nfull report saved to ${out}`);
})().catch((e) => {
  console.error("failed:", e.message);
  process.exit(1);
});
