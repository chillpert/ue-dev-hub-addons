//! The Git addon's helper: answers the Git page's calls. The hub starts it and talks to it
//! over stdin and stdout, see the ue-dev-hub-addon crate.

mod gitui;
mod hub;

use hub::{Log, Project};
use serde::de::DeserializeOwned;
use serde_json::Value;
use ue_dev_hub_addon::Ctx;

/// Calls that change the repository. The hub must run them as steps, so they never overlap
/// a job, Update & Launch or each other.
const STEPS: &[&str] = &["stage", "unstage", "discard", "commit", "push", "pull", "createBranch", "switchBranch", "stashPush", "stashPop", "stashDrop", "removeIndexLock"];

fn arg<T: DeserializeOwned>(params: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(params.get(key).cloned().unwrap_or(Value::Null)).map_err(|_| format!("The call is missing '{key}'."))
}

fn json<T: serde::Serialize>(v: Result<T, String>) -> Result<Value, String> {
    v.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
}

fn handle(ctx: &Ctx, method: &str, params: Value) -> Result<Value, String> {
    if STEPS.contains(&method) && !ctx.is_step() {
        return Err(format!("'{method}' changes the repository and must run as a step."));
    }
    let p = Project::from_ctx(ctx)?;
    let log = Log::new(ctx);
    let a = &params;
    match method {
        "overview" => json(gitui::overview(&p)),
        "diff" => json(gitui::diff(&p, &arg::<String>(a, "path")?)),
        "stage" => json(gitui::stage(&log, &p, &arg::<Vec<String>>(a, "paths")?)),
        "unstage" => json(gitui::unstage(&log, &p, &arg::<Vec<String>>(a, "paths")?)),
        "discard" => json(gitui::discard_with(&log, &p, &arg::<String>(a, "path")?, gitui::to_trash)),
        "commit" => json(gitui::commit(&log, &p, &arg::<String>(a, "summary")?, &arg::<String>(a, "body")?, &arg::<Vec<String>>(a, "paths")?)),
        "push" => json(gitui::push(&log, &p)),
        "pull" => json(gitui::pull(&log, &p, arg(a, "push")?)),
        "createBranch" => json(gitui::create_branch(&log, &p, &arg::<String>(a, "name")?)),
        "switchBranch" => json(gitui::switch_branch(&log, &p, &arg::<String>(a, "name")?)),
        "stashes" => json(gitui::stashes(&p)),
        "stashPush" => json(gitui::stash_push(&log, &p, &arg::<String>(a, "name")?, arg(a, "paths")?)),
        "stashPop" => json(gitui::stash_pop(&log, &p, &arg::<String>(a, "id")?)),
        "stashDrop" => json(gitui::stash_drop(&log, &p, &arg::<String>(a, "id")?)),
        "history" => json(gitui::history(&p, 300)),
        "commitDetails" => json(gitui::commit_details(&p, &arg::<String>(a, "hash")?)),
        "commitDiff" => json(gitui::commit_diff(&p, &arg::<String>(a, "hash")?, &arg::<String>(a, "path")?)),
        "fileHistory" => json(gitui::file_history(&p, &arg::<String>(a, "path")?)),
        "locks" => json(gitui::locks(&p)),
        "removeIndexLock" => json(gitui::remove_index_lock(&p)),
        _ => Err(format!("The Git addon doesn't know '{method}'.")),
    }
}

fn main() {
    ue_dev_hub_addon::serve(handle)
}
