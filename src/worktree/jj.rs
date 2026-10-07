//! Checkout lifecycle for colocated Jujutsu workspaces. `jj workspace add
//! --colocate` registers a Git worktree alongside the jj workspace, and
//! `jj workspace forget` removes both records.

use std::path::Path;

use super::{branch_to_path_slug, run_worktree_command, WorktreeCommand, DEFAULT_WORKTREE_PREFIX};

/// Colocated jj workspaces are also linked Git worktrees, so discovery treats
/// them as Git, but creating or removing one through Git would leave a stale
/// workspace record in jj.
pub(super) fn is_workspace(checkout: &Path) -> bool {
    checkout.join(".jj").is_dir()
}

/// jj exports bookmarks as Git branches in colocated repos, so an existing
/// branch starts the workspace on top of that bookmark. Git's `HEAD` is not a
/// jj revision; in a colocated workspace it is `@-`, which is already jj's
/// default base, so it is dropped rather than passed through.
pub(super) fn add_workspace(
    source_checkout: &Path,
    path: &Path,
    branch: &str,
    branch_exists: bool,
    base: &str,
) -> Result<(), String> {
    let revision = if branch_exists {
        Some(branch)
    } else {
        Some(base).filter(|base| *base != "HEAD")
    };
    let mut args = vec![
        "workspace".to_string(),
        "add".to_string(),
        "--colocate".to_string(),
        "--name".to_string(),
        workspace_name(branch),
    ];
    if let Some(revision) = revision {
        args.extend(["-r".to_string(), revision.to_string()]);
    }
    args.push(path.display().to_string());
    run_worktree_command(&jj_command(source_checkout, args))
}

/// The sidebar shows a jj workspace by its name when no bookmark labels it, so
/// drop herdr's generated `worktree/` prefix the way it does for Git branches.
fn workspace_name(branch: &str) -> String {
    let name = branch
        .strip_prefix(DEFAULT_WORKTREE_PREFIX)
        .and_then(|name| name.strip_prefix('/'))
        .unwrap_or(branch);
    branch_to_path_slug(name)
}

/// Working-copy changes are snapshotted first so they survive as a visible
/// commit after the workspace is forgotten. A failed snapshot only blocks an
/// unforced removal.
pub(super) fn remove_workspace(path: &Path, force: bool) -> Result<(), String> {
    match run_worktree_command(&jj_command(path, ["util", "snapshot"])) {
        Err(err) if !force => return Err(err),
        _ => {}
    }
    run_worktree_command(&jj_command(path, ["workspace", "forget"]))?;
    if path.exists() {
        std::fs::remove_dir_all(path).map_err(|err| {
            format!(
                "forgot jj workspace but failed to remove {}: {err}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn jj_command(
    workspace: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> WorktreeCommand {
    let mut full_args = vec!["-R".to_string(), workspace.display().to_string()];
    full_args.extend(args.into_iter().map(Into::into));
    WorktreeCommand {
        program: "jj".to_string(),
        args: full_args,
    }
}
