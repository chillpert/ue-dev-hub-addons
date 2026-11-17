// Builds each addon's page (Vite) and helper (Cargo, release). `npm run build -- git` builds one.
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { addons, manifest, root } from "./common.mjs";

const run = (cmd, args, cwd) => execFileSync(cmd, args, { cwd, stdio: "inherit", shell: process.platform === "win32" });

for (const dir of addons()) {
  const m = manifest(dir);
  console.log(`\n== ${m.id} ${m.version}`);
  if (existsSync(join(dir, "ui", "vite.config.ts"))) run("npx", ["vite", "build", "--config", join(dir, "ui", "vite.config.ts")], root);
  if (existsSync(join(dir, "helper", "Cargo.toml"))) run("cargo", ["build", "--release", "--locked"], join(dir, "helper"));
}
