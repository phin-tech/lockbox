//! Bring Back, for the chosen sandbox: each folder that went in as a git repository comes back as the branch
//! `playpen/<sandbox>` in that repository on the Mac. Which folders can is asked of git off the window's thread, when
//! the sandbox is chosen, since it reads the repositories.

use std::path::PathBuf;

use playpen_core::{Brought, Repo, bring_back, bring_back_branch, session};

use super::sandboxes::text_value;
use super::*;

/// What Bring Back knows about the chosen sandbox.
#[derive(Default)]
pub(super) struct BringBack {
    sandbox: Option<String>,
    /// Its folders that went in with their history, once looked for.
    repos: Option<Vec<Repo>>,
    /// What the last Bring Back gave, for each repository.
    results: Vec<Result<Brought, String>>,
    working: bool,
    _looking: Option<Task<()>>,
    _working: Option<Task<()>>,
}

impl MainView {
    /// Looks for the chosen sandbox's folders that went in as repositories, and can come back.
    pub(super) fn look_up_repos(&mut self, cx: &mut Context<Self>) {
        let Some(sandbox) = self.selected_sandbox().cloned() else {
            return;
        };
        if self.bring_back.sandbox.as_ref() != Some(&sandbox.name) {
            self.bring_back.repos = None;
            self.bring_back.results.clear();
        }
        self.bring_back.sandbox = Some(sandbox.name.clone());
        if self.watch == Watch::Nothing {
            return;
        }
        self.bring_back._looking = Some(cx.spawn(async move |this, cx| {
            let name = sandbox.name.clone();
            let repos = cx
                .background_spawn(async move {
                    sandbox
                        .folders
                        .iter()
                        .filter_map(|folder| Repo::of(&PathBuf::from(folder)))
                        .filter(|repo| repo.is_seeded(&sandbox.name))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.bring_back.sandbox.as_ref() == Some(&name) {
                    this.bring_back.repos = Some(repos);
                    cx.notify();
                }
            });
        }));
    }

    /// Brings the chosen sandbox's work back, each repository's to its branch, off the window's thread.
    pub(super) fn bring_back_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sandbox) = self.selected_sandbox() else {
            return;
        };
        let Some(repos) = self
            .bring_back
            .repos
            .clone()
            .filter(|repos| !repos.is_empty())
        else {
            return;
        };
        if !sandbox.phase.is_ready() || self.bring_back.working {
            return;
        }
        let name = sandbox.name.clone();
        let Ok(openshell) = self.openshell() else {
            return;
        };
        self.bring_back.working = true;
        tracing::info!(sandbox = %name, "bringing work back");
        self.bring_back._working = Some(cx.spawn_in(window, async move |this, cx| {
            let results = cx
                .background_spawn({
                    let name = name.clone();
                    async move {
                        repos
                            .iter()
                            .map(|repo| {
                                bring_back(&openshell, &name, repo)
                                    .map_err(|error| error.to_string())
                            })
                            .collect::<Vec<_>>()
                    }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.bring_back.working = false;
                let mut ok_branch: Option<String> = None;
                for result in &results {
                    match result {
                        Ok(brought) => {
                            tracing::info!(sandbox = %name, branch = %brought.branch, "brought work back");
                            if ok_branch.is_none() {
                                ok_branch = Some(brought.branch.clone());
                            }
                        }
                        Err(error) => {
                            tracing::warn!(sandbox = %name, "couldn't bring work back: {error}")
                        }
                    }
                }
                if let Some(branch) = ok_branch
                    && results.iter().all(Result::is_ok)
                {
                    Toast::new(
                        t!("Brought work back to %{branch}", branch = branch).into_owned(),
                    )
                    .key("bring-back")
                    .show(window, cx);
                }
                this.bring_back.results = results;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Forgets what the sandbox `name` was given, in each of `folders` that's a repository, once it's deleted.
    pub(super) fn forget_seeds(name: String, folders: Vec<String>, cx: &mut Context<Self>) {
        cx.background_spawn(async move {
            for folder in folders {
                if let Some(repo) = Repo::of(&PathBuf::from(folder)) {
                    repo.forget(&name);
                }
            }
        })
        .detach();
    }

    /// The Bring Back button, enabled while the sandbox runs and has a repository to bring back.
    pub(super) fn bring_back_button(&self, sandbox: &Sandbox, cx: &Context<Self>) -> Button {
        let can = sandbox.phase.is_ready()
            && self
                .bring_back
                .repos
                .as_ref()
                .is_some_and(|repos| !repos.is_empty());
        Button::new("bring-back")
            .small()
            .icon(assets::IconName::GitBranch)
            .label(t!("Bring Back").into_owned())
            .loading(self.bring_back.working)
            .disabled(!can || self.bring_back.working)
            .tooltip(
                t!("The agent's work, to a branch in each repository on this Mac").into_owned(),
            )
            .on_click(cx.listener(|this, _, window, cx| this.run("sandbox.bring-back", window, cx)))
    }

    /// The Bring Back row: where each repository's work comes back to, and what the last Bring Back gave.
    pub(super) fn bring_back_row(&self, sandbox: &Sandbox, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let Some(repos) = &self.bring_back.repos else {
            return text_value(if self.watch == Watch::Nothing {
                t!("Not asked").into_owned()
            } else {
                t!("Looking…").into_owned()
            });
        };
        if repos.is_empty() {
            return div().text_color(theme.muted_foreground).child(
                t!("Nothing: none of its folders went in as a git repository").into_owned(),
            );
        }
        let branch = bring_back_branch(&sandbox.name);
        v_flex()
            .min_w_0()
            .gap_1()
            .items_end()
            .children(repos.iter().map(|repo| {
                let folder = repo
                    .root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let result = self
                    .bring_back
                    .results
                    .iter()
                    .find(|result| matches!(result, Ok(brought) if brought.repo == repo.root));
                h_flex()
                    .gap_2()
                    .child(shell::code_chip(format!("{folder} ▸ {branch}"), cx))
                    .when_some(
                        result.and_then(|result| result.as_ref().ok()),
                        |row, brought| {
                            let said = match (brought.commits, brought.changes.is_empty()) {
                                (_, true) => t!("nothing yet").into_owned(),
                                (1, false) => {
                                    t!("1 commit, %{changes}", changes = brought.changes.clone())
                                        .into_owned()
                                }
                                (commits, false) => t!(
                                    "%{commits} commits, %{changes}",
                                    commits = commits,
                                    changes = brought.changes.clone()
                                )
                                .into_owned(),
                            };
                            let repo = brought.repo.clone();
                            let name = sandbox.name.clone();
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(said),
                            )
                            .child(
                                Button::new(ElementId::from((
                                    ElementId::from("show-work"),
                                    folder.clone(),
                                )))
                                .ghost()
                                .xsmall()
                                .icon(assets::IconName::SquareTerminal)
                                .tooltip(t!("Show the work in Terminal").into_owned())
                                .on_click(move |_, _, _| show_work(&name, &repo)),
                            )
                        },
                    )
            }))
            .children(
                self.bring_back
                    .results
                    .iter()
                    .filter_map(|result| result.as_ref().err())
                    .map(|error| {
                        div()
                            .text_xs()
                            .text_color(theme.danger)
                            .child(error.clone())
                    }),
            )
    }
}

/// Opens Terminal in the repository with the work brought back from the sandbox `name` laid out: each commit and the
/// files it changed.
fn show_work(name: &str, repo: &std::path::Path) {
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let line = format!(
        "cd {repo} && git --no-pager log --stat --format='%n%C(yellow)%h%Creset %s' refs/playpen/seed/{name}..{branch}; exec \"$SHELL\" -l",
        repo = quote(&repo.to_string_lossy()),
        branch = bring_back_branch(name),
    );
    if let Err(error) = session::show_in_terminal(&format!("bring-back-{name}"), &line) {
        tracing::warn!("couldn't show the work: {error}");
    }
}
