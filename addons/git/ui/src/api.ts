// Calls to the Git addon's helper, and the types it returns. Steps (calls that change the repo)
// carry the name the hub shows while they run, and never run alongside other work.
import type { Commit, Hub, UpdateInfo, UpdateResult } from "ue-dev-hub-addon-api";

export type { Commit, UpdateInfo, UpdateResult };

export interface FileChange {
  path: string;
  origPath: string | null;
  /** "M" changed, "A" added, "D" deleted, "R" renamed, "U" new, "C" conflicted. */
  kind: "M" | "A" | "D" | "R" | "U" | "C";
  staged: boolean;
  unstaged: boolean;
  binary: boolean;
  /** A submodule with changes inside it, which this page can't discard. */
  submodule: boolean;
}

export interface GitOverview {
  /** The top of the repository, which every file path is relative to. */
  root: string;
  branch: string;
  baseBranch: string;
  onBase: boolean;
  branches: string[];
  onServer: boolean;
  ahead: number;
  incoming: number;
  files: FileChange[];
  repoBusy: string | null;
  editorRunning: boolean;
  strategy: "merge" | "rebase";
  pushPolicy: "ask" | "always" | "never";
  /** `git.branch_prefixes`: the kinds of branch to pick from, like "feat". Empty: any name. */
  branchPrefixes: string[];
}

export interface Diff {
  binary: boolean;
  text: string;
  truncated: boolean;
}

export interface ChangedPath {
  kind: "M" | "A" | "D" | "R";
  path: string;
  /** The old path of a renamed file. */
  origPath: string | null;
  binary: boolean;
}

export interface Stash {
  id: string;
  name: string;
  branch: string;
  when: string;
  files: ChangedPath[];
}

export interface GitHistory {
  commits: Commit[];
  unpushed: string[];
}

export interface CommitDetails {
  body: string;
  files: ChangedPath[];
}

export interface FileLock {
  path: string;
  owner: string;
  mine: boolean;
}

let L: Hub;

/** Called once when the addon is loaded. */
export function setHub(hub: Hub) {
  L = hub;
}

export function hub(): Hub {
  return L;
}

const read = <T>(method: string, params: Record<string, unknown> = {}) => L.call<T>(method, params);
let ownSteps = 0;

/** Steps of this page that are running. Newer hubs count them in `app.busy()` too. */
export const ownStepsRunning = () => ownSteps > 0;

const step = async <T>(name: string, method: string, params: Record<string, unknown> = {}) => {
  ownSteps++;
  try {
    return await L.call<T>(method, params, { step: name });
  } finally {
    ownSteps--;
  }
};

export const api = {
  checkUpdates: () => L.app.checkUpdates(),
  revealPath: (path: string) => L.app.revealPath(path),
  gitOverview: () => read<GitOverview>("overview"),
  gitDiff: (path: string) => read<Diff>("diff", { path }),
  gitStage: (paths: string[]) => step<void>("Adding files", "stage", { paths }),
  gitUnstage: (paths: string[]) => step<void>("Removing files from the commit", "unstage", { paths }),
  gitDiscard: (path: string) => step<void>("Discarding changes", "discard", { path }),
  /** `paths` is what the page shows as ticked, with both names of a rename. The helper commits nothing if Git's list differs. */
  gitCommit: (summary: string, body: string, paths: string[]) => step<string>("Committing", "commit", { summary, body, paths }),
  gitPush: () => step<{ commits: number; published: boolean }>("Pushing", "push"),
  gitPull: (push: boolean) => step<UpdateResult>("Updating", "pull", { push }),
  gitCreateBranch: (name: string) => step<string[]>("Making a branch", "createBranch", { name }),
  gitSwitchBranch: (name: string) => step<string[]>("Switching branches", "switchBranch", { name }),
  gitStashes: () => read<Stash[]>("stashes"),
  gitStashPush: (name: string, paths: string[] | null) => step<void>("Stashing", "stashPush", { name, paths }),
  gitStashPop: (id: string) => step<void>("Bringing back a stash", "stashPop", { id }),
  gitStashDrop: (id: string) => step<void>("Deleting a stash", "stashDrop", { id }),
  gitHistory: () => read<GitHistory>("history"),
  gitCommitDetails: (hash: string) => read<CommitDetails>("commitDetails", { hash }),
  gitCommitDiff: (hash: string, path: string) => read<Diff>("commitDiff", { hash, path }),
  gitFileHistory: (path: string) => read<Commit[]>("fileHistory", { path }),
  gitLocks: () => read<FileLock[]>("locks"),
  gitRemoveIndexLock: () => step<void>("Removing the Git lock file", "removeIndexLock"),
};
