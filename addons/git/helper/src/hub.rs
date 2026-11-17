//! What gitui.rs used from inside the hub, now from outside it. Running Git is done here;
//! the hub's own Git logic (its update flow, busy checks, backups and submodules) is asked
//! for through host calls, so it exists only once, in the hub.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use ue_dev_hub_addon::Ctx;
use ue_dev_hub_addon::protocol::host;

/// Where log lines go: the hub's console in a step, nowhere otherwise.
#[derive(Clone, Default)]
pub struct Log(Option<Ctx>);

impl Log {
    pub fn new(ctx: &Ctx) -> Log {
        Log(Some(ctx.clone()))
    }

    pub fn info(&self, text: impl Into<String>) {
        if let Some(c) = &self.0 {
            c.info(text);
        }
    }

    pub fn detail(&self, text: impl Into<String>) {
        if let Some(c) = &self.0 {
            c.detail(text);
        }
    }
}

pub struct GitSettings {
    pub update: String,
    pub push: String,
    /// The kinds of branch to pick from when making one, like "feat". Empty: any name.
    pub branch_prefixes: Vec<String>,
}

pub struct Config {
    pub git: GitSettings,
}

/// The project a call is about.
pub struct Project {
    /// The top of the Git repository, which every path from Git is relative to. The hub's
    /// project folder (where the .uproject is) can be deeper inside it.
    pub root: PathBuf,
    pub uproject: PathBuf,
    pub config: Config,
    base: String,
    ctx: Ctx,
}

impl Project {
    pub fn from_ctx(ctx: &Ctx) -> Result<Project, String> {
        let p = ctx.project()?;
        Ok(Project {
            root: repo_root(&p.root),
            uproject: p.uproject.clone(),
            config: Config { git: GitSettings { update: p.git_update.clone(), push: p.git_push.clone(), branch_prefixes: p.branch_prefixes.clone() } },
            base: p.base_branch.clone(),
            ctx: ctx.quiet(),
        })
    }

    pub fn default_branch(&self) -> String {
        self.base.clone()
    }

    /// Whether Unreal has this project open. Yes when it can't tell, to stay on the safe side.
    pub fn editor_running(&self) -> bool {
        self.ctx.host_as(host::EDITOR_RUNNING, Value::Null).unwrap_or(true)
    }

    fn host<T: serde::de::DeserializeOwned>(&self, name: &str, params: Value) -> Result<T, String> {
        self.ctx.host_as(name, params)
    }
}

/// The top of the Git repository that `dir` is in, or `dir` itself when it isn't in one (the
/// page then says so). Asked once per folder.
fn repo_root(dir: &Path) -> PathBuf {
    static ROOTS: Mutex<Option<HashMap<PathBuf, PathBuf>>> = Mutex::new(None);
    if let Some(root) = ROOTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(dir))
        && root.is_dir()
    {
        return root.clone();
    }
    let top = proc::run_timeout("git", &["rev-parse", "--show-toplevel"], Some(dir), 30).ok().filter(|o| o.ok && !o.stdout.is_empty());
    let Some(top) = top else { return dir.to_path_buf() };
    // Git writes C:/Users/...; the page shows this path, so it gets the system's own slashes.
    let root = PathBuf::from(if cfg!(windows) { top.stdout.replace('/', "\\") } else { top.stdout });
    ROOTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(dir.to_path_buf(), root.clone());
    root
}

pub mod config {
    use std::path::{Component, Path};

    /// A path inside the project: relative, without "..", drive letters or a leading slash.
    /// Colons are fine in file names on Linux; on Windows they'd name a drive or a hidden stream.
    pub fn is_inside(path: &str) -> bool {
        let p = Path::new(path);
        !path.is_empty()
            && !(cfg!(windows) && path.contains(':'))
            && !path.starts_with(['/', '\\'])
            && !path.split(['/', '\\']).any(|part| part == "..")
            && !p.is_absolute()
            && p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
            && p.components().any(|c| matches!(c, Component::Normal(_)))
    }
}

pub mod proc {
    use super::Log;
    use std::process::Command;
    use std::time::Duration;
    #[cfg(test)]
    pub use ue_dev_hub_addon::cmd::run;
    pub use ue_dev_hub_addon::cmd::{Output, capture, command, is_running, run_timeout};

    /// Runs `cmd`, sending its output to the log. With `idle`, it's stopped once it shows no
    /// progress for that long.
    pub fn stream(log: &Log, cmd: Command, idle: Option<Duration>, on_line: impl FnMut(&str) -> bool) -> Result<(bool, Vec<String>), String> {
        match &log.0 {
            Some(ctx) => ue_dev_hub_addon::cmd::stream_idle(ctx, cmd, idle, on_line),
            None => ue_dev_hub_addon::cmd::stream_idle(&ue_dev_hub_addon::Ctx::standalone(), cmd, idle, on_line),
        }
    }
}

pub mod git {
    use super::*;

    /// Long enough for a slow fetch or a first sign-in, short enough that nothing hangs forever.
    const GIT_TIMEOUT_SECS: u64 = 600;

    /// The hub's `UpdateResult`, passed on to the page as it is.
    pub type UpdateResult = Value;

    pub fn git(log: &Log, project: &Project, args: &[&str]) -> Result<proc::Output, String> {
        log.info(format!("> git {}", args.join(" ")));
        let out = proc::run_timeout("git", args, Some(&project.root), GIT_TIMEOUT_SECS)?;
        for line in out.stdout.lines().chain(out.stderr.lines()) {
            log.detail(line.to_string());
        }
        Ok(out)
    }

    /// Git's output exactly as it printed it, for diffs: trimming would drop their last line when
    /// it's an unchanged blank one.
    pub fn git_raw(project: &Project, args: &[&str]) -> Result<proc::Output, String> {
        let mut cmd = proc::command("git");
        cmd.args(args).current_dir(&project.root);
        let out = proc::capture(cmd, GIT_TIMEOUT_SECS)?;
        Ok(proc::Output { ok: out.ok, stdout: String::from_utf8_lossy(&out.stdout).into_owned(), stderr: String::from_utf8_lossy(&out.stderr).trim().to_string() })
    }

    pub fn git_ok(log: &Log, project: &Project, args: &[&str], error: &str) -> Result<proc::Output, String> {
        let out = git(log, project, args)?;
        if out.ok { Ok(out) } else { Err(format!("{error}\n\n{}", out.message())) }
    }

    /// What an update brings in, as the hub defines it (see its git.rs).
    pub fn update_target(on_base: bool, base: &str) -> String {
        if on_base { format!("refs/remotes/origin/{base}") } else { format!("refs/heads/{base}") }
    }

    /// An unfinished Git operation or a lock file, as the hub sees it.
    pub fn repo_busy(project: &Project) -> Option<String> {
        project.host::<Option<String>>(host::REPO_BUSY, Value::Null).unwrap_or_else(Some)
    }

    pub fn foreign_server_commits(_log: &Log, project: &Project, branch: &str, server: &str) -> Result<usize, String> {
        project.host(host::FOREIGN_SERVER_COMMITS, json!({ "branch": branch, "server": server }))
    }

    pub fn update_submodules(_log: &Log, project: &Project) -> Vec<String> {
        project.host(host::UPDATE_SUBMODULES, Value::Null).unwrap_or_else(|e| vec![e])
    }

    pub fn apply_update(_log: &Log, project: &Project, push: bool) -> Result<UpdateResult, String> {
        project.host(host::APPLY_UPDATE, json!({ "push": push }))
    }

    #[cfg(test)]
    pub fn check_updates(_log: &Log, project: &Project) -> Result<Value, String> {
        project.host(host::CHECK_UPDATES, Value::Null)
    }

    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct Commit {
        pub hash: String,
        pub author: String,
        pub email: String,
        /// How long ago it was committed, like "3 hours ago".
        pub when: String,
        pub subject: String,
    }

    impl Commit {
        /// The `git log --pretty` format that [`Commit::parse`] reads.
        pub const FORMAT: &'static str = "--pretty=format:%h%x1f%an%x1f%ae%x1f%ar%x1f%s";

        pub fn parse(line: &str) -> Option<Commit> {
            let mut parts = line.splitn(5, '\u{1f}');
            Some(Commit {
                hash: parts.next()?.into(),
                author: parts.next()?.into(),
                email: parts.next()?.into(),
                when: parts.next()?.into(),
                subject: parts.next()?.into(),
            })
        }

        pub fn parse_log(out: &str) -> Vec<Commit> {
            out.lines().filter_map(Commit::parse).collect()
        }
    }
}

/// For tests: a project read from its folder, with a stand-in for the hub's services that
/// behaves like the hub for what the tests need.
#[cfg(test)]
impl Project {
    pub fn load(root: &Path) -> Result<Project, String> {
        Project::load_with(root, false)
    }

    /// Like `load`, but the hub always says Git is idle, so a test can make Git itself fail
    /// halfway through a step.
    pub fn load_ignoring_busy(root: &Path) -> Result<Project, String> {
        Project::load_with(root, true)
    }

    fn load_with(root: &Path, never_busy: bool) -> Result<Project, String> {
        let read = |f: &str| std::fs::read_to_string(root.join(f)).ok().and_then(|t| t.parse::<toml::Table>().ok()).unwrap_or_default();
        let (team, local) = (read("ue-dev-hub.toml"), read("ue-dev-hub.local.toml"));
        let get = |section: &str, key: &str| {
            [&local, &team].iter().find_map(|t| t.get(section)?.get(key)?.as_str().map(str::to_string))
        };
        let uproject = std::fs::read_dir(root).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "uproject")).ok_or("No .uproject.")?;
        let context = ue_dev_hub_addon::protocol::ProjectContext {
            root: root.to_path_buf(),
            uproject,
            name: "Game".into(),
            base_branch: get("project", "base_branch").unwrap_or_else(|| "main".into()),
            git_update: get("git", "update").unwrap_or_else(|| "merge".into()),
            git_push: get("git", "push").unwrap_or_else(|| "ask".into()),
            branch_prefixes: Vec::new(),
        };
        let dir = root.to_path_buf();
        let base = context.base_branch.clone();
        let ctx = Ctx::for_tests(context, true, move |name, params| {
            if never_busy && name == host::REPO_BUSY {
                return Ok(Value::Null);
            }
            test_host(&dir, &base, name, params)
        });
        Project::from_ctx(&ctx)
    }
}

#[cfg(test)]
fn test_host(root: &Path, base: &str, name: &str, params: Value) -> Result<Value, String> {
    let git = |args: &[&str]| proc::run("git", args, Some(root));
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])?.stdout;
    let backups = format!("refs/ue-dev-hub/backups/{}/", branch.replace(['/', '\\'], "-"));
    match name {
        host::EDITOR_RUNNING => Ok(json!(false)),
        host::REPO_BUSY => {
            let dir = PathBuf::from(git(&["rev-parse", "--absolute-git-dir"])?.stdout);
            let busy = if dir.join("rebase-merge").exists() || dir.join("rebase-apply").exists() {
                Some("A rebase was started earlier and never finished.")
            } else if dir.join("MERGE_HEAD").exists() {
                Some("A merge was started earlier and never finished.")
            } else if dir.join("index.lock").exists() {
                Some("Another program is using Git right now (index.lock exists).")
            } else {
                None
            };
            Ok(json!(busy))
        }
        host::FOREIGN_SERVER_COMMITS => {
            let server = params["server"].as_str().unwrap_or_default();
            let out = git(&["rev-list", "--count", server, "--not", "HEAD", &format!("--glob={backups}*")])?;
            Ok(json!(out.stdout.trim().parse::<usize>().unwrap_or(0)))
        }
        host::UPDATE_SUBMODULES => {
            let _ = git(&["submodule", "update", "--init", "--recursive"]);
            Ok(json!([]))
        }
        host::CHECK_UPDATES => {
            git(&["fetch", "-q", "origin"])?;
            if branch != base {
                git(&["fetch", "-q", "origin", &format!("{base}:{base}")])?;
            }
            Ok(json!({}))
        }
        host::APPLY_UPDATE => {
            let backup = format!("{backups}1");
            git(&["update-ref", &backup, "HEAD"])?;
            let out = git(&["rebase", &format!("refs/heads/{base}")])?;
            if !out.ok {
                return Err(out.message());
            }
            Ok(json!({ "backup": backup, "pushed": false, "commits": 0, "warnings": [] }))
        }
        _ => Err(format!("The test hub doesn't offer '{name}'.")),
    }
}
