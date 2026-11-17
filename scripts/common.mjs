// Shared by the build scripts: the addons in this repository and the platform key.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

export const root = resolve(import.meta.dirname, "..");
export const out = join(root, "out");

/** "windows-x86_64" or "linux-x86_64", like the hub's addons::platform(). */
export const platform = `${process.platform === "win32" ? "windows" : "linux"}-${process.arch === "x64" ? "x86_64" : process.arch}`;
export const exe = process.platform === "win32" ? ".exe" : "";

/** Reads the few top-level keys of addon.toml the scripts need. */
export function manifest(dir) {
  const text = readFileSync(join(dir, "addon.toml"), "utf8");
  const get = (key) => text.match(new RegExp(String.raw`^${key}\s*=\s*"([^"]*)"`, "m"))?.[1];
  return { id: get("id"), name: get("name"), description: get("description") ?? "", version: get("version"), api: Number(text.match(/^api\s*=\s*(\d+)/m)?.[1]) };
}

/** Every addon folder, or only the ids given on the command line. */
export function addons(only = process.argv.slice(2).filter((a) => !a.startsWith("-"))) {
  return readdirSync(join(root, "addons"))
    .map((name) => join(root, "addons", name))
    .filter((dir) => existsSync(join(dir, "addon.toml")))
    .filter((dir) => only.length === 0 || only.includes(manifest(dir).id));
}
