import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const fixtureDir = path.join(root, "e2e/.fixtures/stack");
const fixturesBin = path.join(root, "target/release/wispy-fixtures");
const appBinary = path.join(root, "target/release/wispydiff");
const apiPort = 4600;

// The app inherits this environment: fake GitHub, throwaway data dir, no `gh` needed.
const dataDir = mkdtempSync(path.join(tmpdir(), "wispy-e2e-"));
process.env.WISPY_DATA_DIR = dataDir;
process.env.WISPY_GITHUB_API = `http://127.0.0.1:${apiPort}`;
process.env.WISPY_GITHUB_TOKEN = "fixture-token";

let fakeGitHub: ChildProcess | undefined;

export const config: WebdriverIO.Config = {
  runner: "local",
  specs: ["./specs/**/*.e2e.ts"],
  maxInstances: 1,
  capabilities: [{ browserName: "tauri", "tauri:options": { application: appBinary } } as WebdriverIO.Capabilities],
  services: [["@wdio/tauri-service", { appBinaryPath: appBinary, driverProvider: "embedded", embeddedPort: 4445 }]],
  framework: "mocha",
  mochaOpts: { ui: "bdd", timeout: 120_000 },
  reporters: ["spec"],
  logLevel: "warn",
  outputDir: path.join(root, "e2e/logs"),
  waitforTimeout: 30_000,

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
