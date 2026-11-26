#!/usr/bin/env node
const { spawnSync } = require("node:child_process");
const { join } = require("node:path");
const { existsSync } = require("node:fs");

const suffix = process.platform === "win32" ? ".exe" : "";
const executable = join(__dirname, "..", "native", `zg${suffix}`);
if (!existsSync(executable)) {
  console.error("zero-grep binary is missing; reinstall the npm package or install with cargo.");
  process.exit(2);
}
const result = spawnSync(executable, process.argv.slice(2), { stdio: "inherit" });
if (result.error) throw result.error;
process.exit(result.status ?? 2);
