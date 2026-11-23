//! The chosen sandbox's recent network activity: what its programs reached and what they were stopped from reaching,
//! newest first, from the audit lines in its log. It's read when the sandbox is chosen, when a request is decided and
//! when asked, not every few seconds, since a long log is a lot to read.

use playpen_core::{Activity, Reached};

use super::*;

/// How many are shown before expanding, and the maximum shown when expanded.
const COLLAPSED: usize = 8;
const SHOWN: usize = 40;

/// What's known of the chosen sandbox's activity.
#[derive(Default)]
pub(super) struct ActivityLog {
    /// The sandbox it's about.
    sandbox: Option<SandboxKey>,
    /// What it reached, oldest first, or why it couldn't be read.
    listed: Option<Result<Vec<Activity>, String>>,
    /// Whether only what was stopped is shown.
    only_blocked: bool,
    /// Whether the full log (up to [`SHOWN`]) is expanded rather than capped at [`COLLAPSED`].
    expanded: bool,
    _asking: Option<Task<()>>,
}

/// Why something was stopped, in words, for the reasons OpenShell gives most; any other as it gives it.
fn reason_words(reason: &str) -> String {
    match reason {
        "transparent_tcp_policy_denied" | "policy_denied" => {
            t!("Its policy doesn't allow it").into_owned()
        }
        "policy_dns_ineligible" => t!("Its policy doesn't allow the name").into_owned(),
        other => other.replace('_', " "),
    }
}

impl MainView {
    /// Reads the chosen sandbox's activity again. A test or a capture doesn't.
    pub(super) fn look_up_activity(&mut self, cx: &mut Context<Self>) {
        if self.watch == Watch::Nothing {
            return;
        }
        let Some(key) = self.selected.clone() else {
            return;
        };
        if self.activity.sandbox.as_ref() != Some(&key) {
            self.activity.listed = None;
            self.activity.expanded = false;
        }
        self.activity.sandbox = Some(key.clone());
        let Ok(openshell) = self.openshell_for(&key.gateway) else {
            return;
        };
        let name = key.name.clone();
        let read = cx.background_spawn(async move { openshell.activity(&name) });
        self.activity._asking = Some(cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                // Only if it's still the one chosen.
                if this.activity.sandbox.as_ref() == Some(&key) {
                    this.activity.listed = Some(read.map_err(|error| error.to_string()));
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// What's shown of what was read: newest first, and only what was stopped when that's chosen.
    fn shown_activity(all: &[Activity], only_blocked: bool) -> Vec<&Activity> {
        all.iter()
            .rev()
            .filter(|activity| !only_blocked || !activity.allowed)
            .collect()
    }

    pub(super) fn show_only_blocked(&mut self, only: bool, cx: &mut Context<Self>) {
        self.activity.only_blocked = only;
        cx.notify();
    }

    /// One thing reached, or stopped: when, what from and to, and the rule or the reason.
    fn activity_row(activity: &Activity, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let (what, from) = match &activity.reached {
            Reached::Connection {
                program,
                host,
                port,
            } => (
                format!("{host}:{port}"),
                Some(program.rsplit('/').next().unwrap_or(program).to_string()),
            ),
            Reached::Request { method, url } => (format!("{method} {url}"), None),
        };
        let because = activity.because.as_deref().map(|because| {
            if activity.allowed {
                t!("by %{rule}", rule = because).into_owned()
            } else {
                reason_words(because)
            }
        });
        h_flex()
            .gap_2()
            .items_start()
            .min_w_0()
            .child(
                div().flex_none().mt(px(3.)).child(
                    Icon::new(if activity.allowed {
                        assets::IconName::Check
                    } else {
                        assets::IconName::X
                    })
                    .size_3p5()
                    .text_color(if activity.allowed {
                        theme.success
                    } else {
                        theme.danger
                    }),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .w(rems(4.8))
                    .mt(px(1.))
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(activity.time_of_day()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .min_w_0()
                            .when_some(from, |line, from| {
                                line.child(div().flex_none().text_sm().child(from)).child(
                                    div()
                                        .flex_none()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child("→"),
                                )
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_family(theme.mono_font_family.clone())
                                    .child(what),
                            ),
                    )
                    .when_some(because, |row, because| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(because),
                        )
                    }),
            )
    }

    /// The chosen sandbox's recent activity, newest first, with a choice of all or only what was stopped.
    pub(super) fn activity_card(&self, key: &SandboxKey, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let only_blocked = self.activity.only_blocked;
        let current = self.activity.sandbox.as_ref() == Some(key);
        let blocked_count = self
            .activity
            .listed
            .as_ref()
            .filter(|_| current)
            .and_then(|listed| listed.as_ref().ok())
            .map(|all| all.iter().filter(|entry| !entry.allowed).count())
            .unwrap_or(0);
        let blocked_label = if blocked_count > 0 {
            t!("Blocked · %{count}", count = blocked_count).into_owned()
        } else {
            t!("Blocked").into_owned()
        };
        let chip = |only: bool, id: &'static str, label: String| {
            choice_chip(only_blocked == only, cx)
                .id(ElementId::from((ElementId::from("activity-show"), id)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.show_only_blocked(only, cx)))
        };
        let heading = h_flex()
            .gap_1p5()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font_semibold()
                    .child(t!("Recent activity").into_owned()),
            )
            .child(chip(false, "all", t!("All").into_owned()))
            .child(chip(true, "blocked", blocked_label))
            .child(
                Button::new("refresh-activity")
                    .ghost()
                    .xsmall()
                    .icon(assets::IconName::RotateCw)
                    .tooltip(t!("Refresh Activity").into_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.run("sandbox.activity", window, cx);
                    })),
            );
        let body = match self.activity.listed.as_ref().filter(|_| current) {
            None if self.watch == Watch::Nothing => hint(t!("Not asked"), cx),
            None => hint(t!("Reading its log…"), cx),
            Some(Err(error)) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(error.clone()),
            Some(Ok(all)) => {
                let shown = Self::shown_activity(all, only_blocked);
                if shown.is_empty() {
                    hint(
                        if only_blocked {
                            t!("Nothing was stopped.")
                        } else {
                            t!("Nothing yet.")
                        },
                        cx,
                    )
                } else {
                    let limit = if self.activity.expanded {
                        SHOWN
                    } else {
                        COLLAPSED
                    };
                    let total = shown.len();
                    let can_expand = !self.activity.expanded && total > COLLAPSED;
                    let more = total.saturating_sub(SHOWN);
                    v_flex()
                        .gap_2()
                        .children(
                            shown
                                .into_iter()
                                .take(limit)
                                .map(|activity| Self::activity_row(activity, cx)),
                        )
                        .when(can_expand, |list| {
                            list.child(
                                h_flex().pt_1().child(
                                    Button::new("expand-activity")
                                        .ghost()
                                        .xsmall()
                                        .label(
                                            t!("Show all %{count}", count = total.min(SHOWN))
                                                .into_owned(),
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.activity.expanded = true;
                                            cx.notify();
                                        })),
                                ),
                            )
                        })
                        .when(self.activity.expanded && more > 0, |list| {
                            list.child(hint(t!("and %{count} earlier", count = more), cx))
                        })
                }
            }
        };
        v_flex()
            .gap_3()
            .p_3()
            .rounded(theme.radius * 1.5)
            .border_1()
            .border_color(theme.border)
            .bg(shell::recessed_colour(theme))
            .child(heading)
            .child(body)
            .child(hint(
                t!("Since its gateway last started: OpenShell keeps this in memory. A connection inside HTTPS it looked into shows its address too."),
                cx,
            ))
    }
}

#[cfg(test)]
impl MainView {
    /// Gives the chosen sandbox this activity, as reading its log would.
    pub(super) fn give_activity(&mut self, activity: Vec<Activity>) {
        self.activity.sandbox = self.selected.clone();
        self.activity.listed = Some(Ok(activity));
    }

    /// What the card shows, as `(allowed, time)` pairs, newest first.
    pub(super) fn activity_shown(&self) -> Vec<(bool, i64)> {
        match &self.activity.listed {
            Some(Ok(all)) => Self::shown_activity(all, self.activity.only_blocked)
                .into_iter()
                .map(|activity| (activity.allowed, activity.at_ms))
                .collect(),
            _ => Vec::new(),
        }
    }
}

impl MainView {
    /// Gives a capture's chosen sandbox some activity to show.
    pub(super) fn give_stand_in_activity(&mut self) {
        self.activity.sandbox = self.selected.clone();
        self.activity.listed = Some(Ok(stand_in_activity()));
    }
}

/// Activity for a capture.
pub(super) fn stand_in_activity() -> Vec<Activity> {
    let connection =
        |at_ms: i64, allowed: bool, program: &str, host: &str, because: &str| Activity {
            at_ms,
            allowed,
            reached: Reached::Connection {
                program: program.to_string(),
                host: host.to_string(),
                port: "443".to_string(),
            },
            because: Some(because.to_string()),
        };
    vec![
        connection(
            1_795_375_089_504,
            false,
            "/usr/local/bin/node",
            "registry.npmjs.org",
            "transparent_tcp_policy_denied",
        ),
        connection(
            1_795_375_089_524,
            false,
            "/usr/lib/git-core/git-remote-http",
            "github.com",
            "transparent_tcp_policy_denied",
        ),
        connection(
            1_795_375_224_350,
            true,
            "/usr/local/bin/claude",
            "api.anthropic.com",
            "claude_code",
        ),
        Activity {
            at_ms: 1_795_375_224_464,
            allowed: true,
            reached: Reached::Request {
                method: "POST".to_string(),
                url: "https://api.anthropic.com/v1/messages".to_string(),
            },
            because: None,
        },
    ]
}
