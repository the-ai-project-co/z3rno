#!/usr/bin/env node
"use strict";

// Thin platform-dispatch shim. Ships no binary itself -- it resolves the
// right `@z3rno/cli-<platform>` optional dependency for the current
// process.platform/process.arch, then execs its prebuilt `z3rno` binary,
// forwarding argv and exit code. Same shape as esbuild's/swc's npm wrapper.

const path = require("node:path");
const { spawnSync } = require("node:child_process");

const PACKAGES = {
  "darwin arm64": "@z3rno/cli-darwin-arm64",
  "darwin x64": "@z3rno/cli-darwin-x64",
  "linux arm64": "@z3rno/cli-linux-arm64-gnu",
  "linux x64": "@z3rno/cli-linux-x64-gnu",
  "win32 x64": "@z3rno/cli-win32-x64-msvc",
};

function resolveBinary() {
  const key = `${process.platform} ${process.arch}`;
  const pkgName = PACKAGES[key];
  if (!pkgName) {
    throw new Error(
      `no prebuilt z3rno binary for platform "${process.platform}-${process.arch}". ` +
        `Supported platforms: ${Object.keys(PACKAGES).join(", ")}.`
    );
  }

  let pkgJsonPath;
  try {
    pkgJsonPath = require.resolve(`${pkgName}/package.json`);
  } catch {
    throw new Error(
      `optional dependency "${pkgName}" is not installed. This usually means ` +
        `npm skipped it (e.g. --no-optional was used, or an npm bug on your ` +
        `platform) -- try reinstalling, or install it directly: npm i ${pkgName}`
    );
  }

  const binName = process.platform === "win32" ? "z3rno.exe" : "z3rno";
  return path.join(path.dirname(pkgJsonPath), "bin", binName);
}

function main() {
  let binPath;
  try {
    binPath = resolveBinary();
  } catch (err) {
    console.error(`z3rno: ${err.message}`);
    process.exit(1);
  }

  const result = spawnSync(binPath, process.argv.slice(2), { stdio: "inherit" });

  if (result.error) {
    console.error(`z3rno: failed to launch "${binPath}": ${result.error.message}`);
    process.exit(1);
  }

  process.exit(result.status === null ? 1 : result.status);
}

main();
