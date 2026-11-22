//! Network requests: connections a sandbox's programs tried that its policy doesn't allow, which OpenShell holds until
//! the person approves or rejects them. Every Ready sandbox's are asked for every few listings, so the sidebar can
//! say which are waiting, and the chosen sandbox's again whenever it's chosen or one of them is decided.

use std::collections::{BTreeMap, BTreeSet};

use playpen_core::{NetworkRequest, RequestStatus};

use super::SandboxKey;
use super::*;

/// How many listings go by between asking every Ready sandbox for its requests, since each is a call to OpenShell.
pub(super) const ASK_EVERY: usize = 4;

/// What's known of each sandbox's network requests.
#[derive(Default)]
pub(super) struct Requests {
    /// Each sandbox's requests, waiting and decided, once asked.
    pub(super) by_sandbox: BTreeMap<SandboxKey, Vec<NetworkRequest>>,
    /// The requests being approved or rejected, by their IDs.
    deciding: BTreeSet<String>,
    _asking: Option<Task<()>>,
    _deciding: Option<Task<()>>,
}

impl Requests {
    /// The requests of `key` still waiting.
    pub(super) fn pending(&self, key: &SandboxKey) -> Vec<&NetworkRequest> {
        self.by_sandbox
            .get(key)
            .map(|requests| {
                requests
                    .iter()
                    .filter(|request| request.is_pending())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl MainView {
    /// Takes in what OpenShell said of some sandboxes' requests; one that couldn't be asked keeps what was known.
    pub(super) fn requests_listed(
        &mut self,
        listed: Vec<(SandboxKey, Result<Vec<NetworkRequest>, OpenShellError>)>,
        cx: &mut Context<Self>,
    ) {
        for (key, requests) in listed {
            match requests {
                Ok(requests) => {
                    self.requests.by_sandbox.insert(key, requests);
                }
                Err(error) => {
                    tracing::debug!(sandbox = %key.name, "couldn't ask for its network requests: {error}")
                }
            }
        }
        cx.notify();
    }

    /// Asks OpenShell for the chosen sandbox's requests now, rather than at the next round. A test or a capture
    /// doesn't ask.
    pub(super) fn look_up_requests(&mut self, cx: &mut Context<Self>) {
        if self.watch == Watch::Nothing {
            return;
        }
        let Some(key) = self.selected.clone() else {
            return;
        };
        let Ok(openshell) = self.openshell_for(&key.gateway) else {
            return;
        };
        let name = key.name.clone();
        let asked = cx.background_spawn(async move { openshell.network_requests(&name) });
        self.requests._asking = Some(cx.spawn(async move |this, cx| {
            let asked = asked.await;
            let _ = this.update(cx, |this, cx| this.requests_listed(vec![(key, asked)], cx));
        }));
    }

    /// Approves or rejects the requests `ids` of the chosen sandbox, one after another, then asks for them again.
    pub(super) fn decide_requests(
        &mut self,
        ids: Vec<String>,
        approve: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        if ids.is_empty() || !self.requests.deciding.is_empty() {
            return;
        }
        let Ok(openshell) = self.openshell_for(&key.gateway) else {
            return;
        };
        tracing::info!(sandbox = %key.name, count = ids.len(), approve, "deciding network requests");
        self.failed = None;
        self.requests.deciding = ids.iter().cloned().collect();
        let name = key.name.clone();
        let done = cx.background_spawn(async move {
            ids.iter().try_for_each(|id| {
                if approve {
                    openshell.approve_request(&name, id)
                } else {
                    openshell.reject_request(&name, id)
                }
            })
        });
        self.requests._deciding = Some(cx.spawn(async move |this, cx| {
            let done = done.await;
            let _ = this.update(cx, |this, cx| {
                this.requests.deciding.clear();
                if let Err(error) = done {
                    tracing::warn!("couldn't decide a network request: {error}");
                    this.failed = Some(error.to_string());
                }
                this.look_up_requests(cx);
                this.look_up_activity(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Approves every request of the chosen sandbox that's showing as waiting: those the person has seen, never one
    /// that arrived since, as `rule approve-all` would.
    pub(super) fn approve_all_requests(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let ids = self
            .requests
            .pending(&key)
            .into_iter()
            .map(|request| request.id.clone())
            .collect();
        self.decide_requests(ids, true, cx);
    }

    /// A request in a row: what it would let through, and why OpenShell proposes it, with its buttons when it waits.
    fn request_row(&self, request: &NetworkRequest, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let deciding = self.requests.deciding.contains(&request.id);
        let busy = !self.requests.deciding.is_empty();
        let id = |what: &'static str| ElementId::from((ElementId::from(what), request.id.clone()));
        let programs = request
            .programs
            .iter()
            .map(|program| program.rsplit('/').next().unwrap_or(program).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let endpoints = request.endpoints.join(", ");
        let status = match &request.status {
            RequestStatus::Pending => None,
            RequestStatus::Approved => Some((t!("Approved").into_owned(), theme.success)),
            RequestStatus::Rejected => Some((t!("Rejected").into_owned(), theme.danger)),
            RequestStatus::Other(status) => Some((status.clone(), theme.muted_foreground)),
        };
        h_flex()
            .gap_3()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .min_w_0()
                            .child(div().text_sm().font_medium().child(programs))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("→"),
                            )
                            .child(shell::code_chip(endpoints, cx)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(request.programs.join(", ")),
                    )
                    .when(!request.rationale.is_empty(), |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(request.rationale.clone()),
                        )
                    }),
            )
            .map(|row| match status {
                Some((said, colour)) => {
                    row.child(div().flex_none().text_xs().text_color(colour).child(said))
                }
                None => row.child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .child(
                            Button::new(id("reject-request"))
                                .xsmall()
                                .ghost()
                                .label(t!("Reject").into_owned())
                                .disabled(busy)
                                .on_click(cx.listener({
                                    let id = request.id.clone();
                                    move |this, _, _, cx| {
                                        this.decide_requests(vec![id.clone()], false, cx)
                                    }
                                })),
                        )
                        .child(
                            Button::new(id("approve-request"))
                                .xsmall()
                                .primary()
                                .label(t!("Approve").into_owned())
                                .loading(deciding)
                                .disabled(busy)
                                .on_click(cx.listener({
                                    let id = request.id.clone();
                                    move |this, _, _, cx| {
                                        this.decide_requests(vec![id.clone()], true, cx)
                                    }
                                })),
                        ),
                ),
            })
    }

    /// The chosen sandbox's network requests: those waiting first, with Approve All, then the last few decided.
    pub(super) fn requests_card(&self, key: &SandboxKey, cx: &Context<Self>) -> Option<Div> {
        let theme = cx.theme();
        let requests = self.requests.by_sandbox.get(key)?;
        if requests.is_empty() {
            return None;
        }
        let pending: Vec<&NetworkRequest> = requests
            .iter()
            .filter(|request| request.is_pending())
            .collect();
        let decided: Vec<&NetworkRequest> = requests
            .iter()
            .filter(|request| !request.is_pending())
            .collect();
        let busy = !self.requests.deciding.is_empty();
        let heading = h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font_semibold()
                    .child(match pending.len() {
                        0 => t!("Network requests").into_owned(),
                        count => {
                            t!("Network requests waiting · %{count}", count = count).into_owned()
                        }
                    }),
            )
            .when(pending.len() > 1, |heading| {
                heading.child(
                    Button::new("approve-all-requests")
                        .xsmall()
                        .outline()
                        .label(t!("Approve All").into_owned())
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run("sandbox.approve-requests", window, cx)
                        })),
                )
            });
        Some(
            v_flex()
                .gap_3()
                .p_3()
                .rounded(theme.radius * 1.5)
                .border_1()
                .border_color(if pending.is_empty() { theme.border } else { theme.warning.opacity(0.5) })
                .bg(shell::recessed_colour(theme))
                .child(heading)
                .children(pending.iter().map(|request| self.request_row(request, cx)))
                .when(!pending.is_empty(), |card| {
                    card.child(hint(
                        t!("Something in the sandbox tried to reach these and was stopped. An approval takes a few seconds to reach the sandbox, and whatever tried has to try again."),
                        cx,
                    ))
                })
                .when(!decided.is_empty(), |card| {
                    card.child(
                        v_flex()
                            .gap_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(shell::hairline(theme))
                            .children(decided.iter().rev().take(5).map(|request| self.request_row(request, cx))),
                    )
                }),
        )
    }

    /// The words under a sandbox in the list when it has requests waiting.
    pub(super) fn waiting_words(&self, key: &SandboxKey) -> Option<String> {
        match self.requests.pending(key).len() {
            0 => None,
            1 => Some(t!("1 request waiting").into_owned()),
            count => Some(t!("%{count} requests waiting", count = count).into_owned()),
        }
    }
}

/// Requests for a capture, so the card and the sidebar can be seen without a sandbox that asked for anything.
pub(super) fn stand_in_requests() -> Vec<NetworkRequest> {
    let request = |id: &str, status: RequestStatus, host: &str, program: &str| NetworkRequest {
        id: id.to_string(),
        status,
        rule: format!("allow_{}_443", host.replace('.', "_")),
        programs: vec![program.to_string()],
        endpoints: vec![format!("{host}:443 [L4]")],
        confidence: Some(65),
        rationale: format!(
            "Allow {} to connect to {host}:443 (HTTPS).",
            program.rsplit('/').next().unwrap_or(program)
        ),
    };
    vec![
        request(
            "a1",
            RequestStatus::Pending,
            "registry.npmjs.org",
            "/usr/local/bin/node",
        ),
        request(
            "a2",
            RequestStatus::Pending,
            "github.com",
            "/usr/lib/git-core/git-remote-http",
        ),
        request(
            "a3",
            RequestStatus::Approved,
            "pypi.org",
            "/usr/bin/python3",
        ),
    ]
}

#[cfg(test)]
impl MainView {
    /// Gives the sandbox `key` these requests, as asking OpenShell would.
    pub(super) fn give_requests(&mut self, key: SandboxKey, requests: Vec<NetworkRequest>) {
        self.requests.by_sandbox.insert(key, requests);
    }
}
