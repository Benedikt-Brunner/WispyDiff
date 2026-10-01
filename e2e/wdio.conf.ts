import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdirSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const fixtureDir = path.join(root, "e2e/.fixtures/stack");
const fixturesBin = path.join(root, "target/release/wispy-fixtures");
const appBinary = path.join(root, "target/release/wispydiff");
// Overridable so two checkouts (e.g. worktrees) can run their suites at the same time.
const apiPort = Number(process.env.WISPY_E2E_API_PORT ?? 4600);
const driverPort = Number(process.env.WISPY_E2E_DRIVER_PORT ?? 4445);

// The app inherits this environment: fake GitHub, throwaway data dir, no `gh` needed.
// Every wdio process evaluates this file; the run id (set once by the launcher, inherited by
// workers) makes them agree on the paths.
process.env.WISPY_E2E_RUN ??= String(Date.now());
const dataDir = path.join(tmpdir(), `wispy-e2e-${process.env.WISPY_E2E_RUN}`);
mkdirSync(dataDir, { recursive: true });
process.env.WISPY_DATA_DIR = dataDir;
process.env.WISPY_GITHUB_API = `http://127.0.0.1:${apiPort}`;
process.env.WISPY_GITHUB_TOKEN = "fixture-token";
// The assistant runs fake CLIs (speaking the real JSON formats) instead of Claude Code / Codex.
process.env.WISPY_CLAUDE_BIN = path.join(root, "e2e/fake-cli/claude");
process.env.WISPY_CODEX_BIN = path.join(root, "e2e/fake-cli/codex");
process.env.WISPY_FAKE_LOG = path.join(dataDir, "fake-cli.log");

let fakeGitHub: ChildProcess | undefined;

// Note: spec files share one app process (the embedded driver keeps it running), so specs must
// not assume a freshly launched UI.
export const config: WebdriverIO.Config = {
  runner: "local",
  specs: ["./specs/**/*.e2e.ts"],
  // The performance budgets run on their own (`just bench` sets WISPY_E2E_PERF=1).
  exclude: process.env.WISPY_E2E_PERF === "1" ? [] : ["./specs/perf.e2e.ts"],
  maxInstances: 1,
  capabilities: [{ browserName: "tauri", "tauri:options": { application: appBinary } } as WebdriverIO.Capabilities],
  services: [["@wdio/tauri-service", { appBinaryPath: appBinary, driverProvider: "embedded", embeddedPort: driverPort }]],
  framework: "mocha",
  mochaOpts: { ui: "bdd", timeout: 120_000 },
  reporters: ["spec"],
  logLevel: "warn",
  outputDir: path.join(root, "e2e/logs"),
  waitforTimeout: 30_000,

  async afterTest(test, _context, result) {
    if (result.passed) return;
    const report = await browser
      .execute(() => ({
        errors: window.__wispyErrors ?? [],
        title: document.querySelector(".pr-title")?.textContent ?? null,
        palette: document.querySelector(".palette") !== null,
        inserts: document.querySelectorAll(".insert").length,
      }))
      .catch((e) => ({ unavailable: String(e) }));
    console.log(`[failure] ${test.title}: ${JSON.stringify(report)}`);
  },
  onPrepare() {
    execFileSync(fixturesBin, ["generate", "--out", fixtureDir, "--seed", "42"], { stdio: "ignore" });
    fakeGitHub = spawn(fixturesBin, ["serve", "--fixture", fixtureDir, "--port", String(apiPort)], { stdio: "inherit" });
  },
  onComplete() {
    fakeGitHub?.kill();
    rmSync(dataDir, { recursive: true, force: true });
  },
};

export const fixture = () =>
  JSON.parse(readFileSync(path.join(fixtureDir, "fixture.json"), "utf8")) as {
    owner: string;
    repo: string;
    prs: { number: number; title: string; head_sha: string }[];
  };
