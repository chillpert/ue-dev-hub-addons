//! The Git page: changed files, commits, pushing, branches, stashes and history.
//!
//! Team rules are enforced here, not only in the UI: no commits on the base branch, no merge
//! commits, no force push over someone else's work, no commits of files someone else has
//! locked, and nothing while Git is busy.

use crate::hub::git::{self, Commit};
use crate::hub::{Log, Project, config, proc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Diffs bigger than this are cut off, so a huge generated file can't freeze the page.
const MAX_DIFF: usize = 300_000;
/// Moving big art files can take hours, so a push or download is only stopped once it shows
/// no progress for this long.
const TRANSFER_IDLE: Duration = Duration::from_secs(5 * 60);

/// Every Git call from this page treats file names literally (a file called `Tex[1].png` must
/// never match `Tex1.png`), and reads never take `index.lock`, so refreshing the page can't
/// make another Git program fail.
const GLOBAL_ARGS: &[&str] = &["--literal-pathspecs", "--no-optional-locks"];

fn run(log: &Log, project: &Project, args: &[&str]) -> Result<proc::Output, String> {
    let mut all = GLOBAL_ARGS.to_vec();
    all.extend_from_slice(args);
    git::git(log, project, &all)
}

fn run_ok(log: &Log, project: &Project, args: &[&str], error: &str) -> Result<proc::Output, String> {
    let out = run(log, project, args)?;
    if out.ok { Ok(out) } else { Err(format!("{error}\n\n{}", out.message())) }
}

fn quiet() -> Log {
    Log::default()
}

/// Read-only Git call that doesn't write to the console.
fn read(project: &Project, args: &[&str]) -> Result<proc::Output, String> {
    run(&quiet(), project, args)
}

fn read_ok(project: &Project, args: &[&str], error: &str) -> Result<String, String> {
    Ok(run_ok(&quiet(), project, args, error)?.stdout)
}

/// Like `read_ok`, but the output isn't trimmed. For diffs.
fn read_raw(project: &Project, args: &[&str], error: &str) -> Result<String, String> {
    let mut all = GLOBAL_ARGS.to_vec();
    all.extend_from_slice(args);
    let out = git::git_raw(project, &all)?;
    if out.ok { Ok(out.stdout) } else { Err(format!("{error}\n\n{}", out.message())) }
}

/// Runs a long Git transfer, like a push with big art files, showing its progress in the log.
/// It's only stopped when it shows no progress for `TRANSFER_IDLE`, never for taking long.
fn run_streamed(log: &Log, project: &Project, args: &[&str], error: &str) -> Result<(), String> {
    log.info(format!("> git {}", args.join(" ")));
    let mut cmd = proc::command("git");
    // Git LFS only reports upload progress to a terminal unless asked to.
    cmd.args(GLOBAL_ARGS).args(args).current_dir(&project.root).env("GIT_LFS_FORCE_PROGRESS", "1");
    let (ok, lines) = proc::stream(log, cmd, Some(TRANSFER_IDLE), |_| true).map_err(|e| format!("{error}\n\n{e}"))?;
    if ok {
        return Ok(());
    }
    let mut tail: Vec<&str> = lines.iter().rev().filter(|l| !l.trim().is_empty()).take(8).map(String::as_str).collect();
    tail.reverse();
    Err(format!("{error}\n\n{}", tail.join("\n")))
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    /// The old path of a renamed file.
    pub orig_path: Option<String>,
    /// "M" changed, "A" added, "D" deleted, "R" renamed, "U" new (untracked), "C" conflicted.
    pub kind: String,
    /// Some or all of the change is staged for the next commit.
    pub staged: bool,
    /// Some or all of the change isn't staged.
    pub unstaged: bool,
    /// Unreal assets and other Git LFS files, which have no text diff.
    pub binary: bool,
    /// A submodule with changes inside it, which this page can't discard.
    pub submodule: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    /// The top of the repository, which every file path is relative to.
    pub root: String,
    pub branch: String,
    pub base_branch: String,
    pub on_base: bool,
    pub branches: Vec<String>,
    /// The branch exists on the server.
    pub on_server: bool,
    /// Commits here that aren't on the server yet.
    pub ahead: usize,
    /// New commits on the base branch since the last check.
    pub incoming: usize,
    pub files: Vec<FileChange>,
    pub repo_busy: Option<String>,
    pub editor_running: bool,
    /// "merge" or "rebase". The Git page only updates by rebasing.
    pub strategy: String,
    /// "ask", "always" or "never".
    pub push_policy: String,
    /// The kinds of branch to pick from for a new one, like "feat". Empty: any name.
    pub branch_prefixes: Vec<String>,
}

fn has_head(project: &Project) -> bool {
    read(project, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok_and(|o| o.ok)
}

/// The current branch, also before its first commit.
fn current_branch(project: &Project) -> Result<String, String> {
    let out = read(project, &["symbolic-ref", "--short", "-q", "HEAD"])?;
    if out.ok && !out.stdout.is_empty() {
        return Ok(out.stdout);
    }
    if !read(project, &["rev-parse", "--git-dir"]).is_ok_and(|o| o.ok) {
        return Err("This folder isn't a Git repository.".into());
    }
    Err("You're not on a branch right now (detached HEAD). Switch to a branch in your Git program first.".into())
}

/// Branch names are case-insensitive on Windows, so `Develop` is the base branch too.
pub fn is_base(branch: &str, base: &str) -> bool {
    branch.eq_ignore_ascii_case(base)
}

/// Refuses while another program uses Git, or a rebase or merge is unfinished.
fn ensure_idle(project: &Project) -> Result<(), String> {
    match git::repo_busy(project) {
        Some(busy) => Err(busy),
        None => Ok(()),
    }
}

/// Refuses steps that rewrite files while Unreal Editor holds them open.
fn ensure_editor_closed(project: &Project, what: &str) -> Result<(), String> {
    if project.editor_running() {
        return Err(format!("Unreal Editor is open. Save your work and close it before you {what}, so it doesn't hold or overwrite the files that change."));
    }
    Ok(())
}

/// Finding the editor's command line is slow, and the page refreshes often, so the answer for
/// the overview is reused for a few seconds. Steps that change files always check afresh.
static EDITOR_SEEN: Mutex<Option<(Instant, PathBuf, bool)>> = Mutex::new(None);

fn editor_running_cached(project: &Project) -> bool {
    let mut seen = EDITOR_SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, path, running)) = seen.as_ref()
        && at.elapsed() < Duration::from_secs(5)
        && *path == project.uproject
    {
        return *running;
    }
    let running = project.editor_running();
    *seen = Some((Instant::now(), project.uproject.clone(), running));
    running
}

/// A file name that isn't valid UTF-8 reaches the page with replacement characters, so it no
/// longer names the real file. Such files are refused instead of changing the wrong thing.
pub fn is_unreadable_name(path: &str) -> bool {
    path.contains('\u{FFFD}')
}

fn check_paths(paths: &[String]) -> Result<(), String> {
    if let Some(bad) = paths.iter().find(|p| is_unreadable_name(p)) {
        return Err(format!("The name of '{bad}' uses characters the hub can't read, so it can't change this file. Rename it, or use another Git program for it."));
    }
    match paths.iter().find(|p| !config::is_inside(p)) {
        Some(bad) => Err(format!("'{bad}' isn't a file inside the project.")),
        None if paths.is_empty() => Err("No files given.".into()),
        None => Ok(()),
    }
}

fn is_asset(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".uasset") || lower.ends_with(".umap")
}

/// Which of `paths` Git LFS stores, asked in batches so long lists fit on a command line.
fn lfs_paths(project: &Project, paths: &[&str]) -> HashSet<String> {
    let mut found = HashSet::new();
    for chunk in paths.chunks(200) {
        let mut args = vec!["check-attr", "-z", "filter", "--"];
        args.extend_from_slice(chunk);
        let Ok(out) = read(project, &args) else { continue };
        // -z output: path NUL attribute NUL value NUL ...
        let parts: Vec<&str> = out.stdout.split('\0').collect();
        for triple in parts.chunks(3) {
            if let [path, _, value] = triple
                && *value == "lfs"
            {
                found.insert(path.to_string());
            }
        }
    }
    found
}

/// Assets and anything Git LFS stores, which `git diff` would only show as a pointer.
fn is_binary_path(project: &Project, path: &str) -> bool {
    is_asset(path) || lfs_paths(project, &[path]).contains(path)
}

/// Parses `git status --porcelain=v2 -z --untracked-files=all`.
pub fn parse_status(out: &str) -> Vec<FileChange> {
    let mut files = vec![];
    let mut parts = out.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = parts.next() {
        let (xy, sub, path, orig) = match entry.as_bytes().first() {
            Some(b'1') => {
                let f: Vec<&str> = entry.splitn(9, ' ').collect();
                (f.get(1).copied(), f.get(2).copied(), f.get(8).copied(), None)
            }
            Some(b'2') => {
                let f: Vec<&str> = entry.splitn(10, ' ').collect();
                (f.get(1).copied(), f.get(2).copied(), f.get(9).copied(), parts.next())
            }
            Some(b'u') => {
                let f: Vec<&str> = entry.splitn(11, ' ').collect();
                (Some("UU"), f.get(2).copied(), f.get(10).copied(), None)
            }
            Some(b'?') => (Some("??"), None, entry.get(2..), None),
            _ => continue,
        };
        let (Some(xy), Some(path)) = (xy, path) else { continue };
        let mut chars = xy.chars();
        let (x, y) = (chars.next().unwrap_or('.'), chars.next().unwrap_or('.'));
        let kind = match (entry.as_bytes()[0], if x != '.' { x } else { y }) {
            (b'?', _) => "U",
            (b'u', _) => "C",
            (_, 'A' | 'C') => "A",
            (_, 'D') => "D",
            (_, 'R') => "R",
            _ => "M",
        };
        let plain = kind == "U" || kind == "C";
        files.push(FileChange {
            path: path.to_string(),
            orig_path: orig.map(str::to_string),
            kind: kind.into(),
            staged: !plain && x != '.',
            unstaged: plain || y != '.',
            binary: is_asset(path),
            submodule: sub.is_some_and(|s| s.starts_with('S')),
        });
    }
    files
}

pub fn changes(project: &Project) -> Result<Vec<FileChange>, String> {
    let out = read_ok(project, &["status", "--porcelain=v2", "-z", "--untracked-files=all"], "Couldn't read your changes.")?;
    let mut files = parse_status(&out);
    let others: Vec<&str> = files.iter().filter(|f| !f.binary && !f.submodule).map(|f| f.path.as_str()).collect();
    let lfs = lfs_paths(project, &others);
    for f in &mut files {
        f.binary |= lfs.contains(&f.path);
    }
    Ok(files)
}

/// Full ref names, cut here: `refname:short` turns a branch into "heads/x" when a tag is also
/// called "x".
fn local_branches(project: &Project) -> Result<Vec<String>, String> {
    let out = read_ok(project, &["for-each-ref", "--format=%(refname)", "refs/heads"], "Couldn't list the branches.")?;
    Ok(out.lines().filter_map(|l| l.strip_prefix("refs/heads/")).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

fn ref_exists(project: &Project, r: &str) -> bool {
    read(project, &["rev-parse", "--verify", "--quiet", r]).is_ok_and(|o| o.ok)
}

fn remote_ref(branch: &str) -> String {
    format!("refs/remotes/origin/{branch}")
}

/// Where the base branch's latest commits are: the server's copy, if it's known. Full ref
/// names, so a local branch called "origin/develop" can't stand in for it.
fn base_ref(project: &Project) -> String {
    let base = project.default_branch();
    let remote = remote_ref(&base);
    if ref_exists(project, &remote) { remote } else { format!("refs/heads/{base}") }
}

fn count_revs(project: &Project, revs: &[String]) -> Result<usize, String> {
    let mut args = vec!["rev-list", "--count"];
    args.extend(revs.iter().map(String::as_str));
    // "--" so a file called HEAD can't be taken for the revision.
    args.push("--");
    let out = read_ok(project, &args, "Couldn't compare your branch with the server.")?;
    out.trim().parse().map_err(|_| format!("Git gave an unexpected answer: {out}"))
}

/// The commits here that aren't on the server yet: not on your branch there, and not on the
/// base branch either (after an update, the team's new commits aren't yours to push).
fn unpushed_revs(project: &Project, branch: &str) -> Vec<String> {
    let mut revs = vec!["HEAD".to_string()];
    let remote = remote_ref(branch);
    if ref_exists(project, &remote) {
        revs.push(format!("^{remote}"));
    }
    let base = base_ref(project);
    if ref_exists(project, &base) && !revs.contains(&format!("^{base}")) {
        revs.push(format!("^{base}"));
    }
    revs
}

/// The branches here and on the server, from one Git call instead of one per question.
struct Refs(HashSet<String>);

impl Refs {
    fn read(project: &Project) -> Result<Refs, String> {
        let out = read_ok(project, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes/origin"], "Couldn't list the branches.")?;
        Ok(Refs(out.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()))
    }

    fn has(&self, full_name: &str) -> bool {
        self.0.contains(full_name)
    }

    fn local_branches(&self) -> Vec<String> {
        let mut out: Vec<String> = self.0.iter().filter_map(|r| r.strip_prefix("refs/heads/")).filter(|b| !b.is_empty()).map(str::to_string).collect();
        out.sort();
        out
    }

    /// Like `unpushed_revs`, without asking Git again.
    fn unpushed_revs(&self, project: &Project, branch: &str) -> Vec<String> {
        let mut revs = vec!["HEAD".to_string()];
        let remote = remote_ref(branch);
        if self.has(&remote) {
            revs.push(format!("^{remote}"));
        }
        let base = project.default_branch();
        let base = if self.has(&remote_ref(&base)) { remote_ref(&base) } else { format!("refs/heads/{base}") };
        if self.has(&base) && !revs.contains(&format!("^{base}")) {
            revs.push(format!("^{base}"));
        }
        revs
    }
}

/// Everything the Git page shows at the top. The slow part is reading the changed files, so it
/// runs alongside the other questions instead of before them.
pub fn overview(project: &Project) -> Result<Overview, String> {
    // An unfinished rebase leaves HEAD detached, so it's asked first: its message is the one
    // that helps, not "detached HEAD".
    let repo_busy = git::repo_busy(project);
    let editor_running = editor_running_cached(project);
    let base_branch = project.default_branch();
    std::thread::scope(|s| {
        let files = s.spawn(|| changes(project));
        let branch = match current_branch(project) {
            Ok(b) => b,
            Err(e) => return Err(repo_busy.clone().unwrap_or(e)),
        };
        let refs = Refs::read(project)?;
        let on_base = is_base(&branch, &base_branch);
        let on_server = refs.has(&remote_ref(&branch));
        // A branch with no commits yet has no ref, and nothing to compare.
        let (ahead, incoming) = if refs.has(&format!("refs/heads/{branch}")) {
            let target = git::update_target(on_base, &base_branch);
            let incoming = refs.has(&target).then(|| s.spawn(move || count_revs(project, &[target, "^HEAD".into()])));
            let ahead = count_revs(project, &refs.unpushed_revs(project, &branch))?;
            let incoming = match incoming {
                Some(t) => t.join().map_err(|_| "Couldn't compare your branch with the server.".to_string())??,
                None => 0,
            };
            (ahead, incoming)
        } else {
            (0, 0)
        };
        let files = files.join().map_err(|_| "Couldn't read your changes.".to_string())??;
        Ok(Overview {
            root: project.root.display().to_string(),
            branches: refs.local_branches(),
            on_server,
            ahead,
            incoming,
            files,
            repo_busy,
            editor_running,
            strategy: project.config.git.update.clone(),
            push_policy: project.config.git.push.clone(),
            branch_prefixes: project.config.git.branch_prefixes.clone(),
            branch,
            base_branch: base_branch.clone(),
            on_base,
        })
    })
}

// ---------- Diffs ----------

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub binary: bool,
    /// Unified diff text.
    pub text: String,
    pub truncated: bool,
}

fn finish_diff(text: String) -> Diff {
    if text.lines().any(|l| l.starts_with("Binary files ") && l.ends_with(" differ")) {
        return Diff { binary: true, ..Default::default() };
    }
    if text.len() > MAX_DIFF {
        let mut end = MAX_DIFF;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let cut = text[..end].rfind('\n').unwrap_or(end);
        return Diff { binary: false, text: text[..cut].to_string(), truncated: true };
    }
    Diff { binary: false, text, truncated: false }
}

/// A new file shown as one added hunk. Only the start of a big file is read.
fn new_file_diff(full: &Path) -> Result<Diff, String> {
    use std::io::Read;
    let file = std::fs::File::open(full).map_err(|e| format!("Couldn't read {}: {e}", full.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_DIFF as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("Couldn't read {}: {e}", full.display()))?;
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return Ok(Diff { binary: true, ..Default::default() });
    }
    let more = bytes.len() > MAX_DIFF;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = format!("@@ -0,0 +1,{} @@\n", lines.len());
    for l in lines {
        out.push('+');
        out.push_str(l);
        out.push('\n');
    }
    let mut diff = finish_diff(out);
    diff.truncated |= more;
    Ok(diff)
}

/// Everything that changed in `path` since the last commit, staged or not.
pub fn diff(project: &Project, path: &str) -> Result<Diff, String> {
    check_paths(&[path.to_string()])?;
    if is_binary_path(project, path) {
        return Ok(Diff { binary: true, ..Default::default() });
    }
    let head = has_head(project);
    let in_index = read(project, &["ls-files", "--error-unmatch", "--", path]).is_ok_and(|o| o.ok);
    let in_head = head && read(project, &["cat-file", "-e", &format!("HEAD:{path}")]).is_ok_and(|o| o.ok);
    if !in_index && !in_head {
        return new_file_diff(&project.root.join(path));
    }
    let base = if head { "HEAD" } else { "--cached" };
    let out = read_raw(project, &["-c", "core.quotePath=false", "diff", base, "--no-color", "--no-ext-diff", "-M", "--", path], "Couldn't compare the file.")?;
    Ok(finish_diff(out))
}

// ---------- Staging and discarding ----------

/// `paths` plus the old names of renamed files among them, so a rename is always ticked or
/// unticked as a whole. Unticking only the new name would leave the old file's deletion staged.
fn with_renames(project: &Project, paths: &[String]) -> Result<Vec<String>, String> {
    let mut all = paths.to_vec();
    for f in changes(project)? {
        if let Some(orig) = f.orig_path
            && paths.contains(&f.path)
            && !all.contains(&orig)
        {
            all.push(orig);
        }
    }
    Ok(all)
}

pub fn stage(log: &Log, project: &Project, paths: &[String]) -> Result<(), String> {
    check_paths(paths)?;
    ensure_idle(project)?;
    let paths = with_renames(project, paths)?;
    let mut args = vec!["add", "-A", "--"];
    args.extend(paths.iter().map(String::as_str));
    run_ok(log, project, &args, "Couldn't add the files to the commit.")?;
    Ok(())
}

pub fn unstage(log: &Log, project: &Project, paths: &[String]) -> Result<(), String> {
    check_paths(paths)?;
    ensure_idle(project)?;
    let paths = with_renames(project, paths)?;
    let mut args = if has_head(project) { vec!["restore", "--staged", "--"] } else { vec!["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"] };
    args.extend(paths.iter().map(String::as_str));
    run_ok(log, project, &args, "Couldn't take the files out of the commit.")?;
    Ok(())
}

fn git_dir(project: &Project) -> Result<PathBuf, String> {
    Ok(PathBuf::from(read_ok(project, &["rev-parse", "--absolute-git-dir"], "This folder isn't a Git repository.")?.trim()))
}

/// Puts a file back to how it was in the last commit. Your version is copied aside first and
/// handed to `keep` (the trash in the app) only once Git succeeded. If any step fails,
/// your version is put back, so a discard either happens completely or not at all.
pub fn discard_with(log: &Log, project: &Project, path: &str, keep: impl Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    check_paths(&[path.to_string()])?;
    ensure_idle(project)?;
    let file = changes(project)?.into_iter().find(|f| f.path == path).ok_or_else(|| format!("{path} has no changes to discard."))?;
    if file.kind == "C" {
        return Err(format!("{path} is in the middle of a conflict. Finish or abort that in your Git program first."));
    }
    if file.submodule {
        return Err(format!("{path} is a submodule. Discard the changes inside it from its own folder in your Git program."));
    }
    if file.binary {
        ensure_editor_closed(project, "discard changes to an asset")?;
    }
    let head = has_head(project);
    let paths: Vec<&str> = std::iter::once(file.path.as_str()).chain(file.orig_path.as_deref()).collect();
    let (restore, remove): (Vec<&str>, Vec<&str>) =
        paths.iter().partition(|p| head && read(project, &["cat-file", "-e", &format!("HEAD:{p}")]).is_ok_and(|o| o.ok));

    let git_dir = git_dir(project)?;
    let aside = git_dir.join("ue-dev-hub").join("discarding").join(chrono::Local::now().format("%Y%m%d-%H%M%S%f").to_string());
    let keep_copy = |from: &Path, to: &Path, what: &str| -> Result<(), String> {
        std::fs::create_dir_all(to.parent().unwrap()).and_then(|_| std::fs::copy(from, to)).map(|_| ()).map_err(|e| format!("Couldn't keep a copy of {what}, so nothing was discarded: {e}"))
    };
    let mut copies: Vec<(PathBuf, PathBuf)> = vec![];
    // Files that don't exist now (the old name of a rename), so a rollback removes them again.
    let mut absent: Vec<PathBuf> = vec![];
    for (i, p) in paths.iter().enumerate() {
        let full = project.root.join(p);
        if full.is_file() {
            let copy = aside.join(i.to_string()).join(full.file_name().unwrap_or_default());
            keep_copy(&full, &copy, p)?;
            copies.push((full, copy));
        } else if !full.exists() {
            absent.push(full);
        }
    }
    // What's ticked lives in the index, which the steps below change too.
    let index = git_dir.join("index");
    let index_copy = aside.join("index");
    let saved_index = index.is_file();
    if saved_index {
        keep_copy(&index, &index_copy, "what's ticked")?;
    }
    let result = (|| -> Result<(), String> {
        if !restore.is_empty() {
            let mut args = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
            args.extend(&restore);
            run_ok(log, project, &args, &format!("Couldn't put {path} back."))?;
        }
        for p in &remove {
            run_ok(log, project, &["rm", "--cached", "-q", "--ignore-unmatch", "--", p], &format!("Couldn't remove {p} from the commit."))?;
            let full = project.root.join(p);
            if full.is_file() {
                std::fs::remove_file(&full).map_err(|e| format!("Couldn't remove {p}: {e}"))?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        let mut restored = true;
        for (full, copy) in &copies {
            let _ = std::fs::create_dir_all(full.parent().unwrap_or(&project.root));
            restored &= std::fs::copy(copy, full).is_ok();
        }
        for full in &absent {
            if full.is_file() {
                restored &= std::fs::remove_file(full).is_ok();
            }
        }
        // Put the index back only if a step changed it: when it failed because another program
        // holds Git's lock, writing it would get in that program's way.
        if saved_index && std::fs::read(&index).ok() != std::fs::read(&index_copy).ok() {
            restored &= std::fs::copy(&index_copy, &index).is_ok();
        }
        if !restored {
            return Err(format!("{e}\n\nThe hub couldn't put everything back. Your version is kept in {}.", aside.display()));
        }
        let _ = std::fs::remove_dir_all(&aside);
        return Err(format!("{e}\n\nYour version of the file was put back."));
    }
    for (_, copy) in &copies {
        keep(copy).map_err(|e| format!("{e}\n\nYour version is kept in {}.", aside.display()))?;
    }
    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
}

pub fn to_trash(path: &Path) -> Result<(), String> {
    let bin = if cfg!(windows) { "the Recycle Bin" } else { "the trash" };
    trash::delete(path).map_err(|e| format!("Couldn't move {} to {bin}: {e}", path.display()))
}

// ---------- Committing and pushing ----------

/// Commits what's ticked. `ticked` is what the page showed as ticked (with both names of a
/// renamed file); if the index holds anything else, nothing is committed, so a tick that failed
/// or a change from another program never slips into the commit unseen.
pub fn commit(log: &Log, project: &Project, summary: &str, body: &str, ticked: &[String]) -> Result<String, String> {
    commit_with(log, project, summary, body, ticked, |p| locks_within(p, 20))
}

fn commit_with(log: &Log, project: &Project, summary: &str, body: &str, ticked: &[String], locks: impl Fn(&Project) -> Result<Vec<Lock>, String>) -> Result<String, String> {
    ensure_idle(project)?;
    let branch = current_branch(project)?;
    let base = project.default_branch();
    if is_base(&branch, &base) {
        return Err(format!("You can't commit to {base}. Changes reach {base} only through a pull request. Make a branch first; your changes come with you."));
    }
    let summary = summary.trim();
    if summary.is_empty() {
        return Err("Write a short summary of what you changed.".into());
    }
    // Without renames, so both names of a renamed file are listed, the same way the page sends them.
    let staged = read_ok(project, &["-c", "core.quotePath=false", "diff", "--cached", "--name-only", "--no-renames", "-z"], "Couldn't read what's ticked.")?;
    let staged: Vec<&str> = staged.split('\0').filter(|s| !s.is_empty()).collect();
    if staged.is_empty() {
        return Err("Tick the files you want to include first.".into());
    }
    let expected: HashSet<&str> = ticked.iter().map(String::as_str).collect();
    if staged.iter().copied().collect::<HashSet<&str>>() != expected {
        return Err("The ticked files changed since the page last looked, so nothing was committed. Check the list, then commit again.".into());
    }
    // Without a connection to the lock server the commit goes ahead; pushing checks locks again.
    if let Ok(locks) = locks(project)
        && let Some(l) = locks.iter().find(|l| !l.mine && staged.contains(&l.path.as_str()))
    {
        return Err(format!("{} is locked by {}, so you can't commit it while they hold the lock. Untick it.", l.path, l.owner));
    }
    let mut args = vec!["commit", "-q", "-m", summary];
    let body = body.trim();
    if !body.is_empty() {
        args.extend(["-m", body]);
    }
    let out = run(log, project, &args)?;
    if !out.ok {
        let said = out.message();
        if said.contains("Please tell me who you are") || said.contains("unable to auto-detect email") {
            return Err("Git doesn't know your name and email yet. Open a terminal on the Tools page and run:\n\ngit config --global user.name \"Your Name\"\ngit config --global user.email you@example.com".into());
        }
        return Err(format!("Couldn't commit.\n\n{said}"));
    }
    Ok(read_ok(project, &["rev-parse", "--verify", "--short", "HEAD"], "Couldn't read the new commit.")?.trim().to_string())
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PushResult {
    pub commits: usize,
    /// The branch wasn't on the server before.
    pub published: bool,
}

pub fn push(log: &Log, project: &Project) -> Result<PushResult, String> {
    ensure_idle(project)?;
    let branch = current_branch(project)?;
    let base = project.default_branch();
    if is_base(&branch, &base) {
        return Err(format!("You can't push to {base}. Changes reach {base} only through a pull request."));
    }
    if project.config.git.push == "never" {
        return Err("This project is set to never push from the hub (git.push in ue-dev-hub.toml).".into());
    }
    if !has_head(project) {
        return Err("There's nothing to push yet. Commit something first.".into());
    }
    let refspec = format!("HEAD:refs/heads/{branch}");
    let remote = remote_ref(&branch);
    if !on_server(log, project, &branch)? {
        // Never pushed, or deleted on the server since: a copy of it from before is out of date.
        if ref_exists(project, &remote) {
            run_ok(log, project, &["update-ref", "-d", &remote], "Couldn't forget the old copy of your branch.")?;
        }
        let commits = count_revs(project, &unpushed_revs(project, &branch))?;
        run_streamed(log, project, &["push", "--progress", "-u", "origin", &refspec], "Couldn't put your branch on the server. Check your connection and try again.")?;
        note_pushed(log, project, &remote);
        return Ok(PushResult { commits, published: true });
    }
    // An explicit target, so the server's copy is updated also in clones that only fetch some
    // branches (single-branch clones, custom fetch settings).
    let fetch = format!("+refs/heads/{branch}:{remote}");
    run_ok(log, project, &["fetch", "origin", &fetch], "Couldn't check your branch on the server. Check your connection and try again.")?;
    let commits = count_revs(project, &unpushed_revs(project, &branch))?;
    // Read the server's commit once. Every check below and the push itself use exactly this
    // commit, so a teammate's push that lands in between is never overwritten.
    let server = read_ok(project, &["rev-parse", "--verify", &remote], "Couldn't read your branch on the server.")?.trim().to_string();
    let server_only = count_revs(project, &[server.clone(), "^HEAD".into()])?;
    if server_only == 0 {
        // A normal push: Git refuses it by itself if the server moved meanwhile.
        run_streamed(log, project, &["push", "--progress", "-u", "origin", &refspec], "Couldn't push. Check your connection and try again.")?;
    } else if git::foreign_server_commits(log, project, &branch, &server)? == 0 {
        // The server only has your commits from before an update rebased them.
        let lease = format!("--force-with-lease=refs/heads/{branch}:{server}");
        run_streamed(log, project, &["push", "--progress", "-u", &lease, "origin", &refspec], "Couldn't push. Your branch on the server changed meanwhile, or the connection dropped. Press Check, then try again.")?;
    } else {
        return Err(format!(
            "Your branch '{branch}' on the server has {server_only} commit(s) that aren't on this computer, probably pushed from another computer or by a teammate. Pushing would overwrite them, so it's blocked."
        ));
    }
    note_pushed(log, project, &remote);
    Ok(PushResult { commits, published: false })
}

/// Whether `branch` is on the server. Asks for exactly `refs/heads/{branch}`: a plain name would
/// also match `refs/heads/someone/{branch}`. Errors when the server can't be asked, instead of
/// taking that for "not there".
fn on_server(log: &Log, project: &Project, branch: &str) -> Result<bool, String> {
    let full = format!("refs/heads/{branch}");
    let out = run(log, project, &["ls-remote", "origin", &full])?;
    if !out.ok {
        return Err(format!("Couldn't reach the server. Check your connection and try again.\n\n{}", out.message()));
    }
    Ok(out.stdout.lines().any(|l| l.split('\t').nth(1) == Some(full.as_str())))
}

/// After a push, the server has HEAD. Git only records that itself when the clone fetches this
/// branch, so it's recorded here; otherwise the page would keep offering to push.
fn note_pushed(log: &Log, project: &Project, remote: &str) {
    let _ = run(log, project, &["update-ref", remote, "HEAD"]);
}

/// The update from the Launch screen, but only by rebasing: the Git page never makes merge commits.
pub fn pull(log: &Log, project: &Project, push: bool) -> Result<git::UpdateResult, String> {
    if project.config.git.update != "rebase" {
        return Err("This project updates by merging (git.update in ue-dev-hub.toml). The Git page only rebases, so update from the Launch page instead.".into());
    }
    git::apply_update(log, project, push)
}

// ---------- Branches ----------

/// Whether Git accepts `name` as a branch name, exactly as written. Git decides, so the page
/// allows every name Git does, like `feat/x+y` or non-English letters.
fn is_branch_name(project: &Project, name: &str) -> bool {
    // check-ref-format would expand "@{-1}" to another branch's name, so the answer must match.
    !name.is_empty() && !name.starts_with('-') && name != "@" && read(project, &["check-ref-format", "--branch", name]).is_ok_and(|o| o.ok && o.stdout == name)
}

/// What's wrong with `name` for a new branch, if anything.
pub fn branch_name_problem(project: &Project, name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Give the branch a name.");
    }
    if name.chars().any(char::is_whitespace) {
        return Some("No spaces. Use dashes between words, like fix/save-menu-crash.");
    }
    if !is_branch_name(project, name) {
        return Some("Git doesn't accept that name. Use letters, numbers, dashes and slashes, like fix/save-menu-crash.");
    }
    None
}

/// Downloads the art and audio files a checkout of `target` needs before switching, so a
/// dropped connection fails here, with nothing changed, instead of halfway through the switch.
fn lfs_prefetch(log: &Log, project: &Project, target: &str, what: &str) -> Result<(), String> {
    if !read(project, &["lfs", "version"]).is_ok_and(|o| o.ok) {
        return Ok(());
    }
    log.info("Downloading the art and audio files (Git LFS) ...");
    let mut cmd = proc::command("git");
    cmd.args(["lfs", "fetch", "origin", target]).current_dir(&project.root).env("GIT_LFS_FORCE_PROGRESS", "1");
    let (ok, lines) = proc::stream(log, cmd, Some(TRANSFER_IDLE), |_| true)?;
    if !ok {
        let tail: Vec<&str> = lines.iter().rev().take(6).rev().map(String::as_str).collect();
        return Err(format!("Couldn't download the art and audio files for {what}. Check your connection and try again. Nothing was changed.\n\n{}", tail.join("\n")));
    }
    Ok(())
}

/// Runs `git switch`. Git reports a failing post-checkout hook as a failed switch even though
/// the switch happened, so success is judged by where HEAD ended up.
fn switch(log: &Log, project: &Project, args: &[&str], name: &str) -> Result<Vec<String>, String> {
    let out = run(log, project, args)?;
    let mut warnings = vec![];
    if !out.ok {
        if current_branch(project).ok().as_deref() != Some(name) {
            return Err(out.message());
        }
        warnings.push(format!("Switched to {name}, but a Git hook reported a problem:\n\n{}", out.message()));
    }
    warnings.extend(git::update_submodules(log, project));
    Ok(warnings)
}

pub fn create_branch(log: &Log, project: &Project, name: &str) -> Result<Vec<String>, String> {
    if let Some(problem) = branch_name_problem(project, name) {
        return Err(problem.into());
    }
    ensure_idle(project)?;
    ensure_editor_closed(project, "make a branch")?;
    let taken = local_branches(project)?.iter().any(|b| b.eq_ignore_ascii_case(name)) || ref_exists(project, &remote_ref(name));
    if taken {
        return Err(format!("A branch called {name} already exists."));
    }
    let base = project.default_branch();
    // Start from the latest base branch. Without a connection, the last known one has to do.
    let online = run(log, project, &["fetch", "origin", &base]).is_ok_and(|o| o.ok);
    if !online {
        log.info(format!("Couldn't download the latest {base}, so the branch starts from the last known one."));
    }
    let start = base_ref(project);
    if online {
        lfs_prefetch(log, project, &start, &format!("the latest {base}"))?;
    }
    switch(log, project, &["switch", "--no-track", "-c", name, &start], name).map_err(|said| {
        if said.contains("would be overwritten") {
            format!("Your uncommitted changes touch files that are different on the latest {base}, so they can't come along. Stash them first, then make the branch.")
        } else {
            format!("Couldn't make the branch.\n\n{said}")
        }
    })
}

pub fn switch_branch(log: &Log, project: &Project, name: &str) -> Result<Vec<String>, String> {
    // Any branch Git lists can be switched to; a leading dash would be read as an option.
    if name.starts_with('-') || !local_branches(project)?.iter().any(|b| b == name) {
        return Err(format!("There's no branch called {name} on this computer."));
    }
    ensure_idle(project)?;
    if !changes(project)?.is_empty() {
        return Err("You have uncommitted changes. Commit or stash them first, so they don't end up on the other branch.".into());
    }
    ensure_editor_closed(project, "switch branches")?;
    lfs_prefetch(log, project, &format!("refs/heads/{name}"), name)?;
    switch(log, project, &["switch", name], name).map_err(|said| format!("Couldn't switch branches.\n\n{said}"))
}

// ---------- Stashes ----------

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Stash {
    /// The stash commit, which stays the same when other stashes come and go.
    pub id: String,
    pub name: String,
    pub branch: String,
    pub when: String,
    pub files: Vec<ChangedPath>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedPath {
    pub kind: String,
    pub path: String,
    /// The old path of a renamed file.
    pub orig_path: Option<String>,
    pub binary: bool,
}

/// Parses `--name-status -z` output.
fn parse_name_status(out: &str) -> Vec<ChangedPath> {
    let mut files = vec![];
    let mut parts = out.split('\0').filter(|s| !s.is_empty());
    while let Some(status) = parts.next() {
        let first = status.chars().next();
        let kind = match first {
            Some('A' | 'C') => "A",
            Some('D') => "D",
            Some('R') => "R",
            _ => "M",
        };
        let orig = if matches!(first, Some('R' | 'C')) { parts.next() } else { None };
        if let Some(path) = parts.next() {
            files.push(ChangedPath {
                kind: kind.into(),
                path: path.into(),
                // A copy keeps its source, so only a rename has an old path that changed.
                orig_path: orig.filter(|_| kind == "R").map(str::to_string),
                binary: is_asset(path),
            });
        }
    }
    files
}

/// "On feat/x: My name" or "WIP on feat/x: 1a2b3c Last commit" to (branch, name).
fn parse_stash_subject(subject: &str) -> (String, String) {
    let rest = subject.strip_prefix("WIP on ").or_else(|| subject.strip_prefix("On ")).unwrap_or(subject);
    match rest.split_once(": ") {
        Some((branch, name)) if subject.starts_with("WIP on ") => (branch.into(), format!("Changes on {branch}: {}", name.split_once(' ').map_or(name, |(_, s)| s))),
        Some((branch, name)) => (branch.into(), name.into()),
        None => (String::new(), rest.into()),
    }
}

/// A stash never changes, so its file list is read once per stash and kept.
static STASH_FILES: Mutex<Option<HashMap<String, Vec<ChangedPath>>>> = Mutex::new(None);

fn stash_files(project: &Project, id: &str) -> Result<Vec<ChangedPath>, String> {
    if let Some(files) = STASH_FILES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(id)) {
        return Ok(files.clone());
    }
    let out = read_ok(
        project,
        &["-c", "core.quotePath=false", "stash", "show", "--name-status", "-z", "--include-untracked", id],
        "Couldn't read what's in the stash. The hub needs Git 2.32 or newer for this.",
    )?;
    let files = parse_name_status(&out);
    STASH_FILES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id.to_string(), files.clone());
    Ok(files)
}

fn stash_ids(project: &Project) -> Result<Vec<String>, String> {
    let out = read_ok(project, &["stash", "list", "--format=%H"], "Couldn't read your stashes.")?;
    Ok(out.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
}

pub fn stashes(project: &Project) -> Result<Vec<Stash>, String> {
    let out = read_ok(project, &["stash", "list", "--format=%H%x1f%gs%x1f%cr"], "Couldn't read your stashes.")?;
    let mut list = vec![];
    for line in out.lines().filter(|l| !l.is_empty()) {
        let mut f = line.splitn(3, '\u{1f}');
        let (Some(id), Some(subject), Some(when)) = (f.next(), f.next(), f.next()) else { continue };
        let (branch, name) = parse_stash_subject(subject);
        let files = stash_files(project, id).unwrap_or_default();
        list.push(Stash { id: id.into(), name, branch, when: when.into(), files });
    }
    Ok(list)
}

/// `stash@{n}` for a stash id, looked up right before use.
fn stash_ref(project: &Project, id: &str) -> Result<String, String> {
    stash_ids(project)?.iter().position(|l| l == id).map(|i| format!("stash@{{{i}}}")).ok_or_else(|| "That stash doesn't exist anymore.".into())
}

pub fn stash_push(log: &Log, project: &Project, name: &str, paths: Option<Vec<String>>) -> Result<(), String> {
    ensure_idle(project)?;
    let paths = match paths {
        Some(p) => {
            check_paths(&p)?;
            Some(with_renames(project, &p)?)
        }
        None => None,
    };
    if changes(project)?.is_empty() {
        return Err("There are no changes to stash.".into());
    }
    ensure_editor_closed(project, "stash changes")?;
    let before = stash_ids(project)?.len();
    let branch = current_branch(project)?;
    let name = if name.trim().is_empty() { format!("Changes on {branch}") } else { name.trim().to_string() };
    // `--literal-pathspecs` stops `stash push` from finding untracked files, so this call marks
    // each path as literal itself instead.
    let literal: Vec<String> = paths.iter().flatten().map(|p| format!(":(literal){p}")).collect();
    let mut args = vec!["--no-optional-locks", "stash", "push", "--include-untracked", "-m", &name];
    if !literal.is_empty() {
        args.push("--");
        args.extend(literal.iter().map(String::as_str));
    }
    git::git_ok(log, project, &args, "Couldn't stash your changes.")?;
    if stash_ids(project)?.len() <= before {
        return Err("Git didn't stash anything. Changes inside submodules can't be stashed from here.".into());
    }
    Ok(())
}

pub fn stash_pop(log: &Log, project: &Project, id: &str) -> Result<(), String> {
    ensure_idle(project)?;
    let stash = stashes(project)?.into_iter().find(|s| s.id == id).ok_or("That stash doesn't exist anymore.")?;
    let files = stash_files(project, id)?;
    let touched: Vec<&str> = files.iter().flat_map(|f| std::iter::once(f.path.as_str()).chain(f.orig_path.as_deref())).collect();

    let changed: HashSet<String> = changes(project)?.into_iter().flat_map(|c| std::iter::once(c.path).chain(c.orig_path)).collect();
    let overlap: Vec<&str> = touched.iter().copied().filter(|p| changed.contains(*p)).collect();
    if !overlap.is_empty() {
        return Err(format!(
            "You changed these files again since you stashed them. Commit, stash or discard that first, so neither version gets overwritten:\n\n{}",
            overlap.join("\n")
        ));
    }

    // A stash only goes back cleanly onto the same versions of its files. Otherwise Git would
    // leave conflict markers, so refuse and point to the branch it came from. One diff answers
    // this for every file at once.
    let base = format!("{id}^1");
    let since = read_ok(project, &["-c", "core.quotePath=false", "diff", "--name-only", "-z", &base, "HEAD", "--"], "Couldn't compare the stash with your files.")?;
    let since: HashSet<&str> = since.split('\0').filter(|s| !s.is_empty()).collect();
    let mut moved: Vec<&str> = vec![];
    for f in &files {
        // Files that existed when stashing must be unchanged; new ones must not exist yet.
        let existed = matches!(f.kind.as_str(), "M" | "D");
        if (existed && since.contains(f.path.as_str())) || (!existed && (since.contains(f.path.as_str()) || project.root.join(&f.path).exists())) {
            moved.push(&f.path);
        }
        if let Some(orig) = f.orig_path.as_deref()
            && since.contains(orig)
        {
            moved.push(orig);
        }
    }
    if !moved.is_empty() {
        let here = current_branch(project)?;
        let fix = if !stash.branch.is_empty() && stash.branch != here { format!("Switch to {} and bring it back there.", stash.branch) } else { "Commit or discard what changed them first.".to_string() };
        return Err(format!("These files are different here than when you stashed them, so bringing the stash back would mix two versions:\n\n{}\n\n{fix}", moved.join("\n")));
    }
    ensure_editor_closed(project, "bring back a stash")?;
    let r = stash_ref(project, id)?;
    run_ok(log, project, &["stash", "pop", &r], "Couldn't bring the stash back. It's still in the list, so nothing is lost.")?;
    Ok(())
}

pub fn stash_drop(log: &Log, project: &Project, id: &str) -> Result<(), String> {
    ensure_idle(project)?;
    let r = stash_ref(project, id)?;
    run_ok(log, project, &["stash", "drop", &r], "Couldn't delete the stash.")?;
    Ok(())
}

// ---------- History ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub commits: Vec<Commit>,
    /// Hashes of the commits that aren't on the server yet.
    pub unpushed: Vec<String>,
}

pub fn history(project: &Project, limit: usize) -> Result<History, String> {
    if !has_head(project) {
        return Ok(History { commits: vec![], unpushed: vec![] });
    }
    let n = format!("-n{}", limit.clamp(1, 1000));
    let commits = Commit::parse_log(&read_ok(project, &["log", &n, Commit::FORMAT, "HEAD", "--"], "Couldn't read the history.")?);
    // Mid-rebase there's no branch, so nothing counts as unpushed.
    let unpushed = match current_branch(project) {
        Ok(branch) => {
            let mut args = vec!["log".to_string(), n, "--format=%h".into()];
            args.extend(unpushed_revs(project, &branch));
            args.push("--".into());
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            read_ok(project, &args, "Couldn't read the history.")?.lines().map(str::to_string).collect()
        }
        Err(_) => vec![],
    };
    Ok(History { commits, unpushed })
}

/// The last few commits that changed `path`, newest first. Renames aren't followed: on a big
/// repo that would walk the whole history on every click.
pub fn file_history(project: &Project, path: &str) -> Result<Vec<Commit>, String> {
    check_paths(&[path.to_string()])?;
    if !has_head(project) {
        return Ok(vec![]);
    }
    let out = read_ok(project, &["log", "-n8", Commit::FORMAT, "HEAD", "--", path], "Couldn't read the file's history.")?;
    Ok(Commit::parse_log(&out))
}

fn check_hash(hash: &str) -> Result<(), String> {
    if hash.len() < 4 || hash.len() > 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("That isn't a commit.".into());
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub body: String,
    pub files: Vec<ChangedPath>,
}

/// Merge commits (like pull requests merged into develop) show what they brought in: their
/// changes against the first parent.
pub fn commit_details(project: &Project, hash: &str) -> Result<CommitDetails, String> {
    check_hash(hash)?;
    let body = read_ok(project, &["log", "-1", "--format=%b", hash, "--"], "Couldn't read the commit.")?;
    let files = read_ok(project, &["-c", "core.quotePath=false", "show", "--name-status", "-z", "-M", "-m", "--first-parent", "--format=", hash], "Couldn't read the commit.")?;
    Ok(CommitDetails { body: body.trim().into(), files: parse_name_status(&files) })
}

pub fn commit_diff(project: &Project, hash: &str, path: &str) -> Result<Diff, String> {
    check_hash(hash)?;
    check_paths(&[path.to_string()])?;
    if is_binary_path(project, path) {
        return Ok(Diff { binary: true, ..Default::default() });
    }
    let out = read_raw(project, &["-c", "core.quotePath=false", "show", "--format=", "--no-color", "--no-ext-diff", "-M", "-m", "--first-parent", hash, "--", path], "Couldn't compare the file.")?;
    Ok(finish_diff(out))
}

/// Deletes `.git/index.lock` that a crashed Git program left behind. Refused while any Git
/// runs, or while the lock is fresh, since then it's real.
pub fn remove_index_lock(project: &Project) -> Result<(), String> {
    let lock = git_dir(project)?.join("index.lock");
    let Ok(meta) = std::fs::metadata(&lock) else { return Ok(()) };
    let busy = "Git is still working in another program, so the lock file is in use. Close Rider, Fork, GitHub Desktop and Unreal's Git plugin, wait a moment, then try again.";
    if meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(30)) {
        return Err(busy.into());
    }
    if proc::is_running("git.exe") {
        return Err(busy.into());
    }
    std::fs::remove_file(&lock).map_err(|e| format!("Couldn't remove {}: {e}", lock.display()))
}

// ---------- Locks (read only) ----------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lock {
    pub path: String,
    pub owner: String,
    pub mine: bool,
}

#[derive(Deserialize)]
struct RawLock {
    path: String,
    owner: Option<RawOwner>,
}

#[derive(Deserialize)]
struct RawOwner {
    name: String,
}

#[derive(Deserialize)]
struct Verified {
    #[serde(default)]
    ours: Vec<RawLock>,
    #[serde(default)]
    theirs: Vec<RawLock>,
}

fn to_lock(raw: RawLock, mine: bool) -> Lock {
    Lock { path: raw.path, owner: raw.owner.map(|o| o.name).unwrap_or_default(), mine }
}

pub fn parse_verified_locks(json: &str) -> Result<Vec<Lock>, String> {
    let v: Verified = serde_json::from_str(json).map_err(|e| format!("Couldn't read the locks: {e}"))?;
    Ok(v.ours.into_iter().map(|l| to_lock(l, true)).chain(v.theirs.into_iter().map(|l| to_lock(l, false))).collect())
}

fn locks_within(project: &Project, secs: u64) -> Result<Vec<Lock>, String> {
    let out = proc::run_timeout("git", &["lfs", "locks", "--verify", "--json"], Some(&project.root), secs)?;
    if !out.ok {
        return Err(format!("Couldn't read the file locks.\n\n{}", out.message()));
    }
    parse_verified_locks(&out.stdout)
}

/// Who has which files locked on the server. Shown next to changed files; locking itself
/// happens in Unreal.
pub fn locks(project: &Project) -> Result<Vec<Lock>, String> {
    locks_within(project, 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = proc::run("git", args, Some(dir)).unwrap();
        assert!(out.ok, "git {args:?} failed: {}", out.message());
        out.stdout
    }

    fn write(dir: &Path, file: &str, content: &[u8]) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn configure(dir: &Path) {
        sh(dir, &["config", "user.name", "Test"]);
        sh(dir, &["config", "user.email", "test@example.com"]);
        sh(dir, &["config", "core.autocrlf", "false"]);
    }

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ue-dev-hub-gitui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn project_files(dir: &Path) {
        write(dir, "Game.uproject", br#"{"EngineAssociation":"5.8"}"#);
        write(dir, "ue-dev-hub.toml", b"[project]\nbase_branch = \"develop\"\n\n[git]\nupdate = \"rebase\"\n");
    }

    /// An origin with `develop` and `feat/x`, and a clone on `feat/x`. Returns the clone.
    fn scenario(name: &str) -> PathBuf {
        scenario_in(name, "")
    }

    /// Like `scenario`, with the project (.uproject and ue-dev-hub.toml) in the folder `sub` of
    /// the repository.
    fn scenario_in(name: &str, sub: &str) -> PathBuf {
        let root = temp(name);
        let origin = root.join("origin.git");
        sh(&root, &["init", "-q", "--bare", "-b", "develop", origin.to_str().unwrap()]);
        let seed = root.join("seed");
        sh(&root, &["clone", "-q", "-c", "core.autocrlf=false", origin.to_str().unwrap(), "seed"]);
        configure(&seed);
        project_files(&seed.join(sub));
        write(&seed, "Content/Map.umap", &[0, 1, 2, 0xff]);
        write(&seed, "Notes.txt", b"one\ntwo\n");
        sh(&seed, &["add", "-A"]);
        sh(&seed, &["commit", "-qm", "Initial"]);
        sh(&seed, &["push", "-q", "origin", "HEAD:develop"]);
        sh(&seed, &["push", "-q", "origin", "HEAD:feat/x"]);
        let work = root.join("work");
        sh(&root, &["clone", "-q", "-c", "core.autocrlf=false", "-b", "feat/x", origin.to_str().unwrap(), "work"]);
        configure(&work);
        work
    }

    fn project(dir: &Path) -> Project {
        Project::load(dir).unwrap()
    }

    /// Commits what's ticked, sending the ticked files the way the page does.
    fn commit_ticked(log: &Log, p: &Project, summary: &str, body: &str) -> Result<String, String> {
        commit(log, p, summary, body, &ticked(p))
    }

    fn ticked(p: &Project) -> Vec<String> {
        changes(p).unwrap().into_iter().filter(|f| f.staged && f.kind != "C").flat_map(|f| { let orig = f.orig_path.filter(|_| f.kind == "R"); std::iter::once(f.path).chain(orig) }).collect()
    }

    /// Another clone of the scenario's origin, on `branch`.
    fn other_clone(work: &Path, name: &str, branch: &str) -> PathBuf {
        let root = work.parent().unwrap();
        sh(root, &["clone", "-q", "-c", "core.autocrlf=false", "-b", branch, root.join("origin.git").to_str().unwrap(), name]);
        let dir = root.join(name);
        configure(&dir);
        dir
    }

    fn commit_file(log: &Log, p: &Project, file: &str, content: &[u8]) -> String {
        write(&p.root, file, content);
        stage(log, p, &[file.into()]).unwrap();
        commit_ticked(log, p, &format!("Change {file}"), "").unwrap()
    }

    /// Keeps discarded files in a folder next to the repo instead of the trash.
    fn keep_in(dir: PathBuf) -> impl Fn(&Path) -> Result<(), String> {
        move |p: &Path| {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::rename(p, dir.join(p.file_name().unwrap())).map_err(|e| e.to_string())
        }
    }

    fn staged_paths(p: &Project) -> Vec<String> {
        changes(p).unwrap().into_iter().filter(|f| f.staged).map(|f| f.path).collect()
    }

    /// A teammate's new commit on develop.
    fn teammate_commit(work: &Path, file: &str, content: &[u8]) {
        let seed = work.parent().unwrap().join("seed");
        sh(&seed, &["checkout", "-q", "develop"]);
        sh(&seed, &["pull", "-q", "origin", "develop"]);
        write(&seed, file, content);
        sh(&seed, &["add", "-A"]);
        sh(&seed, &["commit", "-qm", "Teammate change"]);
        sh(&seed, &["push", "-q", "origin", "develop"]);
    }

    #[test]
    fn parses_porcelain_v2() {
        let out = "1 .M N... 100644 100644 100644 aaa aaa Source/A.cpp\0\
                   1 A. N... 000000 100644 100644 000 bbb New Folder/B.uasset\0\
                   2 R. N... 100644 100644 100644 ccc ccc R100 Content/New Map.umap\0Content/Old Map.umap\0\
                   u UU N... 100644 100644 100644 100644 d1 d2 d3 Con flict.txt\0\
                   1 .M S.M. 160000 160000 160000 eee eee Tools\0\
                   ? Untracked file.txt\0";
        let files = parse_status(out);
        assert_eq!(files.len(), 6);
        assert_eq!((files[0].kind.as_str(), files[0].staged, files[0].unstaged), ("M", false, true));
        assert_eq!((files[1].path.as_str(), files[1].kind.as_str(), files[1].staged, files[1].binary), ("New Folder/B.uasset", "A", true, true));
        assert_eq!((files[2].path.as_str(), files[2].orig_path.as_deref()), ("Content/New Map.umap", Some("Content/Old Map.umap")));
        assert_eq!(files[2].kind, "R");
        assert_eq!((files[3].path.as_str(), files[3].kind.as_str(), files[3].staged), ("Con flict.txt", "C", false));
        assert!(files[4].submodule && !files[0].submodule);
        assert_eq!((files[5].path.as_str(), files[5].kind.as_str(), files[5].staged), ("Untracked file.txt", "U", false));
    }

    #[test]
    fn allows_every_branch_name_git_does() {
        let work = scenario("names");
        let p = project(&work);
        for good in ["fix/save-menu-crash", "my-work", "Feature/X", "feat/x+y", "fix/größe", "a/b/c"] {
            assert_eq!(branch_name_problem(&p, good), None, "{good}");
        }
        for bad in ["", "my work", "a..b", "@{-1}", "-x", "x.lock", "HEAD", "@", "feat/", "a:b", "x~1"] {
            assert!(branch_name_problem(&p, bad).is_some(), "{bad}");
        }
        assert!(is_base("Develop", "develop") && is_base("develop", "develop") && !is_base("feat/develop", "develop"));
    }

    #[test]
    fn switches_to_and_pushes_any_branch_git_allows() {
        let work = scenario("oddnames");
        let p = project(&work);
        let log = Log::default();
        create_branch(&log, &p, "feat/x+y").unwrap();
        commit_file(&log, &p, "Mine.txt", b"mine\n");
        assert!(push(&log, &p).unwrap().published);
        switch_branch(&log, &p, "feat/x").unwrap();
        assert!(overview(&p).unwrap().branches.contains(&"feat/x+y".to_string()));
        switch_branch(&log, &p, "feat/x+y").unwrap();
        assert_eq!(current_branch(&p).unwrap(), "feat/x+y");
    }

    #[test]
    fn colons_are_fine_in_linux_file_names() {
        assert_eq!(config::is_inside("Notes: today.txt"), !cfg!(windows));
        // On Linux, "C:" is just a folder name.
        assert_eq!(config::is_inside("C:/Windows/notepad.exe"), !cfg!(windows));
        assert!(!config::is_inside("../x") && !config::is_inside("/etc/passwd"));
        assert!(check_paths(&["Bad \u{FFFD} name.txt".into()]).unwrap_err().contains("can't read"));
    }

    #[test]
    fn parses_stash_subjects_name_status_and_locks() {
        assert_eq!(parse_stash_subject("On feat/x: Try red"), ("feat/x".into(), "Try red".into()));
        assert_eq!(parse_stash_subject("WIP on feat/x: 1a2b3c Last commit"), ("feat/x".into(), "Changes on feat/x: Last commit".into()));
        let files = parse_name_status("M\0a.txt\0R100\0old.txt\0new.txt\0C75\0src.txt\0copy.txt\0");
        assert_eq!(files[1].orig_path.as_deref(), Some("old.txt"));
        assert_eq!((files[2].kind.as_str(), files[2].orig_path.as_deref()), ("A", None));
        let json = r#"{"ours":[{"id":"1","path":"Content/A.uasset","owner":{"name":"jonas"},"locked_at":"2026-01-01T00:00:00Z"}],
                       "theirs":[{"id":"2","path":"Content/B.umap","owner":{"name":"priya"},"locked_at":"2026-01-01T00:00:00Z"}]}"#;
        let locks = parse_verified_locks(json).unwrap();
        assert_eq!(locks, vec![
            Lock { path: "Content/A.uasset".into(), owner: "jonas".into(), mine: true },
            Lock { path: "Content/B.umap".into(), owner: "priya".into(), mine: false },
        ]);
    }

    #[test]
    fn stages_diffs_and_discards() {
        let work = scenario("stage");
        let p = project(&work);
        let log = Log::default();
        write(&work, "Notes.txt", b"one\nTWO\n");
        write(&work, "Content/Map.umap", &[9, 9, 9]);
        write(&work, "New File.txt", b"hello\n");

        assert_eq!(changes(&p).unwrap().len(), 3);
        let d = diff(&p, "Notes.txt").unwrap();
        assert!(d.text.contains("-two") && d.text.contains("+TWO"), "{}", d.text);
        assert!(diff(&p, "Content/Map.umap").unwrap().binary);
        assert!(diff(&p, "New File.txt").unwrap().text.contains("+hello"));
        assert!(diff(&p, "../outside.txt").is_err());

        stage(&log, &p, &["Notes.txt".into(), "New File.txt".into()]).unwrap();
        assert_eq!(staged_paths(&p), vec!["New File.txt".to_string(), "Notes.txt".to_string()]);
        unstage(&log, &p, &["Notes.txt".into()]).unwrap();
        assert_eq!(staged_paths(&p), vec!["New File.txt".to_string()]);

        let kept = work.parent().unwrap().join("kept");
        discard_with(&log, &p, "Notes.txt", keep_in(kept.clone())).unwrap();
        discard_with(&log, &p, "New File.txt", keep_in(kept.clone())).unwrap();
        discard_with(&log, &p, "Content/Map.umap", keep_in(kept.clone())).unwrap();
        assert!(changes(&p).unwrap().is_empty(), "{:?}", changes(&p).unwrap());
        assert_eq!(std::fs::read_to_string(work.join("Notes.txt")).unwrap(), "one\ntwo\n");
        assert_eq!(std::fs::read(work.join("Content/Map.umap")).unwrap(), vec![0, 1, 2, 0xff]);
        assert!(!work.join("New File.txt").exists());
        // Our versions were kept, not lost, and nothing is left in the git folder.
        assert_eq!(std::fs::read_to_string(kept.join("Notes.txt")).unwrap(), "one\nTWO\n");
        assert_eq!(std::fs::read_to_string(kept.join("New File.txt")).unwrap(), "hello\n");
        assert_eq!(std::fs::read(kept.join("Map.umap")).unwrap(), vec![9, 9, 9]);
        let aside = work.join(".git/ue-dev-hub/discarding");
        assert!(!aside.exists() || std::fs::read_dir(&aside).unwrap().next().is_none());
    }

    #[test]
    fn file_names_are_never_patterns() {
        let work = scenario("literal");
        let p = project(&work);
        let log = Log::default();
        write(&work, "Plan[1].txt", b"a\n");
        write(&work, "Plan1.txt", b"b\n");
        sh(&work, &["add", "-A"]);
        sh(&work, &["commit", "-qm", "Plans"]);
        write(&work, "Plan[1].txt", b"edited\n");
        write(&work, "Plan1.txt", b"precious\n");

        stage(&log, &p, &["Plan[1].txt".into()]).unwrap();
        assert_eq!(staged_paths(&p), vec!["Plan[1].txt".to_string()]);
        unstage(&log, &p, &["Plan[1].txt".into()]).unwrap();
        stash_push(&log, &p, "One plan", Some(vec!["Plan[1].txt".into()])).unwrap();
        assert_eq!(changes(&p).unwrap().iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["Plan1.txt"]);
        let id = stashes(&p).unwrap()[0].id.clone();
        assert_eq!(stash_files(&p, &id).unwrap().len(), 1);
        stash_pop(&log, &p, &id).unwrap();
        discard_with(&log, &p, "Plan[1].txt", keep_in(work.parent().unwrap().join("kept-literal"))).unwrap();
        assert_eq!(std::fs::read_to_string(work.join("Plan1.txt")).unwrap(), "precious\n");
        assert_eq!(std::fs::read_to_string(work.join("Plan[1].txt")).unwrap(), "a\n");
    }

    #[test]
    fn renames_are_ticked_and_discarded_as_a_whole() {
        let work = scenario("rename");
        let p = project(&work);
        let log = Log::default();
        sh(&work, &["mv", "Content/Map.umap", "Content/Renamed.umap"]);
        let r = changes(&p).unwrap();
        assert_eq!((r[0].kind.as_str(), r[0].orig_path.as_deref()), ("R", Some("Content/Map.umap")));
        // Unticking the rename must not leave the old file's deletion ticked.
        unstage(&log, &p, &["Content/Renamed.umap".into()]).unwrap();
        assert!(staged_paths(&p).is_empty(), "{:?}", changes(&p).unwrap());
        // Ticking both sides again makes it a rename again.
        stage(&log, &p, &["Content/Renamed.umap".into(), "Content/Map.umap".into()]).unwrap();
        assert_eq!(changes(&p).unwrap()[0].kind, "R");

        discard_with(&log, &p, "Content/Renamed.umap", keep_in(work.parent().unwrap().join("kept-rename"))).unwrap();
        std::fs::remove_file(work.join("Notes.txt")).unwrap();
        discard_with(&log, &p, "Notes.txt", keep_in(work.parent().unwrap().join("kept-rename"))).unwrap();
        assert!(changes(&p).unwrap().is_empty(), "{:?}", changes(&p).unwrap());
        assert!(work.join("Notes.txt").is_file());
        assert!(work.join("Content/Map.umap").is_file());
        assert!(!work.join("Content/Renamed.umap").exists());
    }

    #[test]
    fn shows_a_staged_deletion() {
        let work = scenario("deletion");
        let p = project(&work);
        sh(&work, &["rm", "-q", "Notes.txt"]);
        let d = diff(&p, "Notes.txt").unwrap();
        assert!(d.text.contains("-one") && d.text.contains("-two"), "{}", d.text);
    }

    #[test]
    fn refuses_to_discard_a_submodule() {
        let work = scenario("submodule");
        let root = work.parent().unwrap().to_path_buf();
        sh(&root, &["init", "-q", "-b", "main", "subsrc"]);
        let sub = root.join("subsrc");
        configure(&sub);
        write(&sub, "tool.txt", b"v1\n");
        sh(&sub, &["add", "-A"]);
        sh(&sub, &["commit", "-qm", "Tool"]);
        sh(&work, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", sub.to_str().unwrap(), "Tools"]);
        sh(&work, &["commit", "-qm", "Add Tools"]);
        write(&work.join("Tools"), "tool.txt", b"edited\n");
        let p = project(&work);
        assert!(changes(&p).unwrap().iter().any(|f| f.path == "Tools" && f.submodule));
        assert!(discard_with(&Log::default(), &p, "Tools", keep_in(root.join("kept-sub"))).unwrap_err().contains("submodule"));
        assert_eq!(std::fs::read_to_string(work.join("Tools/tool.txt")).unwrap(), "edited\n");
    }

    #[test]
    fn works_before_the_first_commit() {
        let root = temp("unborn");
        sh(&root, &["init", "-q", "-b", "feat/start", "work"]);
        let work = root.join("work");
        configure(&work);
        project_files(&work);
        let p = project(&work);
        let log = Log::default();
        let o = overview(&p).unwrap();
        assert_eq!((o.branch.as_str(), o.ahead, o.files.len()), ("feat/start", 0, 2));
        assert!(history(&p, 10).unwrap().commits.is_empty());
        stage(&log, &p, &["Game.uproject".into()]).unwrap();
        assert!(diff(&p, "Game.uproject").unwrap().text.contains("EngineAssociation"));
        unstage(&log, &p, &["Game.uproject".into()]).unwrap();
        assert!(staged_paths(&p).is_empty());
        stage(&log, &p, &["Game.uproject".into(), "ue-dev-hub.toml".into()]).unwrap();
        commit_ticked(&log, &p, "Start", "").unwrap();
        assert_eq!(history(&p, 10).unwrap().commits.len(), 1);
    }

    #[test]
    fn commits_only_on_feature_branches() {
        let work = scenario("commit");
        let p = project(&work);
        let log = Log::default();
        assert!(commit_ticked(&log, &p, "Nothing", "").unwrap_err().contains("Tick"));
        write(&work, "Notes.txt", b"changed\n");
        stage(&log, &p, &["Notes.txt".into()]).unwrap();
        assert!(commit_ticked(&log, &p, "  ", "").unwrap_err().contains("summary"));
        let hash = commit_ticked(&log, &p, "Change notes", "Because.").unwrap();
        assert_eq!(sh(&work, &["log", "-1", "--format=%h %s"]), format!("{hash} Change notes"));
        assert_eq!(sh(&work, &["log", "-1", "--format=%b"]), "Because.");
        let o = overview(&p).unwrap();
        assert_eq!(o.ahead, 1);
        assert!(o.on_server);
        assert_eq!(history(&p, 10).unwrap().unpushed, vec![hash]);

        sh(&work, &["switch", "-q", "develop"]);
        write(&work, "Notes.txt", b"on develop\n");
        stage(&log, &p, &["Notes.txt".into()]).unwrap();
        let before = sh(&work, &["rev-parse", "HEAD"]);
        assert!(commit_ticked(&log, &p, "Sneaky", "").unwrap_err().contains("can't commit to develop"));
        assert!(push(&log, &p).unwrap_err().contains("can't push to develop"));
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn nothing_changes_while_git_is_locked() {
        let work = scenario("locked");
        let p = project(&work);
        let log = Log::default();
        write(&work, "Notes.txt", b"changed\n");
        std::fs::write(work.join(".git/index.lock"), b"").unwrap();
        assert!(overview(&p).unwrap().repo_busy.is_some());
        assert!(stage(&log, &p, &["Notes.txt".into()]).unwrap_err().contains("index.lock"));
        assert!(commit_ticked(&log, &p, "x", "").unwrap_err().contains("index.lock"));
        assert!(push(&log, &p).unwrap_err().contains("index.lock"));
        assert!(stash_push(&log, &p, "x", None).unwrap_err().contains("index.lock"));
        assert!(create_branch(&log, &p, "feat/y").unwrap_err().contains("index.lock"));
        assert!(discard_with(&log, &p, "Notes.txt", keep_in(work.parent().unwrap().join("k"))).unwrap_err().contains("index.lock"));
        // A fresh lock is never removed: it may be in use.
        assert!(remove_index_lock(&p).is_err());
        assert!(work.join(".git/index.lock").exists());
        std::fs::remove_file(work.join(".git/index.lock")).unwrap();
    }

    #[test]
    fn pushes_without_overwriting_others() {
        let work = scenario("push");
        let p = project(&work);
        let log = Log::default();
        write(&work, "Mine.txt", b"mine\n");
        stage(&log, &p, &["Mine.txt".into()]).unwrap();
        commit_ticked(&log, &p, "Mine", "").unwrap();
        let r = push(&log, &p).unwrap();
        assert_eq!((r.commits, r.published), (1, false));
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), sh(&work, &["rev-parse", "origin/feat/x"]));

        // A teammate pushes to the same branch; our next push must not overwrite it.
        let other = other_clone(&work, "other", "feat/x");
        write(&other, "Theirs.txt", b"theirs\n");
        sh(&other, &["add", "-A"]);
        sh(&other, &["commit", "-qm", "Theirs"]);
        sh(&other, &["push", "-q"]);
        write(&work, "Mine2.txt", b"more\n");
        stage(&log, &p, &["Mine2.txt".into()]).unwrap();
        commit_ticked(&log, &p, "Mine again", "").unwrap();
        assert!(push(&log, &p).unwrap_err().contains("overwrite"));
        sh(&work, &["fetch", "-q"]);
        assert_eq!(sh(&work, &["log", "-1", "--format=%s", "origin/feat/x"]), "Theirs");
    }

    #[test]
    fn updates_then_pushes_the_rebased_branch() {
        let work = scenario("rebasepush");
        let p = project(&work);
        let log = Log::default();
        write(&work, "Mine.txt", b"mine\n");
        stage(&log, &p, &["Mine.txt".into()]).unwrap();
        commit_ticked(&log, &p, "Mine", "").unwrap();
        push(&log, &p).unwrap();
        teammate_commit(&work, "Theirs.txt", b"theirs\n");
        teammate_commit(&work, "Theirs2.txt", b"theirs\n");

        git::check_updates(&log, &p).unwrap();
        assert_eq!(overview(&p).unwrap().incoming, 2);
        pull(&log, &p, false).unwrap();
        // Only our own rebased commit counts as unpushed, not the team's two.
        let o = overview(&p).unwrap();
        assert_eq!((o.ahead, o.incoming), (1, 0));
        assert_eq!(history(&p, 10).unwrap().unpushed.len(), 1);
        // The server still has the pre-rebase commit; it's ours, so the push replaces it.
        let r = push(&log, &p).unwrap();
        assert_eq!(r.commits, 1);
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), sh(&work, &["rev-parse", "origin/feat/x"]));
        assert!(work.join("Theirs2.txt").is_file());
    }

    #[test]
    fn creates_publishes_and_switches_branches() {
        let work = scenario("branch");
        let p = project(&work);
        let log = Log::default();
        assert!(create_branch(&log, &p, "bad name").unwrap_err().contains("No spaces"));
        assert!(create_branch(&log, &p, "feat/x").unwrap_err().contains("already exists"));
        // Uncommitted changes come along to a new branch.
        write(&work, "Notes.txt", b"wip\n");
        create_branch(&log, &p, "feat/new-thing").unwrap();
        assert_eq!(current_branch(&p).unwrap(), "feat/new-thing");
        assert_eq!(std::fs::read_to_string(work.join("Notes.txt")).unwrap(), "wip\n");
        // But switching to an existing branch needs a clean working tree.
        assert!(switch_branch(&log, &p, "feat/x").unwrap_err().contains("uncommitted"));
        stage(&log, &p, &["Notes.txt".into()]).unwrap();
        commit_ticked(&log, &p, "WIP", "").unwrap();
        assert!(!overview(&p).unwrap().on_server);
        assert!(push(&log, &p).unwrap().published);
        assert!(overview(&p).unwrap().on_server);
        switch_branch(&log, &p, "feat/x").unwrap();
        assert_eq!(current_branch(&p).unwrap(), "feat/x");
        assert!(switch_branch(&log, &p, "--orphan").is_err());
    }

    #[test]
    fn a_failing_hook_doesnt_hide_a_switch_that_worked() {
        let work = scenario("hook");
        let p = project(&work);
        write(&work, ".git/hooks/post-checkout", b"#!/bin/sh\necho hook failed >&2\nexit 1\n");
        // Git skips hooks that aren't executable, which only matters outside Windows.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = work.join(".git/hooks/post-checkout");
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let warnings = create_branch(&Log::default(), &p, "feat/hooked").unwrap();
        assert_eq!(current_branch(&p).unwrap(), "feat/hooked");
        assert!(warnings.iter().any(|w| w.contains("hook")), "{warnings:?}");
    }

    #[test]
    fn stashes_and_brings_back_changes() {
        let work = scenario("stash");
        let p = project(&work);
        let log = Log::default();
        assert!(stash_push(&log, &p, "x", None).unwrap_err().contains("no changes"));
        write(&work, "Notes.txt", b"stashed\n");
        write(&work, "New.txt", b"new\n");
        write(&work, "Content/Map.umap", &[5, 5]);
        stash_push(&log, &p, "Only notes", Some(vec!["Notes.txt".into()])).unwrap();
        stash_push(&log, &p, "", None).unwrap();
        assert!(changes(&p).unwrap().is_empty(), "{:?}", changes(&p).unwrap());

        let list = stashes(&p).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Changes on feat/x");
        assert_eq!(list[1].name, "Only notes");
        let mut paths: Vec<_> = list[0].files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["Content/Map.umap", "New.txt"]);

        // Bringing back is refused while the same file changed again.
        write(&work, "Notes.txt", b"changed again\n");
        assert!(stash_pop(&log, &p, &list[1].id).unwrap_err().contains("Notes.txt"));
        discard_with(&log, &p, "Notes.txt", keep_in(work.parent().unwrap().join("kept3"))).unwrap();
        // A new file from the stash that exists again by now is refused too.
        write(&work, "New.txt", b"made again\n");
        assert!(stash_pop(&log, &p, &list[0].id).unwrap_err().contains("New.txt"));
        std::fs::remove_file(work.join("New.txt")).unwrap();

        // Ids stay valid when other stashes go away.
        stash_pop(&log, &p, &list[0].id).unwrap();
        assert!(work.join("New.txt").is_file());
        stash_pop(&log, &p, &list[1].id).unwrap();
        assert_eq!(std::fs::read_to_string(work.join("Notes.txt")).unwrap(), "stashed\n");
        assert!(stashes(&p).unwrap().is_empty());

        // A stash from another version of the file is refused instead of leaving conflicts.
        sh(&work, &["add", "-A"]);
        sh(&work, &["commit", "-qm", "Keep it"]);
        write(&work, "Notes.txt", b"stashed on feat/x\n");
        stash_push(&log, &p, "Old notes", None).unwrap();
        write(&work, "Notes.txt", b"committed since\n");
        stage(&log, &p, &["Notes.txt".into()]).unwrap();
        commit_ticked(&log, &p, "Change notes", "").unwrap();
        let old = stashes(&p).unwrap()[0].id.clone();
        assert!(stash_pop(&log, &p, &old).unwrap_err().contains("different here"));
        assert!(changes(&p).unwrap().is_empty(), "{:?}", changes(&p).unwrap());
        stash_drop(&log, &p, &old).unwrap();
        assert!(stash_drop(&log, &p, &old).is_err());
        assert!(stashes(&p).unwrap().is_empty());
    }

    #[test]
    fn a_stashed_rename_checks_its_old_name_too() {
        let work = scenario("stashrename");
        let p = project(&work);
        let log = Log::default();
        sh(&work, &["mv", "Notes.txt", "Renamed.txt"]);
        stash_push(&log, &p, "Rename", None).unwrap();
        let id = stashes(&p).unwrap()[0].id.clone();
        assert!(stash_files(&p, &id).unwrap().iter().any(|f| f.orig_path.as_deref() == Some("Notes.txt")));
        write(&work, "Notes.txt", b"changed meanwhile\n");
        stage(&log, &p, &["Notes.txt".into()]).unwrap();
        commit_ticked(&log, &p, "Change notes", "").unwrap();
        assert!(stash_pop(&log, &p, &id).unwrap_err().contains("Notes.txt"));
        assert!(changes(&p).unwrap().is_empty(), "{:?}", changes(&p).unwrap());
    }

    #[test]
    fn shows_commit_details_also_for_merges() {
        let work = scenario("history");
        let p = project(&work);
        let h = history(&p, 10).unwrap();
        assert_eq!(h.commits.len(), 1);
        assert!(h.unpushed.is_empty());
        let d = commit_details(&p, &h.commits[0].hash).unwrap();
        let mut paths: Vec<_> = d.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["Content/Map.umap", "Game.uproject", "Notes.txt", "ue-dev-hub.toml"]);
        assert!(commit_diff(&p, &h.commits[0].hash, "Notes.txt").unwrap().text.contains("+two"));
        assert!(commit_diff(&p, &h.commits[0].hash, "Content/Map.umap").unwrap().binary);
        assert!(commit_details(&p, "HEAD; rm").is_err());
        assert_eq!(file_history(&p, "Notes.txt").unwrap().len(), 1);

        // A merge commit, like a pull request merged on the server, lists what it brought in.
        sh(&work, &["switch", "-q", "-c", "feat/side"]);
        write(&work, "Side.txt", b"side\n");
        sh(&work, &["add", "-A"]);
        sh(&work, &["commit", "-qm", "Side"]);
        sh(&work, &["switch", "-q", "feat/x"]);
        write(&work, "Main.txt", b"main\n");
        sh(&work, &["add", "-A"]);
        sh(&work, &["commit", "-qm", "Main"]);
        sh(&work, &["merge", "-q", "--no-ff", "--no-edit", "feat/side"]);
        let merge = sh(&work, &["rev-parse", "--short", "HEAD"]);
        let d = commit_details(&p, &merge).unwrap();
        assert_eq!(d.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["Side.txt"]);
        assert!(commit_diff(&p, &merge, "Side.txt").unwrap().text.contains("+side"));
    }

    #[test]
    fn pull_refuses_to_merge() {
        let work = scenario("pullmerge");
        write(&work, "ue-dev-hub.local.toml", b"[git]\nupdate = \"merge\"\n");
        let p = project(&work);
        assert!(pull(&Log::default(), &p, false).unwrap_err().contains("only rebases"));
    }

    #[test]
    fn works_when_the_project_is_deeper_in_the_repo() {
        let work = scenario_in("nested", "Game");
        let p = project(&work.join("Game"));
        let log = Log::default();
        assert_eq!(std::fs::canonicalize(&p.root).unwrap(), std::fs::canonicalize(&work).unwrap());
        write(&work, "Notes.txt", b"one\nTWO\n");
        write(&work, "Game/New.txt", b"new\n");
        write(&work, "Game/Gone.txt", b"gone\n");
        let o = overview(&p).unwrap();
        assert_eq!(PathBuf::from(&o.root), p.root);
        let mut paths: Vec<_> = o.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["Game/Gone.txt", "Game/New.txt", "Notes.txt"]);
        assert!(diff(&p, "Notes.txt").unwrap().text.contains("+TWO"));
        assert!(diff(&p, "Game/New.txt").unwrap().text.contains("+new"));
        stage(&log, &p, &["Notes.txt".into(), "Game/New.txt".into()]).unwrap();
        assert_eq!(staged_paths(&p), vec!["Game/New.txt".to_string(), "Notes.txt".to_string()]);
        discard_with(&log, &p, "Game/Gone.txt", keep_in(work.parent().unwrap().join("kept-nested"))).unwrap();
        assert!(!work.join("Game/Gone.txt").exists());
        commit_ticked(&log, &p, "Nested", "").unwrap();
        assert!(changes(&p).unwrap().is_empty());
    }

    #[test]
    fn pushes_from_a_clone_that_fetches_only_some_branches() {
        let work = scenario("singlebranch");
        let root = work.parent().unwrap();
        let origin = root.join("origin.git");
        sh(root, &["clone", "-q", "--single-branch", "-c", "core.autocrlf=false", "-b", "develop", origin.to_str().unwrap(), "single"]);
        let single = root.join("single");
        configure(&single);
        let p = project(&single);
        let log = Log::default();
        create_branch(&log, &p, "feat/solo").unwrap();
        commit_file(&log, &p, "Solo.txt", b"1\n");
        assert!(push(&log, &p).unwrap().published);
        let o = overview(&p).unwrap();
        assert_eq!((o.on_server, o.ahead), (true, 0));

        // This clone's fetch settings don't cover feat/solo; the push must still see the server's copy.
        commit_file(&log, &p, "Solo.txt", b"2\n");
        let r = push(&log, &p).unwrap();
        assert_eq!((r.commits, r.published), (1, false));
        assert_eq!(sh(&single, &["rev-parse", "HEAD"]), sh(&origin, &["rev-parse", "refs/heads/feat/solo"]));

        // Deleted on the server: the next push puts it back instead of getting stuck.
        sh(&origin, &["update-ref", "-d", "refs/heads/feat/solo"]);
        commit_file(&log, &p, "Solo.txt", b"3\n");
        assert!(push(&log, &p).unwrap().published);
        assert_eq!(sh(&single, &["rev-parse", "HEAD"]), sh(&origin, &["rev-parse", "refs/heads/feat/solo"]));
        assert_eq!(overview(&p).unwrap().ahead, 0);
    }

    #[test]
    fn a_branch_with_a_similar_name_isnt_taken_for_yours() {
        let work = scenario("similar");
        let p = project(&work);
        let log = Log::default();
        let origin = work.parent().unwrap().join("origin.git");
        sh(&work, &["push", "-q", "origin", "HEAD:refs/heads/someone/feat/z"]);
        create_branch(&log, &p, "feat/z").unwrap();
        commit_file(&log, &p, "Z.txt", b"z\n");
        assert!(push(&log, &p).unwrap().published);
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), sh(&origin, &["rev-parse", "refs/heads/feat/z"]));

        // A server that can't be reached is an error, not "the branch isn't there".
        sh(&work, &["remote", "set-url", "origin", work.parent().unwrap().join("missing.git").to_str().unwrap()]);
        commit_file(&log, &p, "Z.txt", b"zz\n");
        let err = push(&log, &p).unwrap_err();
        assert!(err.contains("Couldn't reach the server"), "{err}");
    }

    #[test]
    fn explains_an_unfinished_rebase() {
        let work = scenario("midrebase");
        let p = project(&work);
        let log = Log::default();
        commit_file(&log, &p, "Notes.txt", b"mine\n");
        teammate_commit(&work, "Notes.txt", b"theirs\n");
        sh(&work, &["fetch", "-q", "origin"]);
        assert!(!proc::run("git", &["rebase", "origin/develop"], Some(&work)).unwrap().ok);
        let Err(err) = overview(&p) else { panic!("The overview should explain the rebase.") };
        assert!(err.contains("rebase") && !err.contains("detached"), "{err}");
        assert!(history(&p, 10).is_ok());
        sh(&work, &["rebase", "--abort"]);
        assert!(overview(&p).is_ok());
    }

    #[test]
    fn commits_only_what_the_page_showed_as_ticked() {
        let work = scenario("ticked");
        let p = project(&work);
        let log = Log::default();
        write(&work, "A.txt", b"a\n");
        write(&work, "B.txt", b"b\n");
        stage(&log, &p, &["A.txt".into()]).unwrap();
        let page = ticked(&p);
        // Another program ticks B behind the page's back.
        sh(&work, &["add", "B.txt"]);
        let before = sh(&work, &["rev-parse", "HEAD"]);
        assert!(commit(&log, &p, "Only A", "", &page).unwrap_err().contains("ticked files changed"));
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
        commit(&log, &p, "A and B", "", &["B.txt".into(), "A.txt".into()]).unwrap();

        // A rename is sent with both of its names.
        sh(&work, &["mv", "Notes.txt", "Renamed.txt"]);
        assert!(commit(&log, &p, "Rename", "", &["Renamed.txt".into()]).is_err());
        commit(&log, &p, "Rename", "", &["Renamed.txt".into(), "Notes.txt".into()]).unwrap();
        assert!(changes(&p).unwrap().is_empty());
    }

    #[test]
    fn a_file_locked_by_someone_else_blocks_the_commit() {
        let work = scenario("lockcommit");
        let p = project(&work);
        let log = Log::default();
        write(&work, "Content/Map.umap", &[7]);
        stage(&log, &p, &["Content/Map.umap".into()]).unwrap();
        let lock = |mine| move |_: &Project| Ok(vec![Lock { path: "Content/Map.umap".into(), owner: "sam".into(), mine }]);
        let before = sh(&work, &["rev-parse", "HEAD"]);
        let err = commit_with(&log, &p, "Map", "", &ticked(&p), lock(false)).unwrap_err();
        assert!(err.contains("locked by sam"), "{err}");
        assert_eq!(sh(&work, &["rev-parse", "HEAD"]), before);
        commit_with(&log, &p, "Map", "", &ticked(&p), lock(true)).unwrap();
        assert_ne!(sh(&work, &["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn a_file_called_head_doesnt_confuse_git() {
        let work = scenario("headfile");
        let p = project(&work);
        let log = Log::default();
        let hash = commit_file(&log, &p, "HEAD", b"not a revision\n");
        assert_eq!(overview(&p).unwrap().ahead, 1);
        let h = history(&p, 10).unwrap();
        assert_eq!((h.commits.len(), h.unpushed.clone()), (2, vec![hash.clone()]));
        assert_eq!(commit_details(&p, &hash).unwrap().files[0].path, "HEAD");
        assert_eq!(file_history(&p, "HEAD").unwrap().len(), 1);
        assert_eq!(push(&log, &p).unwrap().commits, 1);
        assert_eq!(overview(&p).unwrap().ahead, 0);
    }

    #[test]
    fn diffs_keep_their_last_blank_line() {
        let work = scenario("blankline");
        let p = project(&work);
        let log = Log::default();
        let hash = commit_file(&log, &p, "Blank.txt", b"x\n\n");
        write(&work, "Blank.txt", b"y\n\n");
        let d = diff(&p, "Blank.txt").unwrap();
        assert!(d.text.ends_with("+y\n \n"), "{:?}", d.text);
        let c = commit_diff(&p, &hash, "Blank.txt").unwrap();
        assert!(c.text.ends_with("+x\n+\n"), "{:?}", c.text);
    }

    #[test]
    fn a_failed_discard_changes_nothing() {
        let work = scenario("discardfail");
        let p = Project::load_ignoring_busy(&work).unwrap();
        let kept = work.parent().unwrap().join("kept-fail");
        write(&work, "Notes.txt", b"precious\n");
        // Git itself fails: another program holds its lock.
        std::fs::write(work.join(".git/index.lock"), b"").unwrap();
        let err = discard_with(&Log::default(), &p, "Notes.txt", keep_in(kept.clone())).unwrap_err();
        std::fs::remove_file(work.join(".git/index.lock")).unwrap();
        assert!(err.contains("was put back"), "{err}");
        assert_eq!(std::fs::read_to_string(work.join("Notes.txt")).unwrap(), "precious\n");
        assert!(!kept.exists());
    }

    #[test]
    fn a_discard_that_fails_halfway_is_rolled_back() {
        let work = scenario("discardhalf");
        let p = project(&work);
        std::fs::create_dir_all(work.join("Moved")).unwrap();
        sh(&work, &["mv", "Notes.txt", "Moved/Notes.txt"]);
        let moved = work.join("Moved/Notes.txt");
        // Something holds the new file, so putting the old name back works but deleting the new
        // one fails.
        #[cfg(windows)]
        let _hold = {
            use std::os::windows::fs::OpenOptionsExt;
            const SHARE_READ_WRITE: u32 = 1 | 2;
            std::fs::OpenOptions::new().read(true).share_mode(SHARE_READ_WRITE).open(&moved).unwrap()
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(work.join("Moved"), std::fs::Permissions::from_mode(0o555)).unwrap();
            if std::fs::write(work.join("Moved/probe"), b"").is_ok() {
                // Running as root, which ignores the permissions; nothing to test.
                std::fs::set_permissions(work.join("Moved"), std::fs::Permissions::from_mode(0o755)).unwrap();
                return;
            }
        }
        let err = discard_with(&Log::default(), &p, "Moved/Notes.txt", keep_in(work.parent().unwrap().join("kept-half"))).unwrap_err();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(work.join("Moved"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(err.contains("was put back"), "{err}");
        assert!(!work.join("Notes.txt").exists());
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "one\ntwo\n");
        let c = changes(&p).unwrap();
        assert_eq!((c.len(), c[0].kind.as_str(), c[0].staged, c[0].orig_path.as_deref()), (1, "R", true, Some("Notes.txt")), "{c:?}");
    }

    #[test]
    fn a_failed_stash_pop_keeps_the_stash() {
        let work = scenario("popfail");
        let p = Project::load_ignoring_busy(&work).unwrap();
        let log = Log::default();
        write(&work, "Notes.txt", b"stashed\n");
        stash_push(&log, &p, "Keep me", None).unwrap();
        let id = stashes(&p).unwrap()[0].id.clone();
        std::fs::write(work.join(".git/index.lock"), b"").unwrap();
        let err = stash_pop(&log, &p, &id).unwrap_err();
        std::fs::remove_file(work.join(".git/index.lock")).unwrap();
        assert!(err.contains("still in the list"), "{err}");
        assert_eq!(stashes(&p).unwrap().len(), 1);
        assert_eq!(std::fs::read_to_string(work.join("Notes.txt")).unwrap(), "one\ntwo\n");
        stash_pop(&log, &p, &id).unwrap();
        assert_eq!(std::fs::read_to_string(work.join("Notes.txt")).unwrap(), "stashed\n");
    }
}
