#!/usr/bin/env node
/**
 * Builds the standalone macOS `Nabla.app`, applies a valid ad-hoc bundle
 * signature (`codesign --force --deep --sign -`) so Gatekeeper shows the
 * standard "Open Anyway" prompt instead of "app is damaged", verifies the
 * signature, and packages `Nabla_<version>_<arch>.dmg`.
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
const tauriConf = JSON.parse(
  readFileSync(join(ROOT, "src-tauri", "tauri.conf.json"), "utf8"),
);
const version = tauriConf.version;
const arch = process.arch === "arm64" ? "aarch64" : process.arch;

const appPath = join(
  ROOT,
  "src-tauri",
  "target",
  "release",
  "bundle",
  "macos",
  "Nabla.app",
);
const dmgDir = join(ROOT, "src-tauri", "target", "release", "bundle", "dmg");
const dmgPath = join(dmgDir, `Nabla_${version}_${arch}.dmg`);

function run(cmd, args) {
  console.log(`[macos-dmg] ${cmd} ${args.join(" ")}`);
  execFileSync(cmd, args, { cwd: ROOT, stdio: "inherit" });
}

run("node", ["scripts/prepare-pi-runtime.mjs"]);
run("pnpm", [
  "tauri",
  "build",
  "--config",
  "src-tauri/tauri.bundle.conf.json",
  "--bundles",
  "app",
]);
run("codesign", ["--force", "--deep", "--sign", "-", appPath]);
run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", appPath]);

mkdirSync(dmgDir, { recursive: true });
rmSync(dmgPath, { force: true });
run("hdiutil", [
  "create",
  "-volname",
  "Nabla",
  "-srcfolder",
  appPath,
  "-ov",
  "-format",
  "UDZO",
  dmgPath,
]);
console.log(`[macos-dmg] ready: ${dmgPath}`);
