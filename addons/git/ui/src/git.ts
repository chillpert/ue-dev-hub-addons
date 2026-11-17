// The Git page: commit, push, update, branches, stashes and history, with the team's rules built in.
import { api, type ChangedPath, type Commit, type CommitDetails, type Diff, type FileChange, type FileLock, type GitHistory, type GitOverview, type Stash, type UpdateInfo } from "./api";
import { avatar, busy as hubBusy, busyLabel, confirmDialog, errorText, h, icon, loadAvatars, modal, plural, rerender, toast } from "./host";
import { hub, ownStepsRunning } from "./api";

/// Something else runs in the hub. Newer hubs also count this page's own steps as busy;
/// those must not block the page (quick ticks queue behind each other).
const busy = () => hubBusy() && !ownStepsRunning();

type Tab = "changes" | "history" | "stashes";
/// Names for the system's own places, so Linux users don't read about the Recycle Bin.
const ON_WINDOWS = navigator.userAgent.includes("Windows");
const TRASH = ON_WINDOWS ? "the Recycle Bin" : "the trash";
const SHOW_IN_FOLDER = ON_WINDOWS ? "Show in Explorer" : "Show in folder";
/// Diffs longer than this many lines only show their start, so the page stays fast.
const MAX_LINES = 4000;

const g = {
  overview: null as GitOverview | null,
  error: null as string | null,
  tab: "changes" as Tab,
  sel: null as string | null,
  diff: null as Diff | null,
  diffFor: "",
  diffError: null as string | null,
  fileHistory: [] as Commit[],
  locks: [] as FileLock[],
  locksError: null as string | null,
  history: null as GitHistory | null,
  historyError: null as string | null,
  selCommit: null as string | null,
  details: null as CommitDetails | null,
  detailsError: null as string | null,
  selCommitFile: null as string | null,
  commitDiff: null as Diff | null,
  stashes: [] as Stash[],
  selStash: null as string | null,
  summary: "",
  body: "",
  /// What's running from this page, like "push", for spinners.
  working: null as string | null,
  /// Output of this page's last step (push, commit, ...), from hubs that send it.
  log: [] as string[],
  logOpen: false,
  menuOpen: false,
  /// Changes with every project, so answers for a previous project are dropped.
  token: 0,
  /// Changes with every overview request, so a slow, older answer never replaces a newer one.
  seq: 0,
};

export function resetGit() {
  Object.assign(g, {
    overview: null,
    error: null,
    sel: null,
    diff: null,
    diffFor: "",
    diffError: null,
    fileHistory: [],
    locks: [],
    locksError: null,
    history: null,
    historyError: null,
    selCommit: null,
    details: null,
    detailsError: null,
    selCommitFile: null,
    commitDiff: null,
    stashes: [],
    selStash: null,
    summary: "",
    body: "",
    menuOpen: false,
    log: [],
  });
  g.token++;
}

// ---------- Loading ----------

export async function loadGit() {
  const token = g.token;
  const seq = ++g.seq;
  try {
    const o = await api.gitOverview();
    if (token !== g.token || seq !== g.seq) return;
    g.overview = o;
    g.error = null;
    if (!g.sel || !o.files.some((f) => f.path === g.sel)) g.sel = o.files[0]?.path ?? null;
  } catch (e) {
    if (token !== g.token || seq !== g.seq) return;
    g.error = errorText(e);
  }
  rerender();
  // The file may have changed on disk too, so its diff is fetched again.
  loadSelected(true);
  api.gitStashes().then((s) => {
    if (token !== g.token) return;
    g.stashes = s;
    if (!s.some((x) => x.id === g.selStash)) g.selStash = s[0]?.id ?? null;
    rerender();
  }, () => {});
  if (g.tab === "history") loadHistory();
}

/// Looks at the server first (new commits on the base branch), then reloads. Runs when the page
/// opens, so "Up to date" is never just what the last check happened to see.
async function checkInBackground() {
  if (busy() || g.working) return;
  const token = g.token;
  try {
    await api.checkUpdates();
  } catch {
    // Offline is fine here; the Check button shows the error.
  }
  if (token === g.token) loadGit();
}

/// Locks come from the server, so they load on their own and only when asked.
async function loadLocks() {
  const token = g.token;
  try {
    const locks = await api.gitLocks();
    if (token !== g.token) return;
    g.locks = locks;
    g.locksError = null;
  } catch (e) {
    if (token !== g.token) return;
    g.locksError = errorText(e);
  }
  rerender();
}

/// Loads the selected file's diff. Switching files clears the old diff at once; `refresh`
/// fetches the same file again (it may have changed on disk) and keeps showing it meanwhile.
async function loadSelected(refresh = false) {
  const path = g.sel;
  if (!path || !g.overview?.files.some((f) => f.path === path)) {
    g.diff = null;
    g.diffFor = "";
    return;
  }
  if (g.diffFor === path && !refresh) return;
  if (g.diffFor !== path) {
    g.diff = null;
    g.diffError = null;
    g.fileHistory = [];
  }
  g.diffFor = path;
  const ask = ++diffAsk;
  try {
    const d = await api.gitDiff(path);
    const history = d.binary ? await api.gitFileHistory(path).catch(() => []) : [];
    if (ask !== diffAsk) return;
    g.diff = d;
    g.diffError = null;
    g.fileHistory = history;
    loadAvatars(history.map((c) => c.email));
  } catch (e) {
    if (ask !== diffAsk) return;
    g.diff = null;
    g.diffError = errorText(e);
  }
  rerender();
}
let diffAsk = 0;

async function loadHistory() {
  const token = g.token;
  try {
    const hist = await api.gitHistory();
    if (token !== g.token) return;
    g.history = hist;
    if (!hist.commits.some((c) => c.hash === g.selCommit)) g.selCommit = hist.commits[0]?.hash ?? null;
    loadAvatars(hist.commits.map((c) => c.email));
    g.historyError = null;
    rerender();
    loadCommit();
  } catch (e) {
    if (token !== g.token) return;
    g.historyError = errorText(e);
    rerender();
  }
}

async function loadCommit() {
  const hash = g.selCommit;
  if (!hash) return;
  try {
    const d = await api.gitCommitDetails(hash);
    if (g.selCommit !== hash) return;
    g.details = d;
    g.detailsError = null;
    if (!d.files.some((f) => f.path === g.selCommitFile)) g.selCommitFile = d.files[0]?.path ?? null;
    rerender();
    loadCommitDiff();
  } catch (e) {
    if (g.selCommit !== hash) return;
    g.detailsError = errorText(e);
    rerender();
  }
}

async function loadCommitDiff() {
  const hash = g.selCommit;
  const path = g.selCommitFile;
  if (!hash || !path) {
    g.commitDiff = null;
    return;
  }
  try {
    const d = await api.gitCommitDiff(hash, path);
    if (g.selCommit === hash && g.selCommitFile === path) g.commitDiff = d;
  } catch (e) {
    if (g.selCommit === hash && g.selCommitFile === path) g.detailsError = errorText(e);
  }
  rerender();
}

/// Newer hubs send a step's output to the add-on instead of the Launch screen; older ones
/// don't have onLog and show it there themselves. Only output of the page's buttons (push,
/// commit, ...) is kept, not of every tick. Called once when the addon is turned on.
export function listenToLog() {
  const l = hub() as unknown as { onLog?: (fn: (level: "info" | "detail", text: string) => void) => () => void };
  l.onLog?.((_level, text) => {
    if (!g.working) return;
    g.log.push(text);
    if (g.log.length > 500) g.log.splice(0, g.log.length - 500);
    rerender();
  });
}

function stepOutput() {
  if (!g.log.length) return null;
  if (g.working) return h("p", { class: "muted small mono ellipsis step-line", title: g.log[g.log.length - 1] }, g.log[g.log.length - 1]);
  return h(
    "details",
    { class: "step-output", open: g.logOpen, ontoggle: (e: Event) => (g.logOpen = (e.currentTarget as HTMLDetailsElement).open) },
    h("summary", { class: "muted small" }, "Output of the last step"),
    h("pre", { class: "small mono", "data-keep-scroll": "git-output" }, g.log.join(String.fromCharCode(10))),
  );
}

// People change files in Unreal while the page is open, so look again when the window comes back.
window.addEventListener("focus", () => {
  if (hub().app.activePage() === "git" && !g.working && stagePending === 0) loadGit();
});

/// Runs a step that changes the repo, shows what went wrong, and reloads afterwards.
async function run<T>(what: string, f: () => Promise<T>): Promise<T | undefined> {
  if (busy() || g.working) {
    toast(`${busyLabel() || "Another step"} is still running. Wait for it to finish first.`, "error");
    return undefined;
  }
  g.working = what;
  g.log = [];
  rerender();
  try {
    // Ticks still being saved go first, so a step never sees half of them.
    await stageQueue;
    return await f();
  } catch (e) {
    toast(errorText(e), "error");
    return undefined;
  } finally {
    g.working = null;
    await loadGit();
  }
}

// ---------- Rules the page shows before Git refuses ----------

const fileName = (p: string) => p.split("/").pop() ?? p;
const dirName = (p: string) => p.split("/").slice(0, -1).join("/");
const lockOf = (path: string) => g.locks.find((l) => l.path === path);
const lockedByOthers = (files: FileChange[]) => files.filter((f) => lockOf(f.path)?.mine === false);
const staged = () => g.overview?.files.filter((f) => f.staged && f.kind !== "C") ?? [];
/// A file name that wasn't valid text reaches the page changed, so it no longer names the real file.
const unreadable = (path: string) => path.includes(String.fromCharCode(0xfffd));
/// Files that can't be ticked or discarded from here.
const untickable = (f: FileChange) => f.kind === "C" || unreadable(f.path);
const blocked = () => !!g.overview?.repoBusy || !!g.working || busy();
/// Conflicted files can't be ticked or committed from here.
const conflicts = () => g.overview?.files.filter((f) => f.kind === "C") ?? [];

function commitBlocker(): { text: string; warn?: boolean } | null {
  const o = g.overview!;
  if (o.repoBusy) return { text: "Git is busy. See the message at the top.", warn: true };
  if (o.onBase) return { text: `You can't commit to ${o.baseBranch}. Make a branch first; your changes come with you.`, warn: true };
  if (conflicts().length) return { text: `${fileName(conflicts()[0].path)} is in the middle of a conflict. Finish or abort that in your Git program first.`, warn: true };
  if (staged().length === 0) return { text: "Tick the files you want to include." };
  const theirs = lockedByOthers(staged());
  if (theirs.length) return { text: `${fileName(theirs[0].path)} is locked by ${lockOf(theirs[0].path)!.owner}. It can't be committed while they hold the lock, so untick it.`, warn: true };
  if (!g.summary.trim()) return { text: "Write a short summary of what you changed." };
  return null;
}

// ---------- Small pieces ----------

const KIND_LABEL: Record<string, string> = { M: "Changed", A: "Added", U: "New", D: "Deleted", R: "Renamed", C: "Conflict" };

function kindBadge(kind: string) {
  return h("span", { class: `st ${kind}`, title: KIND_LABEL[kind] ?? kind }, kind === "U" ? "+" : kind);
}

function nameCell(path: string) {
  return h("span", { class: "file-name" }, h("span", { class: "n" }, fileName(path)), h("span", { class: "d" }, dirName(path)));
}

function lockBadge(path: string) {
  const l = lockOf(path);
  if (!l) return null;
  return h(
    "span",
    { class: `lock ${l.mine ? "mine" : "theirs"}`, title: l.mine ? "You have this file locked" : `Locked by ${l.owner}` },
    icon(l.mine ? "lock" : "alert", 12),
    l.mine ? "You" : l.owner,
  );
}

/// Built diffs by scroll key: re-renders reuse the same rows instead of rebuilding thousands.
const builtDiffs = new Map<string, { diff: Diff; el: HTMLElement }>();

function diffView(d: Diff, key: string) {
  const built = builtDiffs.get(key);
  if (built?.diff === d) return built.el;
  const el = buildDiff(d, key);
  builtDiffs.set(key, { diff: d, el });
  return el;
}

function buildDiff(d: Diff, key: string) {
  const rows: HTMLElement[] = [];
  let oldNo = 0;
  let newNo = 0;
  const lines = d.text.split("\n");
  let shown = 0;
  for (const line of lines) {
    if (shown >= MAX_LINES) break;
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      oldNo = Number(hunk[1]);
      newNo = Number(hunk[2]);
      rows.push(h("div", { class: "dl hunk" }, h("span", { class: "ln" }), h("span", { class: "ln" }), h("span", { class: "tx" }, line)));
      shown++;
      continue;
    }
    if (!rows.length) continue; // headers before the first hunk
    const c = line[0];
    if (c === "+") rows.push(h("div", { class: "dl add" }, h("span", { class: "ln" }), h("span", { class: "ln" }, newNo++), h("span", { class: "tx" }, line)));
    else if (c === "-") rows.push(h("div", { class: "dl del" }, h("span", { class: "ln" }, oldNo++), h("span", { class: "ln" }), h("span", { class: "tx" }, line)));
    else if (c === " ") rows.push(h("div", { class: "dl" }, h("span", { class: "ln" }, oldNo++), h("span", { class: "ln" }, newNo++), h("span", { class: "tx" }, line)));
    else if (c === "\\") rows.push(h("div", { class: "dl note" }, h("span", { class: "ln" }), h("span", { class: "ln" }), h("span", { class: "tx" }, "(no line break at the end of the file)")));
    else continue;
    shown++;
  }
  const cut = d.truncated || shown >= MAX_LINES;
  if (!rows.length) return h("div", { class: "empty" }, "No text changes. Only the file's mode or line endings changed.");
  return h(
    "div",
    { class: "diff-wrap", "data-keep-scroll": key },
    h("div", { class: "diff" }, ...rows),
    cut && h("p", { class: "muted small diff-cut" }, "This diff is long, so only the start is shown. Open the file in your editor to see all of it."),
  );
}

/// Shows a file in the system's file manager. Paths are relative to the top of the repository,
/// which can be above the project's folder.
function revealFile(path: string) {
  const root = g.overview?.root ?? hub().app.project()!.root;
  const full = ON_WINDOWS ? `${root}\\${path.replace(/\//g, "\\")}` : `${root}/${path}`;
  api.revealPath(full).catch((e) => toast(errorText(e), "error"));
}

function assetView(path: string, kind: string, history: Commit[]) {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  const l = lockOf(path);
  const label = ext === "umap" ? "Map" : ext === "uasset" ? "Unreal asset" : "Binary file";
  return h(
    "div",
    { class: "asset", "data-keep-scroll": "asset" },
    h(
      "div",
      { class: "card" },
      h(
        "div",
        { class: "row", style: "gap: 14px" },
        h("span", { class: "asset-icon" }, icon(ext === "umap" ? "map" : "image", 26)),
        h("div", { class: "col min0" }, h("strong", { class: "ellipsis" }, fileName(path)), h("span", { class: "muted small" }, `${label} · ${(KIND_LABEL[kind] ?? kind).toLowerCase()}`)),
      ),
      h("p", { class: "muted small" }, "There's no text to compare for this file. Open it in Unreal to see what changed."),
      h(
        "div",
        { class: "actions" },
        h("button", { class: "btn small", onclick: () => revealFile(path) }, icon("folder", 14), SHOW_IN_FOLDER),
      ),
    ),
    h(
      "div",
      { class: "card" },
      h("h2", {}, "Lock"),
      g.locksError
        ? h("p", { class: "muted small" }, "Couldn't check the locks. Press Check to try again.")
        : !l
          ? h("p", { class: "muted small" }, kind === "U" ? "New files don't need a lock until they're on the server." : "Nobody has this file locked.")
          : l.mine
            ? h("p", { class: "small row gap-s" }, h("span", { class: "lock mine" }, icon("lock", 12), "You"), "You have this file locked, so nobody else can push changes to it.")
            : h("p", { class: "small error-text row gap-s" }, icon("alert", 15), `Locked by ${l.owner}. You can't commit your changes to this file while they hold the lock.`),
    ),
    h(
      "div",
      { class: "card wide" },
      h("h2", {}, "Recent changes to this file"),
      history.length
        ? h("div", {}, ...history.map((c) => h("div", { class: "mini-row" }, avatar(c.author, c.email), h("span", { class: "grow min0 ellipsis" }, c.subject), h("span", { class: "muted small nowrap" }, `${c.author} · ${c.when}`))))
        : h("p", { class: "muted small" }, kind === "U" ? "New file, no history yet." : "No earlier commits found."),
    ),
  );
}

// ---------- Header ----------

function branchMenu(o: GitOverview) {
  return h(
    "div",
    { class: "branch-menu", role: "menu" },
    ...o.branches.map((b) =>
      h(
        "button",
        { class: `branch-item ${b === o.branch ? "on" : ""}`, role: "menuitem", disabled: blocked(), onclick: () => switchBranch(b) },
        icon("branch", 15),
        h("span", { class: "mono" }, b),
        b === o.baseBranch && h("span", { class: "pill" }, "base"),
        b === o.branch && icon("check", 15),
      ),
    ),
    h(
      "button",
      {
        class: "branch-item new",
        disabled: blocked(),
        onclick: () => {
          g.menuOpen = false;
          rerender();
          newBranchDialog();
        },
      },
      icon("plus", 15),
      "New branch...",
    ),
  );
}

function head(o: GitOverview) {
  const spin = (k: string) => g.working === k && h("span", { class: "spinner small" });
  const pullBlock = o.repoBusy ?? (o.editorRunning ? "Unreal Editor is open. Close it before you update." : o.strategy !== "rebase" ? "This project updates by merging. Update from the Launch page." : null);
  const canPush = !o.onBase && o.pushPolicy !== "never";
  return h(
    "header",
    { class: "git-head" },
    h(
      "div",
      { class: "row wrap", style: "gap: 12px" },
      h("h1", {}, "Git"),
      h(
        "div",
        { class: "branch-wrap" },
        h(
          "button",
          {
            class: "btn branch-btn",
            "aria-expanded": g.menuOpen ? "true" : "false",
            title: "Switch or make a branch",
            onclick: () => {
              g.menuOpen = !g.menuOpen;
              rerender();
            },
          },
          icon("branch", 16),
          h("span", {}, o.branch),
          icon("chevronDown", 15),
        ),
        g.menuOpen && branchMenu(o),
      ),
      h("span", { class: "muted small" }, o.incoming ? `${plural(o.incoming, "new change")} on ${o.baseBranch}` : `Up to date with ${o.baseBranch}`),
    ),
    h(
      "div",
      { class: "actions" },
      h("button", { class: "btn", title: "Check the server for new changes and file locks", disabled: blocked(), onclick: check }, spin("check") || icon("refresh", 16), "Check"),
      h(
        "button",
        { class: `btn ${o.incoming && !pullBlock ? "primary" : ""}`, disabled: blocked() || !o.incoming || !!pullBlock, title: pullBlock ?? (o.incoming ? `Get the new changes from ${o.baseBranch}` : `Up to date with ${o.baseBranch}`), onclick: pullDialog },
        spin("pull") || icon("download", 16),
        o.incoming ? "Update" : "Up to date",
        o.incoming > 0 && h("span", { class: "count" }, o.incoming),
      ),
      canPush &&
        h(
          "button",
          {
            class: `btn ${o.ahead && !o.incoming ? "primary" : ""}`,
            disabled: blocked() || (o.onServer && !o.ahead),
            title: !o.onServer ? "Put this branch on the server for the first time" : o.ahead ? `Send ${plural(o.ahead, "commit")} to the server` : "Nothing to push",
            onclick: push,
          },
          spin("push") || icon("upload", 16),
          o.onServer ? "Push" : "Publish branch",
          o.ahead > 0 && h("span", { class: "count" }, o.ahead),
        ),
    ),
  );
}

function banners(o: GitOverview) {
  const out: (HTMLElement | false)[] = [];
  if (o.repoBusy) {
    const lockFile = o.repoBusy.includes("index.lock");
    out.push(
      h(
        "div",
        { class: "banner error nowrap-banner" },
        icon("alert", 18),
        h(
          "div",
          { class: "col gap-s grow min0" },
          h("strong", {}, "Git is busy"),
          h("p", { class: "pre muted" }, o.repoBusy),
          h(
            "div",
            { class: "actions" },
            h("button", { class: "btn small", onclick: () => loadGit() }, icon("refresh", 14), "Try again"),
            lockFile && h("button", { class: "btn small", onclick: removeIndexLock }, icon("trash", 14), "Remove lock file..."),
          ),
        ),
      ),
    );
  }
  if (o.onBase)
    out.push(
      h(
        "div",
        { class: "banner warn" },
        h("div", { class: "col gap-xs grow min0" }, h("strong", {}, `You're on ${o.baseBranch}`), h("p", {}, `Changes reach ${o.baseBranch} only through a pull request, so you can't commit here. Make a branch for your work; your uncommitted changes come along.`)),
        h("button", { class: "btn small", disabled: blocked(), onclick: () => newBranchDialog() }, icon("plus", 14), "New branch"),
      ),
    );
  if (o.editorRunning && !o.repoBusy)
    out.push(h("div", { class: "banner" }, icon("alert", 18), h("p", { class: "muted grow min0" }, "Unreal Editor is open. You can commit and push, but updating, switching branches and stashing wait until it's closed.")));
  const theirs = lockedByOthers(o.files);
  if (theirs.length && !o.repoBusy) {
    const l = lockOf(theirs[0].path)!;
    out.push(
      h(
        "div",
        { class: "banner error nowrap-banner" },
        icon("alert", 18),
        h(
          "div",
          { class: "col gap-xs grow min0" },
          h("strong", {}, theirs.length === 1 ? `${fileName(theirs[0].path)} is locked by ${l.owner}` : `${theirs.length} files you changed are locked by someone else`),
          h("p", { class: "muted" }, "You can't commit a file someone else has locked. Leave it unticked, or discard your changes."),
        ),
      ),
    );
  }
  return out;
}

function tabs() {
  const o = g.overview!;
  const t = (key: Tab, label: string, n?: number) =>
    h(
      "button",
      {
        class: `seg ${g.tab === key ? "on" : ""}`,
        role: "tab",
        "aria-selected": g.tab === key ? "true" : "false",
        onclick: () => {
          g.tab = key;
          if (key === "history") loadHistory();
          rerender();
        },
      },
      label,
      n ? h("span", { class: "count" }, n) : null,
    );
  return h("div", { class: "segmented", role: "tablist" }, t("changes", "Changes", o.files.length), t("history", "History"), t("stashes", "Stashes", g.stashes.length));
}

// ---------- Changes ----------

function discardTitle(f: FileChange) {
  if (f.submodule) return "This is a submodule. Discard the changes inside it from its own folder.";
  if (unreadable(f.path)) return "This file's name can't be read here. Use another Git program for it.";
  return "Discard changes...";
}

function fileRow(f: FileChange) {
  const mixed = f.staged && f.unstaged && f.kind !== "U";
  const box = h("input", {
    type: "checkbox",
    checked: f.staged,
    disabled: blocked() || untickable(f),
    title: unreadable(f.path) ? "This file's name can't be read here. Use another Git program for it." : undefined,
    "aria-label": `Include ${fileName(f.path)} in the commit`,
    onclick: (e: Event) => e.stopPropagation(),
    // A half-ticked file (part of it changed again after ticking) gets ticked fully.
    onchange: (e: Event) => setStaged([f.path], mixed || (e.target as HTMLInputElement).checked),
  });
  if (mixed) queueMicrotask(() => ((box as HTMLInputElement).indeterminate = true));
  return h(
    "div",
    {
      class: `file-row ${g.sel === f.path ? "on" : ""}`,
      onclick: () => {
        g.sel = f.path;
        loadSelected();
        rerender();
      },
    },
    box,
    kindBadge(f.kind),
    nameCell(f.path),
    lockBadge(f.path),
    h(
      "span",
      { class: "row-actions" },
      h(
        "button",
        {
          class: "icon-btn",
          title: discardTitle(f),
          "aria-label": `Discard changes to ${fileName(f.path)}`,
          disabled: blocked() || untickable(f) || f.submodule,
          onclick: (e: Event) => {
            e.stopPropagation();
            discard(f);
          },
        },
        icon("trash", 15),
      ),
    ),
  );
}

function updateCommitButton() {
  const block = commitBlocker();
  const btn = document.getElementById("commit-btn") as HTMLButtonElement | null;
  const why = document.getElementById("commit-why");
  if (!btn || !why) return;
  btn.disabled = !!block || blocked();
  why.className = `why ${block?.warn ? "warn" : ""}`;
  why.textContent = block ? block.text : "Your commit stays on this computer until you push.";
}

function commitBox(o: GitOverview) {
  const n = staged().length;
  const block = commitBlocker();
  const off = o.onBase || !!o.repoBusy;
  return h(
    "div",
    { class: "commit-box" },
    h("input", {
      type: "text",
      placeholder: "Summary, like: Fix the save menu crash",
      "aria-label": "Commit summary",
      value: g.summary,
      maxlength: 120,
      disabled: off,
      oninput: (e: Event) => {
        g.summary = (e.target as HTMLInputElement).value;
        updateCommitButton();
      },
      onkeydown: (e: KeyboardEvent) => {
        if (e.key === "Enter" && (e.ctrlKey || !g.body) && !commitBlocker() && !blocked()) commit();
      },
    }),
    h(
      "textarea",
      {
        placeholder: "Details (optional)",
        "aria-label": "Commit details",
        disabled: off,
        oninput: (e: Event) => (g.body = (e.target as HTMLTextAreaElement).value),
      },
      g.body,
    ),
    h("button", { id: "commit-btn", class: "btn primary block", disabled: !!block || blocked(), onclick: commit }, g.working === "commit" ? h("span", { class: "spinner small" }) : icon("check", 16), n ? `Commit ${plural(n, "file")} to ${o.branch}` : "Commit"),
    h("p", { id: "commit-why", class: `why ${block?.warn ? "warn" : ""}` }, block ? block.text : "Your commit stays on this computer until you push."),
  );
}

function changesTab(o: GitOverview) {
  const files = o.files;
  const sel = files.find((f) => f.path === g.sel);
  const all = files.length > 0 && files.every((f) => f.staged && !f.unstaged);
  const some = files.some((f) => f.staged);
  const allBox = h("input", {
    type: "checkbox",
    checked: all,
    disabled: blocked(),
    "aria-label": "Include all files",
    onchange: (e: Event) =>
      setStaged(
        files.filter((f) => !untickable(f)).map((f) => f.path),
        (e.target as HTMLInputElement).checked,
      ),
  });
  if (some && !all) queueMicrotask(() => ((allBox as HTMLInputElement).indeterminate = true));
  const left = h(
    "section",
    { class: "pane", "aria-label": "Changed files" },
    h(
      "div",
      { class: "pane-head" },
      files.length > 0 && allBox,
      h("span", { class: "strong grow" }, files.length ? plural(files.length, "changed file") : "No changes"),
      files.length > 0 && h("button", { class: "btn small", disabled: blocked() || o.editorRunning, title: o.editorRunning ? "Close Unreal Editor first" : "Put changes aside for later", onclick: () => stashDialog() }, icon("box", 14), "Stash..."),
    ),
    files.length
      ? h("div", { class: "scroll", "data-keep-scroll": "git-files" }, ...files.map(fileRow))
      : h("div", { class: "empty" }, h("div", { class: "col gap-s center" }, icon("check", 28), h("span", {}, "Nothing to commit. Everything you changed is in a commit."))),
    commitBox(o),
  );
  let body: Node | Node[];
  if (!sel) body = h("div", { class: "empty" }, files.length ? "Pick a file to see what changed." : "When you change files, they show up here.");
  else if (g.diffError && g.diffFor === sel.path)
    body = h("div", { class: "empty" }, h("div", { class: "col gap-s center" }, h("p", { class: "pre" }, g.diffError), h("button", { class: "btn small", onclick: () => loadSelected(true) }, "Try again")));
  else if (!g.diff || g.diffFor !== sel.path) body = h("div", { class: "empty" }, h("span", { class: "spinner" }));
  else if (g.diff.binary) body = assetView(sel.path, sel.kind, g.fileHistory);
  else body = diffView(g.diff, "git-diff");
  const right = h(
    "section",
    { class: "pane", "aria-label": "What changed" },
    sel &&
      h(
        "div",
        { class: "pane-head" },
        kindBadge(sel.kind),
        h("span", { class: "mono ellipsis grow", title: sel.origPath ? `${sel.origPath} → ${sel.path}` : sel.path }, sel.origPath ? `${sel.origPath} → ${sel.path}` : sel.path),
        lockBadge(sel.path),
        h("button", { class: "btn small", disabled: blocked() || untickable(sel) || sel.submodule, title: discardTitle(sel), onclick: () => discard(sel) }, icon("trash", 14), "Discard"),
      ),
    body,
  );
  return h("div", { class: "git-split" }, left, right);
}

// ---------- History ----------

function historyTab(o: GitOverview) {
  const hist = g.history;
  const sel = hist?.commits.find((c) => c.hash === g.selCommit);
  const left = h(
    "section",
    { class: "pane", "aria-label": "Commits" },
    h("div", { class: "pane-head" }, h("span", { class: "strong grow" }, `History of ${o.branch}`), hist && h("span", { class: "muted small" }, plural(hist.commits.length, "commit"))),
    g.historyError
      ? h("div", { class: "empty" }, h("div", { class: "col gap-s center" }, h("p", { class: "pre" }, g.historyError), h("button", { class: "btn small", onclick: () => loadHistory() }, "Try again")))
      : !hist
      ? h("div", { class: "empty" }, h("span", { class: "spinner" }))
      : h(
          "div",
          { class: "scroll", "data-keep-scroll": "git-history" },
          ...hist.commits.map((c) =>
            h(
              "div",
              {
                class: `hist-row ${c.hash === g.selCommit ? "on" : ""}`,
                onclick: () => {
                  g.selCommit = c.hash;
                  g.details = null;
                  g.commitDiff = null;
                  rerender();
                  loadCommit();
                },
              },
              avatar(c.author, c.email),
              h("div", { class: "col grow min0" }, h("span", { class: "ellipsis" }, c.subject), h("span", { class: "muted small ellipsis" }, `${c.author} · ${c.when}`)),
              hist.unpushed.includes(c.hash) && h("span", { class: "pill changed", title: "Only on this computer. Push to share it." }, "Not pushed"),
            ),
          ),
        ),
  );
  let right: HTMLElement;
  if (!sel) right = h("section", { class: "pane" }, h("div", { class: "empty" }, hist?.commits.length === 0 ? "No commits yet." : "Pick a commit."));
  else {
    const d = g.details;
    const file = d?.files.find((f) => f.path === g.selCommitFile);
    right = h(
      "section",
      { class: "pane", "aria-label": "Commit details" },
      h(
        "div",
        { class: "pane-head top" },
        avatar(sel.author, sel.email),
        h("div", { class: "col grow min0" }, h("strong", {}, sel.subject), h("span", { class: "muted small" }, `${sel.author} · ${sel.when} · `, h("span", { class: "hash" }, sel.hash))),
      ),
      d?.body && h("div", { class: "commit-msg small" }, d.body),
      g.detailsError
        ? h("div", { class: "empty" }, h("div", { class: "col gap-s center" }, h("p", { class: "pre" }, g.detailsError), h("button", { class: "btn small", onclick: () => ((g.detailsError = null), loadCommit()) }, "Try again")))
        : !d
        ? h("div", { class: "empty" }, h("span", { class: "spinner" }))
        : h(
            "div",
            { class: "section-split" },
            h(
              "div",
              { class: "top" },
              ...d.files.map((f: ChangedPath) =>
                h(
                  "div",
                  {
                    class: `file-row ${f.path === g.selCommitFile ? "on" : ""}`,
                    title: f.origPath ? `${f.origPath} → ${f.path}` : f.path,
                    onclick: () => {
                      g.selCommitFile = f.path;
                      g.commitDiff = null;
                      rerender();
                      loadCommitDiff();
                    },
                  },
                  kindBadge(f.kind),
                  nameCell(f.path),
                ),
              ),
              d.files.length === 0 && h("p", { class: "muted small pad" }, "This commit doesn't change any files directly."),
            ),
            !file ? null : !g.commitDiff ? h("div", { class: "empty" }, h("span", { class: "spinner" })) : g.commitDiff.binary ? assetView(file.path, file.kind, []) : diffView(g.commitDiff, "git-hdiff"),
          ),
    );
  }
  return h("div", { class: "git-split" }, left, right);
}

// ---------- Stashes ----------

function stashesTab(o: GitOverview) {
  const sel = g.stashes.find((s) => s.id === g.selStash) ?? g.stashes[0];
  const left = h(
    "section",
    { class: "pane", "aria-label": "Stashes" },
    h(
      "div",
      { class: "pane-head" },
      h("span", { class: "strong grow" }, g.stashes.length ? plural(g.stashes.length, "stash", "stashes") : "No stashes"),
      h("button", { class: "btn small", disabled: !o.files.length || blocked() || o.editorRunning, onclick: () => stashDialog() }, icon("plus", 14), "Stash changes..."),
    ),
    g.stashes.length
      ? h(
          "div",
          { class: "scroll" },
          ...g.stashes.map((s) =>
            h(
              "div",
              {
                class: `hist-row ${sel && s.id === sel.id ? "on" : ""}`,
                onclick: () => {
                  g.selStash = s.id;
                  rerender();
                },
              },
              h("span", { class: "stash-icon" }, icon("box", 18)),
              h("div", { class: "col grow min0" }, h("span", { class: "ellipsis" }, s.name), h("span", { class: "muted small ellipsis" }, `${plural(s.files.length, "file")} · from ${s.branch} · ${s.when}`)),
            ),
          ),
        )
      : h("div", { class: "empty" }, h("div", { class: "col gap-s center", style: "max-width: 320px" }, icon("box", 28), h("span", {}, "Stashing puts your uncommitted changes aside, so you can switch branches or try something else. Bring them back here later."))),
    h("div", { class: "pane-foot muted small" }, "Stashes stay on this computer. They're never pushed."),
  );
  const overlap = sel ? sel.files.filter((f) => o.files.some((x) => x.path === f.path)) : [];
  const right = h(
    "section",
    { class: "pane", "aria-label": "Stash details" },
    sel
      ? [
          h(
            "div",
            { class: "pane-head" },
            h("div", { class: "col grow min0" }, h("strong", { class: "ellipsis" }, sel.name), h("span", { class: "muted small" }, `From ${sel.branch} · ${sel.when}`)),
            h("button", { class: "btn small", disabled: blocked(), onclick: () => dropStash(sel) }, icon("trash", 14), "Delete"),
            h(
              "button",
              { class: "btn small primary", disabled: blocked() || overlap.length > 0 || o.editorRunning, title: o.editorRunning ? "Close Unreal Editor first" : "", onclick: () => popStash(sel) },
              icon("download", 14),
              "Bring back",
            ),
          ),
          overlap.length > 0 && h("div", { class: "banner warn inset" }, h("p", {}, `You changed ${overlap.map((f) => fileName(f.path)).join(", ")} again since you stashed. Commit, stash or discard that first, so neither version gets overwritten.`)),
          sel.branch && sel.branch !== o.branch && h("p", { class: "muted small pad" }, `Stashed on ${sel.branch}. Bringing it back here puts these changes on ${o.branch}.`),
          h("div", { class: "scroll" }, ...sel.files.map((f) => h("div", { class: "file-row static", title: f.origPath ? `${f.origPath} → ${f.path}` : f.path }, kindBadge(f.kind), nameCell(f.path), lockBadge(f.path)))),
        ]
      : h("div", { class: "empty" }, "No stash selected."),
  );
  return h("div", { class: "git-split" }, left, right);
}

// ---------- Screen ----------

export function gitScreen() {
  const o = g.overview;
  if (!o)
    return h(
      "main",
      { class: "main git-main" },
      h("header", { class: "git-head" }, h("h1", {}, "Git")),
      g.error
        ? h("div", { class: "banner error" }, h("div", { class: "col gap-s" }, h("strong", {}, "Couldn't read the repository"), h("p", { class: "pre" }, g.error), h("div", {}, h("button", { class: "btn small", onclick: () => loadGit() }, "Try again"))))
        : h("div", { class: "empty" }, h("span", { class: "spinner" })),
    );
  const tab = { changes: changesTab, history: historyTab, stashes: stashesTab }[g.tab];
  return h(
    "main",
    {
      class: "main git-main",
      onmousedown: (e: MouseEvent) => {
        if (g.menuOpen && !(e.target as HTMLElement).closest(".branch-wrap")) {
          g.menuOpen = false;
          rerender();
        }
      },
    },
    head(o),
    g.error &&
      h(
        "div",
        { class: "banner error nowrap-banner" },
        icon("alert", 18),
        h("div", { class: "col gap-s grow min0" }, h("strong", {}, "Couldn't refresh. What you see may be out of date."), h("p", { class: "pre muted" }, g.error)),
        h("button", { class: "btn small", onclick: () => loadGit() }, "Try again"),
      ),
    ...banners(o),
    stepOutput(),
    h("div", { class: "row between wrap" }, tabs(), g.locksError && h("span", { class: "muted small", title: g.locksError }, "Couldn't check file locks")),
    tab(o),
  );
}

/// Called when the page opens.
export function enterGit() {
  loadGit();
  loadLocks();
  checkInBackground();
}

// ---------- Actions ----------

/// Ticks show at once, and the Git calls run one after another behind them, so quick clicks
/// on several files all count.
let stageQueue: Promise<unknown> = Promise.resolve();
let stagePending = 0;
/// Counts ticks that Git refused, so a commit waiting for them knows.
let tickErrors = 0;

function setStaged(paths: string[], on: boolean) {
  if (!paths.length || !g.overview) return;
  if (busy()) {
    toast(`${busyLabel()} is still running. Wait for it to finish first.`, "error");
    return;
  }
  for (const f of g.overview.files) {
    if (!paths.includes(f.path)) continue;
    f.staged = on;
    f.unstaged = !on;
  }
  rerender();
  stagePending++;
  const token = g.token;
  stageQueue = stageQueue
    // A tick for a project that's no longer open is dropped, never sent to the new one.
    .then(() => (token !== g.token ? undefined : on ? api.gitStage(paths) : api.gitUnstage(paths)))
    .catch((e) => {
      tickErrors++;
      toast(errorText(e), "error");
    })
    .then(() => {
      if (--stagePending === 0 && token === g.token) return loadGit();
    });
}

async function check() {
  await run("check", async () => {
    await Promise.all([api.checkUpdates(), loadLocks()]);
  });
}

async function commit() {
  if (commitBlocker()) return;
  const errors = tickErrors;
  await stageQueue;
  // A tick that failed meanwhile means the list on screen isn't what Git has.
  if (tickErrors !== errors) {
    toast("Some files couldn't be ticked or unticked, so nothing was committed. Check the list, then commit again.", "error");
    return;
  }
  const summary = g.summary;
  const body = g.body;
  // Both names of a rename, the way Git lists them. The helper refuses if its list differs.
  const paths = staged().flatMap((f) => (f.kind === "R" && f.origPath ? [f.path, f.origPath] : [f.path]));
  const hash = await run("commit", () => api.gitCommit(summary, body, paths));
  if (hash) {
    g.summary = "";
    g.body = "";
    g.history = null;
    rerender();
    loadLocks();
    toast(`Committed as ${hash}.`);
  }
}

async function push() {
  const o = g.overview!;
  if (o.incoming && o.onServer) {
    const go = await modal<string | null>(
      (close) =>
        h(
          "div",
          { class: "card" },
          h("h2", {}, "Update first?"),
          h("p", { class: "muted" }, `${o.baseBranch} has ${plural(o.incoming, "new change")} you don't have. You can push now, but updating first means your branch is tested against the latest work.`),
          h("div", { class: "actions end" }, h("button", { class: "btn", onclick: () => close("push") }, "Push anyway"), h("button", { class: "btn primary", onclick: () => close("update") }, "Update first")),
        ),
      null,
    );
    if (go === "update") return pullDialog();
    if (go !== "push") return;
  }
  const r = await run("push", () => api.gitPush());
  if (r) {
    g.history = null;
    loadLocks();
    toast(r.published ? `Put ${o.branch} on the server.` : `Pushed ${plural(r.commits, "commit")}.`);
  }
}

async function pullDialog() {
  let info: UpdateInfo | undefined;
  g.working = "pull";
  rerender();
  try {
    info = await api.checkUpdates();
  } catch (e) {
    toast(errorText(e), "error");
  }
  g.working = null;
  rerender();
  if (!info) return loadGit();
  if (!info.commits.length) {
    toast(`Already up to date with ${info.defaultBranch}.`);
    return loadGit();
  }
  if (info.strategy !== "rebase") {
    toast("This project updates by merging. Update from the Launch page instead.", "error");
    return;
  }
  loadAvatars(info.commits.map((c) => c.email));
  const i = info;
  const answer = await modal<{ push: boolean } | null>((close) => {
    let pushAfter = i.pushPolicy !== "never";
    return h(
      "div",
      { class: "card wide" },
      h("h2", {}, `Get ${plural(i.commits.length, "new change")} from ${i.defaultBranch}`),
      h("ul", { class: "list incoming-list" }, ...i.commits.map((c) => h("li", { class: "list-row" }, avatar(c.author, c.email), h("div", { class: "col grow min0" }, h("span", { class: "ellipsis" }, c.subject), h("span", { class: "muted small" }, `${c.author} · ${c.when}`))))),
      i.blockedReason
        ? h("div", { class: "banner error" }, h("div", {}, h("strong", {}, "Can't update right now"), h("p", { class: "pre" }, i.blockedReason)))
        : h(
            "p",
            { class: "muted small" },
            i.onDefaultBranch
              ? `Your copy of ${i.defaultBranch} moves forward to match the server.`
              : `Your ${plural(i.myCommits, "commit")} on ${i.branch} ${i.myCommits === 1 ? "is" : "are"} replayed on top (rebase, no merge commits). Uncommitted changes are set aside and put back, and a backup of your branch is kept.`,
          ),
      !i.blockedReason &&
        i.canPush &&
        i.pushPolicy === "ask" &&
        h(
          "label",
          { class: "option" },
          h("input", { type: "checkbox", checked: true, onchange: (e: Event) => (pushAfter = (e.target as HTMLInputElement).checked) }),
          h("span", { class: "col" }, h("span", {}, "Push my branch afterwards"), h("span", { class: "muted small" }, "Replaces your branch on the server with the updated one. Never touches anyone else's commits.")),
        ),
      h(
        "div",
        { class: "actions end" },
        h("button", { class: "btn", onclick: () => close(null) }, i.blockedReason ? "Close" : "Cancel"),
        !i.blockedReason && h("button", { class: "btn primary", onclick: () => close({ push: pushAfter }) }, icon("download", 16), "Update"),
      ),
    );
  }, null);
  if (!answer) return loadGit();
  const r = await run("pull", () => api.gitPull(answer.push));
  if (r) {
    g.history = null;
    loadLocks();
    toast(`Updated with ${plural(r.commits, "new change")}${r.pushed ? " and pushed" : ""}.`);
    for (const w of r.warnings) toast(w, "error");
  }
}

async function discard(f: FileChange) {
  const isNew = f.kind === "U" || (f.kind === "A" && !f.origPath);
  const ok = await confirmDialog(
    isNew ? `Delete ${fileName(f.path)}?` : `Discard changes to ${fileName(f.path)}?`,
    (isNew ? `This file was never committed. It goes to ${TRASH}.` : `The file goes back to how it was in your last commit. Your version goes to ${TRASH}.`) +
      (f.binary ? "\n\nIf Unreal Editor has this file open, close it first, or the editor may save your version again." : ""),
    isNew ? "Delete" : "Discard",
    true,
  );
  if (!ok) return;
  await run("discard", async () => {
    await api.gitDiscard(f.path);
    toast(isNew ? `Moved ${fileName(f.path)} to ${TRASH}.` : `Discarded changes to ${fileName(f.path)}.`);
  });
}

async function removeIndexLock() {
  const ok = await confirmDialog(
    "Remove the lock file?",
    "Only do this if no other Git program is open. The hub checks that no Git is running before it removes the file.\n\nA lock left behind by a crash is safe to remove.",
    "Remove lock file",
    true,
  );
  if (!ok) return;
  g.working = "unlock";
  try {
    await api.gitRemoveIndexLock();
    toast("Removed the lock file. Git works again.");
  } catch (e) {
    toast(errorText(e), "error");
  }
  g.working = null;
  await loadGit();
}

/// Asks what to stash and stashes it. Returns whether something was stashed.
async function stashDialog(forSwitch = false): Promise<boolean> {
  const o = g.overview!;
  const ticked = staged().map((f) => f.path);
  const choice = await modal<{ name: string; onlyTicked: boolean } | null>((close) => {
    let name = "";
    let onlyTicked = false;
    return h(
      "div",
      { class: "card" },
      h("h2", {}, "Stash changes"),
      h("p", { class: "muted" }, "Puts your uncommitted changes aside and gives you clean files. Bring them back any time from the Stashes tab."),
      !forSwitch &&
        ticked.length > 0 &&
        ticked.length < o.files.length &&
        h(
          "div",
          { class: "col" },
          h("label", { class: "option" }, h("input", { type: "radio", name: "which", checked: true, onchange: () => (onlyTicked = false) }), h("span", {}, `All ${plural(o.files.length, "change")}`)),
          h("label", { class: "option" }, h("input", { type: "radio", name: "which", onchange: () => (onlyTicked = true) }), h("span", {}, `Only the ${plural(ticked.length, "ticked file")}`)),
        ),
      h(
        "label",
        { class: "field" },
        h("span", { class: "small muted" }, "Name, so you recognize it later"),
        h("input", {
          type: "text",
          name: "stash-name",
          placeholder: `Changes on ${o.branch}`,
          oninput: (e: Event) => (name = (e.target as HTMLInputElement).value),
          onkeydown: (e: KeyboardEvent) => {
            if (e.key === "Enter") close({ name, onlyTicked });
          },
        }),
      ),
      h("div", { class: "actions end" }, h("button", { class: "btn", onclick: () => close(null) }, "Cancel"), h("button", { class: "btn primary", onclick: () => close({ name, onlyTicked }) }, icon("box", 16), forSwitch ? "Stash and switch" : "Stash")),
    );
  }, null);
  if (!choice) return false;
  const done = await run("stash", async () => {
    await api.gitStashPush(choice.name, choice.onlyTicked ? ticked : null);
    return true;
  });
  if (done) toast("Stashed your changes.");
  return !!done;
}

async function popStash(s: Stash) {
  const done = await run("stash", async () => {
    await api.gitStashPop(s.id);
    return true;
  });
  if (done) {
    g.tab = "changes";
    if (s.files[0]) g.sel = s.files[0].path;
    loadSelected();
    rerender();
    toast(`Brought back "${s.name}".`);
  }
}

async function dropStash(s: Stash) {
  const ok = await confirmDialog(`Delete "${s.name}"?`, `The ${plural(s.files.length, "stashed change")} are gone for good after this.`, "Delete stash", true);
  if (!ok) return;
  const done = await run("stash", async () => {
    await api.gitStashDrop(s.id);
    return true;
  });
  if (done) toast("Deleted the stash.");
}

async function switchBranch(name: string) {
  g.menuOpen = false;
  const o = g.overview!;
  rerender();
  if (name === o.branch) return;
  if (o.editorRunning) {
    toast("Unreal Editor is open. Save your work and close it before you switch branches.", "error");
    return;
  }
  if (o.files.length) {
    const go = await modal<boolean>(
      (close) =>
        h(
          "div",
          { class: "card" },
          h("h2", {}, `Switch to ${name}?`),
          h("p", { class: "muted" }, `You have ${plural(o.files.length, "uncommitted change")} on ${o.branch}. Commit them first, or stash them to bring back later.`),
          h("div", { class: "actions end" }, h("button", { class: "btn", onclick: () => close(false) }, "Cancel"), h("button", { class: "btn primary", onclick: () => close(true) }, icon("box", 16), "Stash and switch")),
        ),
      false,
    );
    if (!go || !(await stashDialog(true))) return;
  }
  const warnings = await run("switch", () => api.gitSwitchBranch(name));
  if (warnings) {
    afterBranchChange();
    toast(`Switched to ${name}.`);
    warnings.forEach((w) => toast(w, "error"));
  }
}

function afterBranchChange() {
  g.history = null;
  g.selCommit = null;
  g.diffFor = "";
  if (g.overview?.branch) hub().app.setBranch(g.overview.branch);
}

/// What's wrong with `name` for a new branch, by Git's rules (git check-ref-format --branch).
/// Any name Git accepts is fine; the helper asks Git itself before making the branch.
export function branchNameProblem(name: string, existing: string[]): string {
  if (!name) return "";
  if (/\s/.test(name)) return "No spaces. Use dashes between words, like fix/save-menu-crash.";
  const invalid =
    name === "@" ||
    name === "HEAD" ||
    name.startsWith("-") ||
    name.startsWith("/") ||
    name.endsWith("/") ||
    name.endsWith(".") ||
    name.includes("..") ||
    name.includes("//") ||
    name.includes("@{") ||
    /[\u0000-\u001f\u007f~^:?*[\\]/.test(name) ||
    name.split("/").some((part) => part.startsWith(".") || part.endsWith(".lock"));
  if (invalid) return "Git doesn't accept that name. Use letters, numbers, dashes and slashes, like fix/save-menu-crash.";
  if (existing.some((b) => b.toLowerCase() === name.toLowerCase())) return "A branch with this name already exists.";
  return "";
}

async function newBranchDialog() {
  const o = g.overview!;
  if (o.editorRunning) {
    toast("Unreal Editor is open. Save your work and close it before you make a branch.", "error");
    return;
  }
  // The team can list the kinds of branch, like feat/ and fix/, in ue-dev-hub.toml.
  const prefixes = o.branchPrefixes ?? [];
  const name = await modal<string | null>((close) => {
    let kind = prefixes[0] ?? "";
    let words = "";
    const full = () => (kind ? `${kind}/${words}` : words);
    const err = h("p", { class: "small error-text min-line" });
    const preview = h("span", { class: "mono" }, full());
    const create = h("button", { class: "btn primary", disabled: true, onclick: () => close(full()) }, icon("plus", 16), "Make branch") as HTMLButtonElement;
    const refresh = () => {
      preview.textContent = full();
      const problem = words ? branchNameProblem(full(), o.branches) : "";
      err.textContent = problem;
      create.disabled = !words || !!problem;
    };
    const seg =
      prefixes.length > 0 &&
      h(
        "div",
        { class: "segmented wrap", role: "radiogroup", "aria-label": "Kind of branch" },
        ...prefixes.map((k) =>
          h(
            "button",
            {
              class: `seg ${k === kind ? "on" : ""}`,
              role: "radio",
              "aria-checked": k === kind ? "true" : "false",
              onclick: (e: Event) => {
                kind = k;
                const me = e.currentTarget as HTMLElement;
                me.parentElement!.querySelectorAll(".seg").forEach((el) => {
                  el.classList.toggle("on", el === me);
                  el.setAttribute("aria-checked", el === me ? "true" : "false");
                });
                refresh();
              },
            },
            k,
          ),
        ),
      );
    return h(
      "div",
      { class: "card" },
      h("h2", {}, "New branch"),
      seg && h("div", { class: "field" }, h("span", { class: "small muted" }, "What kind of work?"), seg),
      h(
        "label",
        { class: "field" },
        h("span", { class: "small muted" }, "Name"),
        h("input", {
          type: "text",
          name: "branch-name",
          placeholder: seg ? "save-menu-crash" : "fix/save-menu-crash",
          oninput: (e: Event) => {
            words = (e.target as HTMLInputElement).value.trim();
            refresh();
          },
          onkeydown: (e: KeyboardEvent) => {
            if (e.key === "Enter" && !create.disabled) close(full());
          },
        }),
        err,
      ),
      h("p", { class: "small muted" }, "Starts from the latest ", h("span", { class: "mono" }, o.baseBranch), o.files.length ? `. Your ${plural(o.files.length, "uncommitted change")} come along.` : "."),
      h(
        "div",
        { class: "row between wrap" },
        seg ? h("span", { class: "small muted" }, "Branch: ", preview) : h("span"),
        h("div", { class: "actions" }, h("button", { class: "btn", onclick: () => close(null) }, "Cancel"), create),
      ),
    );
  }, null);
  if (!name) return;
  const warnings = await run("branch", () => api.gitCreateBranch(name));
  if (warnings) {
    afterBranchChange();
    toast(`Made ${name} and switched to it.`);
    warnings.forEach((w) => toast(w, "error"));
  }
}
