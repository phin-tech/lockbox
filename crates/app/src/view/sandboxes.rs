//! The sandboxes on OpenShell's gateway: listed every few seconds, the chosen one shown beside the list, and
//! opening, starting, stopping and deleting it. Every call to OpenShell runs off the window's thread.

use std::path::PathBuf;
use std::time::Duration;

use playpen_core::{GatewayInfo, HerdrCore, Installed, Keep, Opened, Phase, session};

use crate::providers::ShownGateway;

use super::*;

/// How often the sandboxes are listed again while the window's open. OpenShell sends nothing when one changes.
const LIST_EVERY: Duration = Duration::from_secs(3);

/// The gateway a key of the lists names: the active one for an empty key, which is what there is before they're listed.
fn gateway_named(key: &str) -> Gateway {
    if key.is_empty() {
        Gateway::Active
    } else {
        Gateway::named(key)
    }
}

/// Something done to a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Act {
    Start,
    /// Starts a sandbox that failed, which takes a stop first.
    Restart,
    Stop,
    Delete,
}

impl MainView {
    /// Lists the sandboxes now, and then every few seconds while the window's open.
    pub(super) fn watch_sandboxes(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(openshell) = this.update(cx, |this, _| this.base_openshell()) else {
                    break;
                };
                // The gateways come first, since the one in focus may have gone, and which are asked depends on them.
                let registered = openshell.clone();
                let gateways = cx
                    .background_spawn(async move {
                        registered.and_then(|openshell| openshell.gateways())
                    })
                    .await;
                let Ok((openshell, polled, added)) = this.update(cx, |this, cx| {
                    this.gateways_listed(gateways);
                    cx.set_global(ShownGateway(this.gateway.clone()));
                    (this.base_openshell(), this.polled(), this.added.clone())
                }) else {
                    break;
                };
                // Each at the same time, so one that's slow to answer doesn't hold the others up.
                let asks: Vec<_> = polled
                    .into_iter()
                    .map(|key| {
                        let openshell = openshell.clone();
                        let added = added.clone();
                        cx.background_spawn(async move {
                            let mut listed = openshell.and_then(|openshell| {
                                openshell.on(gateway_named(&key)).sandboxes()
                            });
                            // With the folders added to each since it was made, after those it was made with.
                            if let (Ok(sandboxes), Some(added)) = (&mut listed, &added) {
                                added.fill(sandboxes);
                            }
                            (key, listed)
                        })
                    })
                    .collect();
                let listings = futures::future::join_all(asks).await;
                // What each agent is doing, when herdr-core keeps their sessions; nothing without it.
                let states = cx
                    .background_spawn(
                        async move { HerdrCore::find().map(|core| core.agent_states()) },
                    )
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.agent_states = match states {
                            Some(Ok(states)) => {
                                this.herdr_failed = None;
                                states
                            }
                            // Logged once, not every few seconds, since it's asked again every few seconds.
                            Some(Err(error)) => {
                                let error = error.to_string();
                                if this.herdr_failed.as_ref() != Some(&error) {
                                    tracing::warn!(
                                        "herdr-core's sessions couldn't be read: {error}"
                                    );
                                    this.herdr_failed = Some(error);
                                }
                                BTreeMap::new()
                            }
                            None => BTreeMap::new(),
                        };
                        for (key, listed) in listings {
                            this.listed_for(&key, listed, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(LIST_EVERY).await;
            }
        })
    }

    /// The `openshell` command, looked for again if it wasn't there before, since it may have been installed since,
    /// not set to go to any gateway.
    pub(super) fn base_openshell(&mut self) -> Result<OpenShell, OpenShellError> {
        if self.openshell.is_err() {
            self.openshell = OpenShell::find();
        }
        self.openshell.clone()
    }

    /// The `openshell` command, set to go to the gateway in focus.
    pub(super) fn openshell(&mut self) -> Result<OpenShell, OpenShellError> {
        let gateway = self.gateway.clone();
        self.base_openshell().map(|openshell| openshell.on(gateway))
    }

    /// The `openshell` command, set to go to the gateway `key` names, which is the active one when it's empty.
    pub(super) fn openshell_for(&mut self, key: &str) -> Result<OpenShell, OpenShellError> {
        self.base_openshell()
            .map(|openshell| openshell.on(gateway_named(key)))
    }

    /// The gateway in focus, by the name its sandboxes are kept under: empty before the gateways are listed.
    pub(super) fn gateway_key(&self) -> String {
        self.gateway.name().unwrap_or("").to_string()
    }

    /// What's known of the sandboxes of the gateway `key` names.
    pub(super) fn listing_of(&self, key: &str) -> &Listing {
        self.lists.get(key).unwrap_or(&Listing::Loading)
    }

    /// What's known of the sandboxes of the gateway in focus.
    pub(super) fn listing(&self) -> &Listing {
        self.listing_of(&self.gateway_key())
    }

    /// The gateways to ask, which are the ones shown open and the one in focus, whatever it shows.
    pub(super) fn polled(&self) -> Vec<String> {
        let mut keys: BTreeSet<String> = self
            .expanded
            .iter()
            .filter(|key| self.gateways.iter().any(|gateway| &gateway.name == *key))
            .cloned()
            .collect();
        keys.insert(self.gateway_key());
        keys.into_iter().collect()
    }

    /// The gateways in the order the sidebar lists them, which is only the active one before they're listed.
    fn gateway_keys(&self) -> Vec<String> {
        if self.gateways.is_empty() {
            vec![String::new()]
        } else {
            self.gateways
                .iter()
                .map(|gateway| gateway.name.clone())
                .collect()
        }
    }

    /// Whether the sidebar shows the sandboxes of the gateway `key` names under it.
    fn is_open(&self, key: &str) -> bool {
        key.is_empty() || self.expanded.contains(key)
    }

    /// The name herdr and the terminal know a sandbox by: its own, unless it's on a gateway other than the one OpenShell
    /// runs on this Mac, which comes before it, so two gateways' sandboxes of one name don't share a session.
    pub(super) fn label_of(&self, key: &SandboxKey) -> String {
        let own = key.gateway.is_empty()
            || self
                .gateways
                .iter()
                .find(|gateway| gateway.name == key.gateway)
                .is_some_and(GatewayInfo::has_certificates_of_its_own);
        if own {
            key.name.clone()
        } else {
            format!("{}/{}", key.gateway, key.name)
        }
    }

    /// Takes in the gateways OpenShell lists. The one shown stays shown while it's there; otherwise the active one is,
    /// which is what's shown at first. A listing that failed leaves the last one, and is logged once.
    pub(super) fn gateways_listed(&mut self, listed: Result<Vec<GatewayInfo>, OpenShellError>) {
        match listed {
            Ok(gateways) => {
                let there = self
                    .gateway
                    .name()
                    .is_some_and(|name| gateways.iter().any(|gateway| gateway.name == name));
                if !there && let Some(first) = gateways.first() {
                    self.gateway = first.gateway();
                    // It's the one in front of the person, so its sandboxes are shown.
                    self.expanded.insert(first.name.clone());
                }
                // What was kept for a gateway that's gone goes with it, so one given its name later starts afresh.
                let registered = |key: &String| gateways.iter().any(|gateway| &gateway.name == key);
                self.lists
                    .retain(|key, _| key.is_empty() || registered(key));
                self.expanded.retain(registered);
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|key| !key.gateway.is_empty() && !registered(&key.gateway))
                {
                    self.selected = None;
                }
                self.gateways = gateways;
            }
            Err(error) => {
                if self.gateways.is_empty() {
                    tracing::warn!("the gateways couldn't be listed: {error}");
                }
            }
        }
    }

    /// What's known of the gateway that's shown. Before the gateways are listed, or if they can't be, it's taken to be
    /// the one on this Mac, as every gateway was before there were several.
    pub(super) fn current_gateway(&self) -> GatewayInfo {
        self.gateway_info(&self.gateway_key())
    }

    /// What's known of the gateway `key` names, taken to be the one on this Mac when it isn't known.
    pub(super) fn gateway_info(&self, key: &str) -> GatewayInfo {
        self.gateways
            .iter()
            .find(|gateway| gateway.name == key)
            .cloned()
            .unwrap_or_else(|| {
                GatewayInfo::assumed_local(if key.is_empty() { "openshell" } else { key })
            })
    }

    /// Shows the gateway `step` along the list, wrapping round, with its sandboxes in place of the last one's.
    pub(super) fn switch_gateway(
        &mut self,
        step: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.gateways.len() < 2 {
            return;
        }
        let at = self
            .gateway
            .name()
            .and_then(|name| {
                self.gateways
                    .iter()
                    .position(|gateway| gateway.name == name)
            })
            .unwrap_or(0);
        let next = (at as isize + step).rem_euclid(self.gateways.len() as isize) as usize;
        let name = self.gateways[next].name.clone();
        self.show_gateway(&name, window, cx);
    }

    /// What clicking a gateway does: another gateway is brought into focus and opened; the one in focus is opened or
    /// closed, which shows its sandboxes under it or hides them.
    pub(super) fn click_gateway(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.gateway.name() == Some(name) {
            if !self.expanded.remove(name) {
                self.expanded.insert(name.to_string());
            }
            cx.notify();
        } else {
            self.show_gateway(name, window, cx);
        }
    }

    /// Brings the gateway `name` into focus and shows its sandboxes under it, choosing the first of them if they're
    /// known. Nothing happens if it isn't one that's registered, or if it's in focus already.
    pub(super) fn show_gateway(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gateway) = self.gateways.iter().find(|gateway| gateway.name == name) else {
            return;
        };
        if self.gateway.name() == Some(name) {
            return;
        }
        self.gateway = gateway.gateway();
        self.expanded.insert(name.to_string());
        cx.set_global(ShownGateway(self.gateway.clone()));
        self.selected = match self.listing_of(name) {
            Listing::Listed(sandboxes) => sandboxes.first().map(|sandbox| SandboxKey {
                gateway: name.to_string(),
                name: sandbox.name.clone(),
            }),
            Listing::Loading | Listing::Failed(_) => None,
        };
        self.making = None;
        self.adding = None;
        self.imaging = None;
        self.failed = None;
        tracing::info!(gateway = name, "showing a gateway");
        self.refresh(window, cx);
        cx.notify();
    }

    /// Lists the sandboxes again now.
    pub(super) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch == Watch::OpenShell {
            self._watching = Some(Self::watch_sandboxes(window, cx));
        }
    }

    /// Takes in what listing the gateway in focus found.
    pub(super) fn listed(
        &mut self,
        listed: Result<Vec<Sandbox>, OpenShellError>,
        cx: &mut Context<Self>,
    ) {
        let key = self.gateway_key();
        self.listed_for(&key, listed, cx);
    }

    /// Takes in what listing the gateway `key` names found, keeping the chosen sandbox chosen while it's there.
    pub(super) fn listed_for(
        &mut self,
        key: &str,
        listed: Result<Vec<Sandbox>, OpenShellError>,
        cx: &mut Context<Self>,
    ) {
        let list = match listed {
            Ok(mut sandboxes) => {
                // The running ones first, as the list shows them, so the arrows go down it in the order it's seen.
                sandboxes.sort_by_key(|sandbox| !is_running(&sandbox.phase));
                Listing::Listed(sandboxes)
            }
            Err(error) => {
                let error = error.to_string();
                // Logged once, not every few seconds, since it's asked again every few seconds.
                if !matches!(self.lists.get(key), Some(Listing::Failed(before)) if *before == error)
                {
                    tracing::warn!(gateway = key, "the sandboxes couldn't be listed: {error}");
                }
                Listing::Failed(error)
            }
        };
        self.lists.insert(key.to_string(), list);
        let chosen = self.selected.clone();
        self.keep_a_sandbox_chosen(key);
        // What's attached to the chosen sandbox is looked up again when another is chosen.
        if self.selected != chosen {
            self.look_up_services(cx);
        }
        cx.notify();
    }

    /// The sandboxes of the gateway in focus, when they're known.
    pub(super) fn listed_sandboxes(&self) -> &[Sandbox] {
        self.sandboxes_of(&self.gateway_key())
    }

    /// Chooses the first sandbox of the gateway `key` names when the one chosen there is gone, or when none is chosen
    /// and it's the gateway in focus. A list that couldn't be had changes nothing.
    fn keep_a_sandbox_chosen(&mut self, key: &str) {
        let Some(Listing::Listed(sandboxes)) = self.lists.get(key) else {
            return;
        };
        let chosen_here = self
            .selected
            .as_ref()
            .filter(|chosen| chosen.gateway == key);
        if chosen_here
            .is_some_and(|chosen| sandboxes.iter().any(|sandbox| sandbox.name == chosen.name))
        {
            return;
        }
        if chosen_here.is_some() || (self.selected.is_none() && key == self.gateway_key()) {
            self.selected = sandboxes.first().map(|sandbox| SandboxKey {
                gateway: key.to_string(),
                name: sandbox.name.clone(),
            });
        }
    }

    /// The sandboxes of the gateway `key` names, when they're known.
    fn sandboxes_of(&self, key: &str) -> &[Sandbox] {
        match self.listing_of(key) {
            Listing::Listed(sandboxes) => sandboxes,
            Listing::Loading | Listing::Failed(_) => &[],
        }
    }

    pub(super) fn selected_sandbox(&self) -> Option<&Sandbox> {
        let key = self.selected.as_ref()?;
        self.sandboxes_of(&key.gateway)
            .iter()
            .find(|sandbox| sandbox.name == key.name)
    }

    /// The sandboxes the sidebar shows, in the order it shows them: each open gateway's, running ones first.
    pub(super) fn visible_sandboxes(&self) -> Vec<SandboxKey> {
        self.gateway_keys()
            .into_iter()
            .filter(|key| self.is_open(key))
            .flat_map(|key| {
                self.sandboxes_of(&key)
                    .iter()
                    .map(|sandbox| SandboxKey {
                        gateway: key.clone(),
                        name: sandbox.name.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Brings the gateway `name` into focus, without changing what's chosen: where a sandbox is chosen is where Providers,
    /// New Agent… and the Gateway menu's Commands are.
    fn focus_gateway(&mut self, name: &str, cx: &mut Context<Self>) {
        if name.is_empty()
            || self.gateway.name() == Some(name)
            || !self.gateways.iter().any(|gateway| gateway.name == name)
        {
            return;
        }
        self.gateway = Gateway::named(name);
        cx.set_global(ShownGateway(self.gateway.clone()));
    }

    pub(super) fn select(&mut self, key: SandboxKey, cx: &mut Context<Self>) {
        self.focus_gateway(&key.gateway, cx);
        let changed = self.selected.as_ref() != Some(&key);
        self.selected = Some(key);
        self.making = None;
        self.adding = None;
        self.imaging = None;
        if changed {
            self.look_up_services(cx);
        }
        cx.notify();
    }

    /// Chooses the sandbox `step` places down the sidebar from the chosen one, or up for a negative step.
    pub(super) fn select_next(&mut self, step: isize, cx: &mut Context<Self>) {
        let keys = self.visible_sandboxes();
        if keys.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|chosen| keys.iter().position(|key| key == chosen));
        let next = match at {
            Some(at) => at.saturating_add_signed(step).min(keys.len() - 1),
            None => 0,
        };
        self.select(keys[next].clone(), cx);
    }

    pub(super) fn act_on_selected(
        &mut self,
        act: Act,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(sandbox), Some(key)) = (self.selected_sandbox(), self.selected.clone()) else {
            return;
        };
        let can = match act {
            Act::Start => sandbox.phase.can_start(),
            Act::Restart => sandbox.phase.needs_restart(),
            Act::Stop => sandbox.phase.can_stop(),
            Act::Delete => true,
        };
        if can {
            self.act(key, act, window, cx);
        }
    }

    /// Asks before deleting the chosen sandbox, whose files go with it.
    pub(super) fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(_), Some(key)) = (self.selected_sandbox(), self.selected.clone()) else {
            return;
        };
        let name = key.name.clone();
        let delete = t!("Delete");
        let answer = window.prompt(
            PromptLevel::Warning,
            &t!("Delete “%{name}”?", name = name),
            Some(&t!(
                "Its files go with it, and anything its agent made that you haven't copied out."
            )),
            &[&*delete, &*t!("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.act(key, Act::Delete, window, cx);
                });
            }
        })
        .detach();
    }

    /// Starts, stops or deletes the sandbox `key` names, off the window's thread, on its own gateway, and lists the
    /// sandboxes again after.
    fn act(&mut self, key: SandboxKey, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        if self.working.contains_key(&key) {
            return;
        }
        let openshell = match self.openshell_for(&key.gateway) {
            Ok(openshell) => openshell,
            Err(error) => {
                self.failed = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.failed = None;
        tracing::info!(sandbox = %key.name, gateway = %key.gateway, ?act, "acting on a sandbox");
        // Its folders, to forget what each repository gave it once it's gone; its branches stay, as the work.
        let folders: Vec<String> = self
            .sandboxes_of(&key.gateway)
            .iter()
            .find(|sandbox| sandbox.name == key.name)
            .map(|sandbox| sandbox.folders.clone())
            .unwrap_or_default();
        let doing = key.name.clone();
        let done = cx.background_spawn(async move {
            match act {
                Act::Start => openshell.start(&doing),
                Act::Restart => openshell.restart(&doing),
                Act::Stop => openshell.stop(&doing),
                Act::Delete => openshell.delete(&doing),
            }
        });
        let task = cx.spawn_in(window, {
            let key = key.clone();
            async move |this, cx| {
                let done = done.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.working.remove(&key);
                    match done {
                        Ok(()) => {
                            tracing::info!(sandbox = %key.name, ?act, "done");
                            if act == Act::Delete {
                                if let Some(added) = &this.added {
                                    added.forget(&key.name);
                                }
                                Self::forget_seeds(key.name.clone(), folders.clone(), cx);
                            }
                        }
                        Err(error) => {
                            tracing::warn!(sandbox = %key.name, ?act, "failed: {error}");
                            this.failed = Some(error.to_string());
                        }
                    }
                    this.refresh(window, cx);
                    cx.notify();
                });
            }
        });
        self.working.insert(key, task);
        cx.notify();
    }

    /// Asks for folders to copy into the chosen sandbox while it runs, then copies them in beside the others.
    pub(super) fn choose_folders_to_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sandbox) = self.selected_sandbox() else {
            return;
        };
        let Some(key) = self.selected.clone() else {
            return;
        };
        if !sandbox.phase.is_ready() || self.working.contains_key(&key) {
            return;
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(t!("Add").into_owned().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(folders))) = chosen.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.add_folders(key, folders, window, cx);
            });
        })
        .detach();
    }

    /// Copies `folders` into the sandbox `key` names, off the window's thread, on its own gateway, and lists the
    /// sandboxes again after.
    pub(super) fn add_folders(
        &mut self,
        key: SandboxKey,
        folders: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = key.name.clone();
        let Some(sandbox) = self
            .sandboxes_of(&key.gateway)
            .iter()
            .find(|sandbox| sandbox.name == name)
            .cloned()
        else {
            return;
        };
        if folders.is_empty() || self.working.contains_key(&key) {
            return;
        }
        let openshell = match self.openshell_for(&key.gateway) {
            Ok(openshell) => openshell,
            Err(error) => {
                self.failed = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let Some(added) = self.added.clone() else {
            return;
        };
        self.failed = None;
        tracing::info!(sandbox = %name, count = folders.len(), "adding folders");
        let done = cx.background_spawn(async move { added.add(&openshell, &sandbox, &folders) });
        let task = cx.spawn_in(window, {
            let name = name.clone();
            let key = key.clone();
            async move |this, cx| {
                let done = done.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.working.remove(&key);
                    if let Err(error) = done {
                        tracing::warn!(sandbox = %name, "couldn't add folders: {error}");
                        this.failed = Some(error.to_string());
                    }
                    this.refresh(window, cx);
                    cx.notify();
                });
            }
        });
        self.working.insert(key, task);
        cx.notify();
    }

    /// Opens a session running `what` in the chosen sandbox, shown in `place`, or where the settings say. herdr-core
    /// keeps it when it's installed and the settings ask; one it keeps that's open already is shown again.
    pub(super) fn open_session(
        &mut self,
        place: Option<Place>,
        what: What,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(sandbox), Some(key)) = (self.selected_sandbox().cloned(), self.selected.clone())
        else {
            return;
        };
        if !sandbox.phase.is_ready() || self.working.contains_key(&key) {
            return;
        }
        let label = self.label_of(&key);
        let openshell = match self.openshell_for(&key.gateway) {
            Ok(openshell) => openshell,
            Err(error) => {
                self.failed = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.failed = None;
        let settings = crate::settings::current(cx);
        let keep = if settings.keep_sessions {
            Keep::InHerdrCore
        } else {
            Keep::Nowhere
        };
        let name = sandbox.name.clone();
        tracing::info!(sandbox = %name, ?what, ?place, ?keep, "opening a session");
        let opened = cx.background_spawn(async move {
            let place = place.unwrap_or_else(|| settings.show_in.place(&Installed::now()));
            session::open(&openshell, &sandbox, &label, what, place, keep)
        });
        let task = cx.spawn_in(window, {
            let name = name.clone();
            let key = key.clone();
            async move |this, cx| {
                let opened = opened.await;
                let _ = this.update(cx, |this, cx| {
                    this.working.remove(&key);
                    match &opened {
                        Ok(Opened::AsAsked) => tracing::info!(sandbox = %name, "opened a session"),
                        Ok(Opened::InTerminalInstead(place)) => {
                            tracing::info!(sandbox = %name, ?place, "not installed, so opened in Terminal");
                        }
                        Err(error) => tracing::warn!(sandbox = %name, "couldn't open a session: {error}"),
                    }
                    this.failed = match opened {
                        Ok(Opened::AsAsked) => None,
                        Ok(Opened::InTerminalInstead(place)) => Some(
                            t!(
                                "%{place} isn't installed, so it opened in Terminal.",
                                place = place_name(place)
                            )
                            .into_owned(),
                        ),
                        Err(error) => Some(
                            t!(
                                "Couldn't open “%{name}”: %{error}",
                                name = name,
                                error = error.to_string()
                            )
                            .into_owned(),
                        ),
                    };
                    cx.notify();
                });
            }
        });
        self.working.insert(key, task);
        cx.notify();
    }

    /// What's known of the sandboxes of the gateway in focus, in a few words: whether it's answering, and how many are
    /// running.
    fn sandboxes_said(&self) -> String {
        self.said_of(&self.gateway_key())
    }

    /// What's known of the sandboxes of the gateway `key` names, in a few words.
    fn said_of(&self, key: &str) -> String {
        match self.listing_of(key) {
            Listing::Loading => t!("Asking OpenShell…").into_owned(),
            Listing::Failed(_) if self.gateway_info(key).has_certificates_of_its_own() => {
                t!("OpenShell isn't answering").into_owned()
            }
            Listing::Failed(_) => t!("Not answering").into_owned(),
            Listing::Listed(sandboxes) => {
                let running = sandboxes
                    .iter()
                    .filter(|sandbox| sandbox.phase.can_stop())
                    .count();
                match (sandboxes.len(), running) {
                    (0, _) => t!("No sandboxes").into_owned(),
                    (_, 0) => t!("None running").into_owned(),
                    (_, running) => t!("%{count} running", count = running).into_owned(),
                }
            }
        }
    }

    /// The list's heading: how many are running.
    pub(super) fn list_heading(&self, cx: &Context<Self>) -> Div {
        let said = self.sandboxes_said();
        heading(t!("Sandboxes").into_owned(), said, cx).child(
            Button::new("new-agent")
                .ghost()
                .small()
                .rounded(px(999.))
                .icon(assets::IconName::Plus)
                .tooltip(t!("New Agent…").into_owned())
                .on_click(cx.listener(|this, _, window, cx| this.run("sandbox.new", window, cx))),
        )
    }

    /// The sidebar: the gateways, each open one with its sandboxes under it, the running ones first and then the rest
    /// under a heading of their own. Before the gateways are listed there's only the active one, and its sandboxes are
    /// listed on their own.
    pub(super) fn sandbox_list(&self, cx: &Context<Self>) -> Div {
        let mut footer = Footer::new()
            .busy(!self.working.is_empty() || matches!(self.listing(), Listing::Loading));
        match self.listing() {
            Listing::Loading => footer = footer.line(t!("Asking OpenShell…").into_owned()),
            Listing::Failed(error) => footer = footer.error(error.clone()),
            Listing::Listed(_) => {}
        }
        if let Some(said) = self.gateway_work_said() {
            footer = footer.busy(true).line(said.to_string());
        }
        if let Some(failed) = &self.failed {
            footer = footer.error(failed.clone());
        }
        let nested = !self.gateways.is_empty();
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .id("sandboxes")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pt_1()
                    .gap(px(2.))
                    // Always once they're listed, since it's where one is added.
                    .when(nested, |list| {
                        list.child(self.gateway_heading(cx)).children(
                            self.gateways
                                .iter()
                                .flat_map(|gateway| {
                                    let mut rows =
                                        vec![self.gateway_row(gateway, cx).into_any_element()];
                                    if self.is_open(&gateway.name) {
                                        rows.push(self.sandboxes_under(&gateway.name, cx));
                                    }
                                    rows
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .when(!nested, |list| list.children(self.sandbox_rows("", cx))),
            )
            .when(!footer.is_empty(), |list| list.child(footer))
    }

    /// The heading above the gateways, with the button that adds one.
    fn gateway_heading(&self, cx: &Context<Self>) -> Div {
        h_flex()
            .justify_between()
            .items_center()
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("Gateways").into_owned()),
            )
            .child(
                Button::new("add-gateway-row")
                    .ghost()
                    .xsmall()
                    .icon(assets::IconName::Plus)
                    .tooltip(t!("Add Gateway…").into_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.run("gateway.add", window, cx);
                    })),
            )
    }

    /// An open gateway's sandboxes, set in under it, or a line saying why there are none to show.
    fn sandboxes_under(&self, key: &str, cx: &Context<Self>) -> AnyElement {
        let quiet = |text: String| {
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text)
                .into_any_element()
        };
        let rows = match self.listing_of(key) {
            Listing::Loading => vec![quiet(t!("Asking…").into_owned())],
            // The gateway's row says it isn't answering.
            Listing::Failed(_) => Vec::new(),
            Listing::Listed(sandboxes) if sandboxes.is_empty() => {
                vec![quiet(t!("No sandboxes").into_owned())]
            }
            Listing::Listed(_) => self.sandbox_rows(key, cx),
        };
        v_flex()
            .pl_3()
            .gap(px(2.))
            .children(rows)
            .into_any_element()
    }

    /// The rows of the sandboxes of the gateway `key` names: the running ones, then the rest under a heading.
    fn sandbox_rows(&self, key: &str, cx: &Context<Self>) -> Vec<AnyElement> {
        let (running, resting): (Vec<_>, Vec<_>) = self
            .sandboxes_of(key)
            .iter()
            .partition(|sandbox| is_running(&sandbox.phase));
        let rows = |list: Vec<&Sandbox>| {
            list.into_iter()
                .map(|sandbox| self.sandbox_row(sandbox, key, cx).into_any_element())
                .collect::<Vec<_>>()
        };
        let mut all = rows(running);
        if !resting.is_empty() {
            all.push(
                div()
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("Stopped").into_owned())
                    .into_any_element(),
            );
            all.extend(rows(resting));
        }
        all
    }

    /// A gateway in the list: whether it's open, a dot for how it's answering, its name, and where it is or how many of
    /// its sandboxes are running. Only open gateways are asked, so one never opened says where it is and nothing more.
    fn gateway_row(&self, gateway: &GatewayInfo, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        // In focus, and not already marked by one of its sandboxes being the one chosen.
        let shown = self.gateway.name() == Some(gateway.name.as_str())
            && self
                .selected
                .as_ref()
                .is_none_or(|chosen| chosen.gateway != gateway.name);
        let open = self.is_open(&gateway.name);
        let (dot, said) = match self.lists.get(&gateway.name) {
            None => (theme.border, gateway_where(gateway)),
            Some(Listing::Loading) => (theme.muted_foreground, self.said_of(&gateway.name)),
            Some(Listing::Failed(_)) => (theme.danger, t!("Not answering").into_owned()),
            Some(Listing::Listed(_)) => (theme.success, self.said_of(&gateway.name)),
        };
        let name = gateway.name.clone();
        let hover = theme.list_hover;
        // The local gateway OpenShell runs isn't removed from here, and only one that signs in in a browser can.
        let removable = !gateway.has_certificates_of_its_own();
        let signs_in = !matches!(gateway.auth.as_str(), "mtls" | "plaintext" | "");
        let acts = h_flex()
            .flex_none()
            .gap_0p5()
            .when(signs_in, |acts| {
                acts.child(
                    Button::new(ElementId::from((
                        ElementId::from("gateway-sign-in"),
                        name.clone(),
                    )))
                    .ghost()
                    .xsmall()
                    .label(t!("Sign in").into_owned())
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, window, cx| {
                            this.sign_in_to_gateway(name.clone(), window, cx);
                        }
                    })),
                )
            })
            .when(removable, |acts| {
                acts.child(
                    Button::new(ElementId::from((
                        ElementId::from("gateway-remove"),
                        name.clone(),
                    )))
                    .ghost()
                    .xsmall()
                    .icon(assets::IconName::Trash)
                    .tooltip(t!("Remove…").into_owned())
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, window, cx| {
                            this.confirm_remove_gateway(name.clone(), window, cx);
                        }
                    })),
                )
            });
        h_flex()
            .id(ElementId::from((ElementId::from("gateway"), name.clone())))
            .flex_none()
            .gap_2p5()
            .px_2()
            .py_1p5()
            .rounded_lg()
            .cursor_pointer()
            .when(shown, |row| row.bg(theme.list_active))
            .when(!shown, |row| row.hover(move |row| row.bg(hover)))
            .child(
                div()
                    .flex_none()
                    .w(px(10.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if open { "▾" } else { "▸" }),
            )
            .child(div().flex_none().size(px(8.)).rounded_full().bg(dot))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .child(gateway.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(said),
                    ),
            )
            .child(acts)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.click_gateway(&name, window, cx);
                this.focus.focus(window, cx);
            }))
    }

    /// A sandbox in the list: its agent's mark in the colour of its state, its name, what it's doing, and a button
    /// each to start or stop it and to delete it.
    fn sandbox_row(&self, sandbox: &Sandbox, gateway: &str, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let key = SandboxKey {
            gateway: gateway.to_string(),
            name: sandbox.name.clone(),
        };
        let chosen = self.selected.as_ref() == Some(&key)
            && self.making.is_none()
            && self.adding.is_none()
            && self.imaging.is_none();
        let busy = self.working.contains_key(&key);
        let agent = sandbox.agent.as_deref().and_then(Agent::from_id);
        let state = self.agent_states.get(&self.label_of(&key)).copied();
        let resting = !is_running(&sandbox.phase);
        // Two gateways can each have a sandbox of one name, so the element's id has the gateway in it too.
        let id_name = format!("{gateway}/{}", sandbox.name);
        let id = |what: &str| {
            ElementId::from((
                ElementId::from(SharedString::from(what.to_string())),
                id_name.clone(),
            ))
        };
        let (act, icon, tip) = if sandbox.phase.needs_restart() {
            ("sandbox.restart", assets::IconName::RotateCw, t!("Restart"))
        } else if sandbox.phase.can_start() {
            ("sandbox.start", assets::IconName::CirclePlay, t!("Start"))
        } else {
            ("sandbox.stop", assets::IconName::CircleStop, t!("Stop"))
        };
        let acts = h_flex()
            .flex_none()
            .gap_0p5()
            .child(
                Button::new(id("row-act"))
                    .ghost()
                    .xsmall()
                    .icon(icon)
                    .tooltip(tip.into_owned())
                    .disabled(busy || (act == "sandbox.stop" && !sandbox.phase.can_stop()))
                    .on_click(cx.listener({
                        let key = key.clone();
                        move |this, _, window, cx| {
                            this.select(key.clone(), cx);
                            this.run(act, window, cx);
                        }
                    })),
            )
            .child(
                Button::new(id("row-delete"))
                    .ghost()
                    .xsmall()
                    .icon(assets::IconName::Trash)
                    .tooltip(t!("Delete…").into_owned())
                    .disabled(busy)
                    .on_click(cx.listener({
                        let key = key.clone();
                        move |this, _, window, cx| {
                            this.select(key.clone(), cx);
                            this.run("sandbox.delete", window, cx);
                        }
                    })),
            );
        let blocked = state == Some(AgentState::Blocked) && sandbox.phase.is_ready();
        let hover = theme.list_hover;
        h_flex()
            .id(id("sandbox"))
            .flex_none()
            .gap_2p5()
            .px_2()
            .py_1p5()
            .rounded_lg()
            .cursor_pointer()
            .when(chosen, |row| row.bg(theme.list_active))
            .when(!chosen, |row| row.hover(move |row| row.bg(hover)))
            .child(agent_tile(
                agent,
                state_colour(&sandbox.phase, state, cx),
                rems(28. / 13.),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .when(resting, |name| name.text_color(theme.muted_foreground))
                            .child(sandbox.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(if blocked {
                                theme.warning
                            } else {
                                theme.muted_foreground
                            })
                            .child(
                                t!(
                                    "%{agent} · %{state}",
                                    agent = agent_name(agent),
                                    state = state_words(&sandbox.phase, state)
                                )
                                .into_owned(),
                            ),
                    ),
            )
            .child(acts)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(key.clone(), cx);
                this.focus.focus(window, cx);
            }))
    }

    pub(super) fn sandbox_detail(&self, cx: &Context<Self>) -> AnyElement {
        let (Some(sandbox), Some(key)) = (self.selected_sandbox(), self.selected.as_ref()) else {
            return self.nothing_chosen(cx).into_any_element();
        };
        let theme = cx.theme();
        let busy = self.working.contains_key(key);
        let agent = sandbox.agent.as_deref().and_then(Agent::from_id);
        let state = self.agent_states.get(&self.label_of(key)).copied();
        let hero = h_flex()
            .gap_3()
            .child(agent_tile(
                agent,
                state_colour(&sandbox.phase, state, cx),
                rems(44. / 13.),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_lg()
                            .font_semibold()
                            .child(sandbox.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(agent_name(agent)),
                    ),
            );
        // They wrap rather than run out of a narrow window.
        let actions = h_flex()
            .flex_wrap()
            .gap_2()
            .child(open_button(
                agent,
                busy || !sandbox.phase.is_ready(),
                Installed::now(),
                cx,
            ))
            .child(if sandbox.phase.needs_restart() {
                Button::new("restart")
                    .small()
                    .disabled(busy)
                    .icon(assets::IconName::RotateCw)
                    .label(t!("Restart").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.restart", window, cx)),
                    )
            } else if sandbox.phase.can_start() {
                Button::new("start")
                    .small()
                    .disabled(busy)
                    .icon(assets::IconName::CirclePlay)
                    .label(t!("Start").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.start", window, cx)),
                    )
            } else {
                Button::new("stop")
                    .small()
                    .disabled(busy || !sandbox.phase.can_stop())
                    .icon(assets::IconName::CircleStop)
                    .label(t!("Stop").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.stop", window, cx)),
                    )
            })
            .child(self.bring_back_button(sandbox, cx))
            .child(
                Button::new("add-folders")
                    .small()
                    .ghost()
                    .disabled(busy || !sandbox.phase.is_ready() || self.added.is_none())
                    .icon(assets::IconName::FolderPlus)
                    .label(t!("Add Folders…").into_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.run("sandbox.add-folders", window, cx);
                    })),
            )
            .child(
                Button::new("delete")
                    .small()
                    .ghost()
                    .disabled(busy)
                    .icon(assets::IconName::Trash)
                    .label(t!("Delete…").into_owned())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run("sandbox.delete", window, cx)),
                    ),
            );
        let mut info = vec![
            (t!("Agent").into_owned(), text_value(agent_name(agent))),
            (
                t!("Status").into_owned(),
                text_value(state_words(&sandbox.phase, state)),
            ),
        ];
        // Which gateway it's on, once there's another it might be on.
        if self.gateways.len() > 1 && !key.gateway.is_empty() {
            info.push((t!("Gateway").into_owned(), text_value(key.gateway.clone())));
        }
        if !sandbox.folders.is_empty() {
            info.push((
                t!("Copied from").into_owned(),
                v_flex().min_w_0().gap_1().items_end().children(
                    sandbox
                        .folders
                        .iter()
                        .map(|folder| shell::code_chip(folder.clone(), cx)),
                ),
            ));
        }
        if let Some(workdir) = sandbox.workdir.clone() {
            info.push((t!("Works in").into_owned(), shell::code_chip(workdir, cx)));
        }
        info.push((t!("Services").into_owned(), self.services_row(sandbox, cx)));
        info.push((
            t!("Brings back to").into_owned(),
            self.bring_back_row(sandbox, cx),
        ));
        info.push((
            t!("Made").into_owned(),
            text_value(t!("%{when} UTC", when = sandbox.created.clone()).into_owned()),
        ));
        let body = v_flex()
            .gap_5()
            .p_5()
            .child(hero)
            .child(actions)
            .child(info_card(info, cx))
            .child(hint(
                t!(
                    "Its agent reaches its model, and the services attached to it, and nothing else. Stopping keeps its files; deleting doesn't."
                ),
                cx,
            ));
        div()
            .id("sandbox")
            .size_full()
            .overflow_y_scroll()
            .child(body)
            .into_any_element()
    }

    /// The detail panel with no sandbox to show: why, and how to make one.
    fn nothing_chosen(&self, cx: &Context<Self>) -> Div {
        let (title, said) = match self.listing() {
            Listing::Loading => (t!("Sandboxes"), t!("Asking OpenShell for its sandboxes…")),
            Listing::Failed(_) if self.current_gateway().has_certificates_of_its_own() => (
                t!("OpenShell isn't answering"),
                t!(
                    "Is its gateway running? `brew services start openshell` starts it. Playpen asks again every few seconds."
                ),
            ),
            // Any other gateway is somewhere else, so what to try is its address and the network to it.
            Listing::Failed(_) => (
                t!(
                    "“%{gateway}” isn't answering",
                    gateway = self.gateway.name().unwrap_or("")
                ),
                t!(
                    "Is it running, and can this Mac reach its address? A VPN or a tunnel may be down, or a sign-in may have run out. Playpen asks again every few seconds."
                ),
            ),
            Listing::Listed(_) => (
                t!("No sandboxes yet"),
                t!(
                    "Make one for an agent with New Agent…: it runs in its own sandbox, on copies of folders, and reaches only its model and the services you attach."
                ),
            ),
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_6()
            .child(div().text_lg().font_semibold().child(title.into_owned()))
            .child(
                div()
                    .max_w(rems(28.))
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(said.into_owned()),
            )
            .when(matches!(self.listing(), Listing::Listed(_)), |this| {
                this.child(
                    Button::new("new-agent-empty")
                        .primary()
                        .small()
                        .label(t!("New Agent…").into_owned())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.run("sandbox.new", window, cx)),
                        ),
                )
            })
    }
}

/// Sandboxes for a capture to show, of every agent and in each state the list sets apart.
pub(super) fn stand_ins() -> Vec<Sandbox> {
    let sandbox = |name: &str, phase: Phase, agent: Option<&str>| Sandbox {
        name: name.to_string(),
        phase,
        created: "2026-11-21 09:41:00".to_string(),
        agent: agent.map(str::to_string),
        folders: agent
            .map(|_| vec!["/Users/sam/src/Patchbay".to_string()])
            .unwrap_or_default(),
        workdir: agent.map(|_| "/sandbox".to_string()),
    };
    vec![
        sandbox("claude-patchbay", Phase::Ready, Some("claude-code")),
        sandbox("codex-patchbay", Phase::Provisioning, Some("codex")),
        sandbox("scratch", Phase::Error, None),
        sandbox("claude-typecase", Phase::Stopped, Some("claude-code")),
        sandbox("codex-red-pen", Phase::Stopped, Some("codex")),
    ]
}

/// Whether a sandbox is up, or on its way up or down, rather than resting: the list shows these first.
pub(super) fn is_running(phase: &Phase) -> bool {
    !matches!(phase, Phase::Stopped | Phase::Suspended | Phase::Completed)
}

/// An agent's name, or what a sandbox Playpen didn't make offers instead.
fn agent_name(agent: Option<Agent>) -> String {
    match agent {
        Some(agent) => agent.name().to_string(),
        None => t!("Shell").into_owned(),
    }
}

/// The mark for an agent, or a plain box for a sandbox Playpen didn't make.
fn agent_icon(agent: Option<Agent>) -> assets::IconName {
    match agent {
        Some(Agent::ClaudeCode) => assets::IconName::Asterisk,
        Some(Agent::Codex) => assets::IconName::SquareTerminal,
        None => assets::IconName::Box,
    }
}

/// A rounded square holding an agent's mark, tinted with the colour of its sandbox's state, so the list says at a
/// glance which are running, which want the person, and which failed.
pub(super) fn agent_tile(agent: Option<Agent>, colour: Hsla, size: Rems, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_none()
        .size(size)
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius * 1.5)
        .bg(colour.opacity(0.16))
        .border_1()
        .border_color(colour.opacity(0.3))
        .text_color(colour)
        .child(Icon::new(agent_icon(agent)).size(size * 0.55))
}

/// The colour of a sandbox's state: what its agent's doing while it runs, or its phase.
fn state_colour(phase: &Phase, agent: Option<AgentState>, cx: &App) -> Hsla {
    let theme = cx.theme();
    match (phase, agent) {
        (Phase::Ready, Some(AgentState::Working)) => theme.info,
        (Phase::Ready, Some(AgentState::Blocked)) => theme.warning,
        (Phase::Ready, _) => theme.success,
        (Phase::Provisioning, _) => theme.info,
        (Phase::Deleting, _) => theme.warning,
        (Phase::Error, _) => theme.danger,
        (Phase::Stopped | Phase::Suspended | Phase::Completed | Phase::Other(_), _) => {
            theme.muted_foreground
        }
    }
}

/// Where a gateway is, in a few words: this Mac, or the host of its endpoint.
pub(super) fn gateway_where(gateway: &GatewayInfo) -> String {
    if gateway.is_local() {
        return t!("This Mac").into_owned();
    }
    let host = gateway
        .endpoint
        .split_once("://")
        .map_or(gateway.endpoint.as_str(), |(_, rest)| rest);
    host.split('/').next().unwrap_or(host).to_string()
}

/// What a sandbox is doing, in words: its agent's state while it runs, or its phase.
fn state_words(phase: &Phase, agent: Option<AgentState>) -> String {
    match (phase, agent) {
        (Phase::Ready, Some(state)) => match state {
            AgentState::Working => t!("Working"),
            AgentState::Blocked => t!("Waiting for you"),
            AgentState::Done => t!("Done"),
            AgentState::Idle => t!("Idle"),
        }
        .into_owned(),
        _ => phase_words(phase),
    }
}

/// A value in an info card, in plain text.
pub(super) fn text_value(text: impl Into<SharedString>) -> Div {
    div().min_w_0().truncate().child(text.into())
}

/// Facts about something, in a rounded card set back from the panel: a row each, its name on the left and its value
/// on the right, with a hairline between them.
pub(super) fn info_card(rows: Vec<(String, Div)>, cx: &App) -> Div {
    let theme = cx.theme();
    let count = rows.len();
    v_flex()
        .rounded(theme.radius * 1.5)
        .border_1()
        .border_color(theme.border)
        .bg(shell::recessed_colour(theme))
        .text_sm()
        .children(rows.into_iter().enumerate().map(|(at, (name, value))| {
            h_flex()
                .gap_4()
                .justify_between()
                .px_3()
                .py_2()
                .when(at + 1 < count, |row| {
                    row.border_b_1().border_color(shell::hairline(theme))
                })
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(name),
                )
                .child(h_flex().min_w_0().justify_end().child(value))
        }))
}

/// A phase in words.
fn phase_words(phase: &Phase) -> String {
    match phase {
        Phase::Ready => t!("Ready").into_owned(),
        Phase::Provisioning => t!("Starting").into_owned(),
        Phase::Stopped => t!("Stopped").into_owned(),
        Phase::Suspended => t!("Suspended").into_owned(),
        Phase::Completed => t!("Finished").into_owned(),
        Phase::Deleting => t!("Deleting").into_owned(),
        Phase::Error => t!("Failed").into_owned(),
        Phase::Other(name) => name.clone(),
    }
}

/// A place's name, as the menus give it.
fn place_name(place: Place) -> String {
    match place {
        Place::HerdrApp => t!("The Herdr app").into_owned(),
        Place::Ghostty => t!("Ghostty").into_owned(),
        Place::Terminal => t!("Terminal").into_owned(),
    }
}

/// A menu item's action: the Command `name`.
fn run(name: &'static str) -> Box<dyn Action> {
    Box::new(RunAction { name })
}

/// Open, which opens a session where the settings say, with the rest of the choices for one session under its arrow.
/// A place that isn't installed is there, but can't be chosen.
fn open_button(
    agent: Option<Agent>,
    disabled: bool,
    installed: Installed,
    cx: &Context<MainView>,
) -> DropdownButton {
    let label = match agent {
        Some(agent) => t!("Open %{agent}", agent = agent.name()).into_owned(),
        None => t!("Open Shell").into_owned(),
    };
    DropdownButton::new("open")
        .primary()
        .small()
        .disabled(disabled)
        .button(
            Button::new("open-session")
                .label(label)
                .on_click(cx.listener(|this, _, window, cx| this.run("sandbox.open", window, cx))),
        )
        .dropdown_menu(move |menu, _, _| {
            let menu = if agent.is_some() {
                menu.menu(
                    t!("Continue Last Conversation").into_owned(),
                    run("sandbox.continue"),
                )
                .menu(t!("Open Shell").into_owned(), run("sandbox.shell"))
                .separator()
            } else {
                menu
            };
            menu.menu_with_disabled(
                t!("Open in Herdr App").into_owned(),
                run("sandbox.open-herdr"),
                !installed.herdr_app,
            )
            .menu_with_disabled(
                t!("Open in Ghostty").into_owned(),
                run("sandbox.open-ghostty"),
                !installed.ghostty,
            )
            .menu(
                t!("Open in Terminal").into_owned(),
                run("sandbox.open-terminal"),
            )
        })
}
