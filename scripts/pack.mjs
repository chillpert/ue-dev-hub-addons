// Puts each built addon into out/<id>-<version>-<platform>.zip, with addon.toml, ui/ and the
// helper for this platform. Run `npm run build` first.
import { execFileSync } from "node:child_process";
import { copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { addons, exe, manifest, out, platform } from "./common.mjs";

mkdirSync(out, { recursive: true });
for (const dir of addons()) {
  const m = manifest(dir);
  const stage = join(out, "stage", m.id);
  rmSync(stage, { recursive: true, force: true });
  mkdirSync(join(stage, "ui"), { recursive: true });
  copyFileSync(join(dir, "addon.toml"), join(stage, "addon.toml"));
  cpSync(join(dir, "ui", "dist"), join(stage, "ui"), { recursive: true });
  const toml = readFileSync(join(dir, "addon.toml"), "utf8");
  const key = process.platform === "win32" ? "windows" : "linux";
  const helper = toml.match(new RegExp(String.raw`^${key}\s*=\s*"([^"]*)"`, "m"))?.[1];
  if (helper) {
    const built = join(dir, "helper", "target", "release", `ue-dev-hub-${m.id}${exe}`);
    if (!existsSync(built)) throw new Error(`${built} is missing. Run npm run build first.`);
    mkdirSync(join(stage, helper, ".."), { recursive: true });
    copyFileSync(built, join(stage, helper));
  }
  const zip = join(out, `${m.id}-${m.version}-${platform}.zip`);
  rmSync(zip, { force: true });
  // Windows' own tar (bsdtar) writes zips; a GNU tar earlier on PATH (Git Bash) can't.
  if (process.platform === "win32") execFileSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe"), ["-a", "-c", "-f", zip, "-C", stage, "."], { stdio: "inherit" });
  else execFileSync("zip", ["-q", "-r", zip, "."], { cwd: stage, stdio: "inherit" });
  console.log(`Packed ${zip}`);
}
