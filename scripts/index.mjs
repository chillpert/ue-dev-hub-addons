// Signs the zips in out/ and writes out/index.json, the list the hub installs from.
//
//   node scripts/index.mjs [--previous old-index.json] [--base-url https://…/download/git-v1.0.0]
//
// Signing needs TAURI_SIGNING_PRIVATE_KEY (or _PATH) and, if the key has one,
// TAURI_SIGNING_PRIVATE_KEY_PASSWORD. Entries for addons not in out/ are kept from --previous,
// so releasing one addon doesn't drop the others. Without --base-url, downloads are next to
// the index (for trying it from a folder with UE_DEV_HUB_ADDON_INDEX).
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { addons, manifest, out } from "./common.mjs";

const arg = (name) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const previous = arg("--previous");
const baseUrl = arg("--base-url");

const index = previous && existsSync(previous) ? JSON.parse(readFileSync(previous, "utf8")) : { addons: [] };
const byId = new Map(index.addons.map((e) => [e.id, e]));

for (const dir of addons([])) {
  const m = manifest(dir);
  const zips = readdirSync(out).filter((f) => f.startsWith(`${m.id}-${m.version}-`) && f.endsWith(".zip"));
  if (zips.length === 0) continue;
  const entry = { id: m.id, name: m.name, description: m.description, version: m.version, api: m.api, platforms: {} };
  for (const zip of zips) {
    const file = join(out, zip);
    execFileSync("npx", ["tauri", "signer", "sign", file], { stdio: "inherit", shell: process.platform === "win32" });
    const platform = zip.slice(`${m.id}-${m.version}-`.length, -".zip".length);
    entry.platforms[platform] = {
      url: baseUrl ? `${baseUrl}/${zip}` : zip,
      sha256: createHash("sha256").update(readFileSync(file)).digest("hex"),
      signature: readFileSync(`${file}.sig`, "utf8").trim(),
    };
  }
  // A release for one platform keeps the other platform's files of the same version.
  const old = byId.get(m.id);
  if (old && old.version === m.version) entry.platforms = { ...old.platforms, ...entry.platforms };
  byId.set(m.id, entry);
}

writeFileSync(join(out, "index.json"), JSON.stringify({ addons: [...byId.values()] }, null, 2) + "\n");
console.log(`Wrote ${join(out, "index.json")}`);
