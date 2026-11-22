#!/usr/bin/env node
/**
 * Prepares a self-contained `pi` runtime inside `src-tauri/` so standalone
 * Nabla installers (Windows NSIS, Linux .deb, macOS .dmg) work out of the box
 * without requiring the user to install Node.js or `pi` manually.
 *
 * Outputs (all gitignored):
 *   - `src-tauri/binaries/nabla-node-<target-triple>{.exe}` (Tauri externalBin)
 *   - `src-tauri/resources/pi-runtime/node_modules/@earendil-works/pi-coding-agent/`
 */
import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const NODE_VERSION = process.env.NABLA_NODE_VERSION || "22.14.0";
const PI_PKG_VERSION = process.env.NABLA_PI_VERSION || "latest";

const ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
const BINARIES_DIR = join(ROOT, "src-tauri", "binaries");
const RUNTIME_DIR = join(ROOT, "src-tauri", "resources", "pi-runtime");

function detectTargetTriple() {
  const argIdx = process.argv.indexOf("--target");
  if (argIdx !== -1 && process.argv[argIdx + 1]) {
    return process.argv[argIdx + 1];
  }
  const { platform, arch } = process;
  if (platform === "win32" && arch === "x64") return "x86_64-pc-windows-msvc";
  if (platform === "win32" && arch === "arm64") return "aarch64-pc-windows-msvc";
  if (platform === "linux" && arch === "x64") return "x86_64-unknown-linux-gnu";
  if (platform === "linux" && arch === "arm64") return "aarch64-unknown-linux-gnu";
  if (platform === "darwin" && arch === "arm64") return "aarch64-apple-darwin";
  if (platform === "darwin" && arch === "x64") return "x86_64-apple-darwin";
  throw new Error(`Unsupported platform/arch: ${platform}/${arch}`);
}

function nodeDistForTriple(triple) {
  switch (triple) {
    case "x86_64-pc-windows-msvc":
      return {
        archive: `node-v${NODE_VERSION}-win-x64.zip`,
        innerBin: `node-v${NODE_VERSION}-win-x64/node.exe`,
        ext: ".exe",
        format: "zip",
      };
    case "aarch64-pc-windows-msvc":
      return {
        archive: `node-v${NODE_VERSION}-win-arm64.zip`,
        innerBin: `node-v${NODE_VERSION}-win-arm64/node.exe`,
        ext: ".exe",
        format: "zip",
      };
    case "x86_64-unknown-linux-gnu":
      return {
        archive: `node-v${NODE_VERSION}-linux-x64.tar.xz`,
        innerBin: `node-v${NODE_VERSION}-linux-x64/bin/node`,
        ext: "",
        format: "tar.xz",
      };
    case "aarch64-unknown-linux-gnu":
      return {
        archive: `node-v${NODE_VERSION}-linux-arm64.tar.xz`,
        innerBin: `node-v${NODE_VERSION}-linux-arm64/bin/node`,
        ext: "",
        format: "tar.xz",
      };
    case "aarch64-apple-darwin":
      return {
        archive: `node-v${NODE_VERSION}-darwin-arm64.tar.gz`,
        innerBin: `node-v${NODE_VERSION}-darwin-arm64/bin/node`,
        ext: "",
        format: "tar.gz",
      };
    case "x86_64-apple-darwin":
      return {
        archive: `node-v${NODE_VERSION}-darwin-x64.tar.gz`,
        innerBin: `node-v${NODE_VERSION}-darwin-x64/bin/node`,
        ext: "",
        format: "tar.gz",
      };
    default:
      throw new Error(`No Node distribution mapping for target triple: ${triple}`);
  }
}

async function downloadFile(url, dest) {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`Failed to download ${url}: HTTP ${res.status}`);
  }
  const buf = Buffer.from(await res.arrayBuffer());
  writeFileSync(dest, buf);
}

async function prepareNodeBinary(triple) {
  const dist = nodeDistForTriple(triple);
  mkdirSync(BINARIES_DIR, { recursive: true });
  const targetPath = join(BINARIES_DIR, `nabla-node-${triple}${dist.ext}`);
  if (existsSync(targetPath)) {
    console.log(`[pi-runtime] reusing existing ${targetPath}`);
    return targetPath;
  }

  const url = `https://nodejs.org/dist/v${NODE_VERSION}/${dist.archive}`;
  const workDir = mkdtempSync(join(tmpdir(), "nabla-node-"));
  const archivePath = join(workDir, dist.archive);
  try {
    console.log(`[pi-runtime] downloading ${url}`);
    await downloadFile(url, archivePath);

    if (dist.format === "zip") {
      if (process.platform === "win32") {
        execFileSync(
          "powershell",
          [
            "-NoProfile",
            "-Command",
            `Expand-Archive -Path '${archivePath}' -DestinationPath '${workDir}' -Force`,
          ],
          { stdio: "inherit" },
        );
      } else {
        execFileSync("unzip", ["-q", archivePath, "-d", workDir], { stdio: "inherit" });
      }
    } else if (dist.format === "tar.xz") {
      execFileSync("tar", ["-xJf", archivePath, "-C", workDir], { stdio: "inherit" });
    } else {
      execFileSync("tar", ["-xzf", archivePath, "-C", workDir], { stdio: "inherit" });
    }

    const extracted = join(workDir, ...dist.innerBin.split("/"));
    if (!existsSync(extracted)) {
      throw new Error(`Expected node binary not found after extraction: ${extracted}`);
    }
    copyFileSync(extracted, targetPath);
    if (process.platform !== "win32") {
      chmodSync(targetPath, 0o755);
    }
    console.log(`[pi-runtime] prepared sidecar ${targetPath}`);
    return targetPath;
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

function preparePiPackages() {
  mkdirSync(RUNTIME_DIR, { recursive: true });
  const pkgJsonPath = join(RUNTIME_DIR, "package.json");
  writeFileSync(
    pkgJsonPath,
    JSON.stringify(
      {
        name: "nabla-embedded-pi-runtime",
        private: true,
        dependencies: {
          "@earendil-works/pi-coding-agent": PI_PKG_VERSION,
        },
      },
      null,
      2,
    ) + "\n",
  );

  const npmCmd = process.platform === "win32" ? "npm.cmd" : "npm";
  console.log(`[pi-runtime] installing @earendil-works/pi-coding-agent@${PI_PKG_VERSION} into ${RUNTIME_DIR}`);
  const result = spawnSync(
    npmCmd,
    ["install", "--omit=dev", "--no-audit", "--no-fund", "--prefer-offline"],
    { cwd: RUNTIME_DIR, stdio: "inherit", shell: process.platform === "win32" },
  );
  if (result.status !== 0) {
    throw new Error(`npm install failed with exit code ${result.status}`);
  }

  const cliPath = join(
    RUNTIME_DIR,
    "node_modules",
    "@earendil-works",
    "pi-coding-agent",
    "dist",
    "cli.js",
  );
  if (!existsSync(cliPath)) {
    throw new Error(`pi CLI entry point missing at ${cliPath}`);
  }
  return cliPath;
}

async function main() {
  const triple = detectTargetTriple();
  console.log(`[pi-runtime] target triple: ${triple}`);
  const nodeBin = await prepareNodeBinary(triple);
  const cliPath = preparePiPackages();

  // Smoke-check on native host builds
  const check = spawnSync(nodeBin, [cliPath, "--version"], {
    encoding: "utf8",
    env: { ...process.env, NODE_PATH: join(RUNTIME_DIR, "node_modules") },
  });
  if (check.status === 0) {
    console.log(`[pi-runtime] verified bundled pi: ${check.stdout.trim()}`);
  } else {
    console.warn(`[pi-runtime] smoke-check skipped/warned: ${check.stderr || check.error}`);
  }
}

main().catch((err) => {
  console.error("[pi-runtime] fatal:", err);
  process.exit(1);
});
