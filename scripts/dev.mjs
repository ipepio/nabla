#!/usr/bin/env node
// Frontend-only dev entry point, wired to `package.json` -> scripts.dev.
//
// `pnpm run dev <arg>` appends the arg to the script command, so `pnpm dev tauri`
// used to reach vite as a positional argument. Vite reads that as the root
// directory, drops `vite.config.ts` (port 1420, strictPort), serves on the
// default 5173 and the Tauri window waits forever on 1420. So: no arguments,
// or a hint and exit 1. The desktop app has its own command.
import { spawn } from "node:child_process";
import { constants as osConstants } from "node:os";
import { accessSync } from "node:fs";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);

if (args.length > 0) {
  console.error(
    `dev starts only the frontend and takes no arguments (received: ${args.join(" ")}).`,
  );
  console.error("For the desktop app run: pnpm tauri dev");
  // exitCode instead of exit(): lets stderr drain before the process ends.
  process.exitCode = 1;
} else {
  const viteSuffix = process.platform === "win32" ? ".cmd" : "";
  const viteBin = fileURLToPath(
    new URL(`../node_modules/.bin/vite${viteSuffix}`, import.meta.url),
  );

  try {
    accessSync(viteBin);
  } catch {
    console.error("vite is not installed. Run `pnpm install` first.");
    process.exitCode = 1;
  }

  if (process.exitCode === undefined) {
    const child = spawn(viteBin, [], { stdio: "inherit" });

    // Inherited stdio means the terminal's Ctrl-C already reaches vite; this
    // keeps the wrapper alive to report vite's exit code and forwards signals
    // sent to this process alone (IDE stop buttons, `kill`).
    for (const signal of ["SIGINT", "SIGTERM"]) {
      process.on(signal, () => {
        child.kill(signal);
      });
    }

    child.on("error", (error) => {
      console.error(`could not start vite (${viteBin}): ${error.message}`);
      process.exitCode = 1;
    });

    child.on("exit", (code, signal) => {
      if (code !== null) {
        process.exitCode = code;
        return;
      }
      // Killed by a signal: report it the way a shell does, 128 + the signal
      // number, so `pnpm dev` does not look like a clean exit.
      const number = signal ? osConstants.signals[signal] : undefined;
      process.exitCode = number ? 128 + number : 1;
    });
  }
}
