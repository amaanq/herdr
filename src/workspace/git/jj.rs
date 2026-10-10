//! Status for colocated Jujutsu workspaces. Their Git HEAD is never a branch,
//! so the branch label and ahead/behind counts come from jj instead: the
//! label is a bookmark on `@` or `@-`, falling back to the workspace name for
//! linked workspaces, and the counts compare `@` with `trunk()`. The same
//! query flags a conflicted `@`.
//!
//! Results are cached on jj's operation heads, which change with every jj
//! operation. jj runs with `--ignore-working-copy` so a background refresh
//! never snapshots, and thereby never races an agent working in the
//! workspace.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::status::GitStatusRefreshDemand;
use crate::workspace::{GitSpaceMetadata, WorkspaceGitStatusSnapshot};

/// Bookmarks on trunk are excluded so freshly created workspaces, whose `@-`
/// is usually trunk, keep their own name. The counts already show where trunk
/// is. `jj log` lists `@` before `@-`, so a bookmark on `@` wins. Each row is
/// `bookmarks<TAB>workspace names`, the names only on `@`.
const LABEL_REVSET: &str = "(@ | @-) ~ trunk()";
const LABEL_TEMPLATE: &str = concat!(
    r#"local_bookmarks.map(|b| b.name()).join(" ") ++ "\t" ++ "#,
    r#"if(current_working_copy, working_copies.map(|w| w.name()).join(" ")) ++ "\n""#,
);

/// Without a trunk bookmark `trunk()` is the root commit. Restricting ahead to
/// descendants of a real trunk keeps that from counting all of history. A
/// conflicted `@` is pulled in even when it is neither ahead nor behind.
const COUNT_REVSET: &str =
    "((trunk()..@ ~ empty()) & (trunk() ~ root())::) | @..trunk() | (@ & conflicts())";
const COUNT_TEMPLATE: &str = concat!(
    r#"if(self.contained_in("@..trunk()"), "-") ++ "#,
    r#"if(self.contained_in("(trunk()..@ ~ empty()) & (trunk() ~ root())::"), "+") ++ "#,
    r#"if(current_working_copy && conflict, "!")"#,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JjStatusCache {
    op_heads: Vec<OsString>,
    demand: GitStatusRefreshDemand,
    label: Option<String>,
    ahead_behind: Option<(usize, usize)>,
    conflicted: bool,
}

impl JjStatusCache {
    pub(super) fn apply(&self, snapshot: &mut WorkspaceGitStatusSnapshot) {
        snapshot.branch = self.label.clone();
        snapshot.ahead_behind = self.ahead_behind;
        snapshot.conflicted = self.conflicted;
    }
}

/// Returns `None` when the checkout is not a jj workspace. A failing `jj` is
/// cached like a successful one, so it is not retried until the next jj
/// operation.
pub(super) fn jj_status(
    space: &GitSpaceMetadata,
    cached: Option<&JjStatusCache>,
    demand: GitStatusRefreshDemand,
) -> Option<JjStatusCache> {
    let checkout = &space.repo_root;
    let op_heads = read_op_heads(checkout)?;
    if let Some(cached) =
        cached.filter(|cached| cached.op_heads == op_heads && cached.demand == demand)
    {
        return Some(cached.clone());
    }

    let label = demand
        .branch
        .then(|| jj_log(checkout, LABEL_REVSET, LABEL_TEMPLATE))
        .flatten()
        .and_then(|stdout| parse_label(&stdout, space.is_linked_worktree));
    let counts = demand
        .ahead_behind
        .then(|| jj_log(checkout, COUNT_REVSET, COUNT_TEMPLATE))
        .flatten();
    Some(JjStatusCache {
        op_heads,
        demand,
        label,
        ahead_behind: counts
            .as_ref()
            .map(|stdout| (stdout.matches('+').count(), stdout.matches('-').count())),
        conflicted: counts.is_some_and(|stdout| stdout.contains('!')),
    })
}

/// The main workspace falls back to no label, like a detached Git HEAD,
/// rather than jj's `default` workspace name.
fn parse_label(stdout: &str, linked: bool) -> Option<String> {
    let rows = stdout
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect::<Vec<_>>();
    let first_word = |words: &str| words.split_whitespace().next().map(str::to_string);
    rows.iter()
        .find_map(|(bookmarks, _)| first_word(bookmarks))
        .or_else(|| {
            linked
                .then(|| rows.iter().find_map(|(_, names)| first_word(names)))
                .flatten()
        })
}

/// Secondary workspaces store the path of the main workspace's repo
/// directory in `.jj/repo`, relative to `.jj` unless absolute.
fn op_heads_dir(checkout: &Path) -> Option<PathBuf> {
    let jj_dir = checkout.join(".jj");
    let repo = jj_dir.join("repo");
    let repo = if repo.is_dir() {
        repo
    } else {
        jj_dir.join(std::fs::read_to_string(&repo).ok()?.trim_end())
    };
    Some(repo.join("op_heads").join("heads"))
}

fn read_op_heads(checkout: &Path) -> Option<Vec<OsString>> {
    let mut heads = std::fs::read_dir(op_heads_dir(checkout)?)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
        .collect::<Vec<_>>();
    heads.sort();
    Some(heads)
}

fn jj_log(checkout: &Path, revset: &str, template: &str) -> Option<String> {
    let output = crate::noninteractive_process::command("jj")
        .args(["--ignore-working-copy", "--color=never", "-R"])
        .arg(checkout)
        .args(["log", "--no-graph", "-r", revset, "-T", template])
        .output()
        .ok()?;
    if !output.status.success() {
        tracing::debug!(
            checkout = %checkout.display(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "jj status query failed"
        );
        return None;
    }
    String::from_utf8(output.stdout).ok()
}
