# UE Dev Hub addons

The official addons for [UE Dev Hub](https://github.com/chillpert/ue-dev-hub). Each one is a folder
in `addons/` with an `addon.toml`, a page (`ui/`) and optionally a helper program (`helper/`). How
addons work is in the hub's [docs/addons.md](https://github.com/chillpert/ue-dev-hub/blob/main/docs/addons.md).

| Addon | What it does |
|---|---|
| [git](addons/git) | Commit, push, update, switch branches and stash changes without another Git program. |

Addons are fully trusted: an addon's page runs inside the hub and its helper runs as a normal
program with your permissions. Only install addons from people you trust.

## Writing an addon

The hub's [docs/addons.md](https://github.com/chillpert/ue-dev-hub/blob/main/docs/addons.md)
explains the `addon.toml`, the page API (`addon-api/index.d.ts`) and the helper protocol. A Rust
helper can use the hub's SDK crate, `ue-dev-hub-addon`. The addons here use it from a
checkout next to this repository; an addon in its own repository should depend on a hub tag,
so the SDK only changes when you choose:

```toml
[dependencies]
ue-dev-hub-addon = { git = "https://github.com/chillpert/ue-dev-hub", tag = "vX.Y.Z" }
```

Use the tag of the hub version your addon is made for. Addons declare the API version they
use (`api` in `addon.toml`), and a hub only loads addons with its own API version.

## Building

Needs Node.js, Rust, and the hub repository checked out next to this one
(`../ue-dev-hub`), because the addons use its SDK crate and API types.

```sh
npm install
npm run check          # type-check the pages
npm run build          # build every addon (or: npm run build -- git)
npm run pack           # zip them into out/
```

To try a zip, use Settings > Addons > Install from a zip… in the hub.

To try the signed, official way: sign the zips and write `out/index.json`, then start the hub
with `UE_DEV_HUB_ADDON_INDEX` pointing at it.

```sh
TAURI_SIGNING_PRIVATE_KEY_PATH=~/.tauri/ue-dev-hub-addons.key npm run index
```

## Releasing

Bump `version` in the addon's `addon.toml`, then push a tag like `git-v1.1.0`. The release workflow
tests and builds the addon on Windows and Linux, signs the zips, publishes them as a release, and
updates `index.json` on the `index` release that hubs read.

It needs the repository secret `ADDONS_SIGNING_KEY` (the content of `ue-dev-hub-addons.key`) and,
if the key has a password, `ADDONS_SIGNING_KEY_PASSWORD`. Until the SDK is on the hub's `main`
branch, set the repository variable `HUB_REF` to the branch that has it. To build releases
against a fixed SDK, set the variable `HUB_RELEASE_REF` to a hub tag; releases use it
instead of `HUB_REF`, while CI keeps testing against `HUB_REF`. Bump `version` in the
helper's `Cargo.toml` together with `addon.toml`; the release checks both.

CI and releases check out the hub repository, which is private, so both also need the secret
`HUB_TOKEN`: a fine-grained token with read-only Contents access to `chillpert/ue-dev-hub`.
Once the hub is public, delete the secret; the workflows then use their own token.

While this repository is private, hubs can't download from it: install from a zip instead.
Once it's public, official addons install and update from the hub, and projects that list
an addon in `[addons] default` get it by themselves.

## License

MIT, see [LICENSE](LICENSE).
