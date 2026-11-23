//! The guide: a welcome at the first launch offering a tour of the window or setting up, the tour, and setting up,
//! which walks through choosing a model and signing in to it, a gateway, a kit, a sandbox and opening its agent. Each
//! is a Typecase tour over the window, with cards of Playpen's own that can offer choices. Setting up points at the real
//! forms and goes on when the person has done what a step asks, however they did it.

use std::time::Duration;

use gpui_kit::base::Placement;
use typecase::tour::{Advance, Card, Step, Tour, TourEvent};

use playpen_core::{Harness, Kind, MICROVM_GATEWAY, Provider};

use super::*;

/// Where OpenShell's own guides are, which the last step offers.
const OPENSHELL_DOCS: &str = "https://docs.nvidia.com/openshell/";
const OPENSHELL_REPOSITORY: &str = "https://github.com/NVIDIA/OpenShell";

/// Which of the guide's tours runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Welcome,
    Tour,
    /// Setting up, as far as choosing the gateway, after which the rest points at what that gateway needs.
    Setup,
    SetupRest,
}

/// A step of the guide: what its card says and offers, and what the window does when it's shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GuideStep {
    Welcome,
    Places,
    Gateways,
    Detail,
    NewAgent,
    Kits,
    Providers,
    Palette,
    TourEnd,
    ChooseAgent,
    ChooseGateway,
    Gateway,
    ChooseKey,
    SignIn,
    NewKit,
    KitKey,
    MakeSandbox,
    OpenIt,
    LearnMore,
}

/// Where the person chose their sandboxes run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
    /// OpenShell's own gateway, on this Mac's Docker, which is there once OpenShell runs.
    Docker,
    MicroVm,
    Another,
}

/// A choice a card offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pick {
    Tour,
    Setup,
    Agent(Agent),
    /// One of the person's own agents, by where it is in their list.
    Custom(usize),
    Gateway(Place),
    /// A key the gateway holds already, by where it is in the keys listed.
    Key(usize),
    /// A new key of this kind, signed in to or pasted.
    NewKey(Kind),
    Docs,
    Repository,
    Finish,
}

/// The guide while it runs.
pub(super) struct Guide {
    pub(super) tour: Entity<Tour>,
    stage: Stage,
    steps: Vec<GuideStep>,
    /// The step shown, from 0.
    at: Option<usize>,
    /// Whether the step shown was come back to, with Back, so what it waits on may be done already; it then waits for
    /// Next rather than going on by itself.
    came_back: bool,
    /// How many steps of setting up came before these, and how many there are in all, which the cards count by.
    before: usize,
    all: usize,
    /// Whether the tour has stepped aside for a step that waits on the person, which a card in the window's corner
    /// says instead, leaving the window theirs to use.
    parked: bool,
    agent: Option<Harness>,
    place: Option<Place>,
    /// The keys the chosen gateway holds, once listed.
    keys: Option<Result<Vec<Provider>, String>>,
    /// The key chosen or added, by its provider's name, and its kind.
    key: Option<String>,
    kind: Option<Kind>,
    /// The kit made while setting up, by name.
    kit: Option<String>,
    /// Whether the sandbox was made.
    made: bool,
    _events: Subscription,
    _listing: Option<Task<()>>,
    /// Looking every so often for what a waiting step waits on, such as a gateway answering.
    _watching: Task<()>,
}

impl Guide {
    /// Whether the tour shows a card, which holds the keyboard; not while it's stepped aside for a step that waits.
    pub(super) fn tour_shows_a_card(&self) -> bool {
        !self.parked
    }

    fn shown(&self) -> Option<GuideStep> {
        self.steps.get(self.at?).copied()
    }
}

impl Stage {
    fn steps(self) -> Vec<GuideStep> {
        match self {
            Stage::Welcome => vec![GuideStep::Welcome],
            Stage::Tour => vec![
                GuideStep::Places,
                GuideStep::Gateways,
                GuideStep::Detail,
                GuideStep::NewAgent,
                GuideStep::Kits,
                GuideStep::Providers,
                GuideStep::Palette,
                GuideStep::TourEnd,
            ],
            Stage::Setup => vec![GuideStep::ChooseAgent, GuideStep::ChooseGateway],
            Stage::SetupRest => vec![
                GuideStep::Gateway,
                GuideStep::ChooseKey,
                GuideStep::SignIn,
                GuideStep::NewKit,
                GuideStep::KitKey,
                GuideStep::MakeSandbox,
                GuideStep::OpenIt,
                GuideStep::LearnMore,
            ],
        }
    }
}

impl GuideStep {
    fn title(self) -> String {
        match self {
            GuideStep::Welcome => t!("Welcome to Playpen"),
            GuideStep::Places => t!("Places"),
            GuideStep::Gateways => t!("Gateways and their sandboxes"),
            GuideStep::Detail => t!("What's chosen"),
            GuideStep::NewAgent => t!("A new agent"),
            GuideStep::Kits => t!("Kits"),
            GuideStep::Providers => t!("Providers"),
            GuideStep::Palette => t!("Every command"),
            GuideStep::TourEnd => t!("That's the window"),
            GuideStep::ChooseAgent => t!("Which agent?"),
            GuideStep::ChooseGateway => t!("Where do sandboxes run?"),
            GuideStep::Gateway => t!("Your gateway"),
            GuideStep::ChooseKey => t!("Its key"),
            GuideStep::SignIn => t!("Sign in"),
            GuideStep::NewKit => t!("Make a kit"),
            GuideStep::KitKey => t!("Give it your key"),
            GuideStep::MakeSandbox => t!("Make a sandbox"),
            GuideStep::OpenIt => t!("Run it"),
            GuideStep::LearnMore => t!("You're set up"),
        }
        .into_owned()
    }

    /// What it points at, by the name the window marks it with; none for a card in the middle.
    fn target(self) -> Option<&'static str> {
        match self {
            GuideStep::Places => Some("places"),
            GuideStep::Gateways | GuideStep::Kits => Some("list"),
            GuideStep::Detail | GuideStep::Providers => Some("detail"),
            GuideStep::NewAgent => Some("new-agent"),
            GuideStep::NewKit => Some("new-kit"),
            GuideStep::Welcome
            | GuideStep::Palette
            | GuideStep::TourEnd
            | GuideStep::ChooseAgent
            | GuideStep::ChooseGateway
            | GuideStep::ChooseKey
            | GuideStep::LearnMore => None,
            // Said in the window's corner instead.
            GuideStep::Gateway
            | GuideStep::SignIn
            | GuideStep::KitKey
            | GuideStep::MakeSandbox
            | GuideStep::OpenIt => None,
        }
    }

    /// Whether it waits for a choice from its card, rather than going on with Next.
    fn chooses(self) -> bool {
        matches!(
            self,
            GuideStep::Welcome
                | GuideStep::TourEnd
                | GuideStep::ChooseAgent
                | GuideStep::ChooseGateway
                | GuideStep::ChooseKey
        )
    }

    /// Whether it waits for the person to do something in the window.
    fn waits(self) -> bool {
        matches!(
            self,
            GuideStep::SignIn
                | GuideStep::Gateway
                | GuideStep::KitKey
                | GuideStep::MakeSandbox
                | GuideStep::OpenIt
        )
    }

    fn step(self) -> Step {
        let target = self.target();
        let step = Step::new(self.title(), String::new());
        let step = match target {
            Some(target) => step.on(target),
            None => step,
        };
        let step = match target {
            Some("places" | "list") => step.placement(Placement::Right),
            Some("new-agent" | "new-kit") => step.placement(Placement::Bottom),
            Some(_) => step.placement(Placement::Left),
            None => step,
        };
        if self == GuideStep::NewKit {
            step.advance_on_click()
        } else if self.chooses() {
            step.until("chosen")
        } else if self.waits() {
            step.until("done")
        } else {
            step
        }
    }
}

impl MainView {
    /// What a step's card offers to choose from, which can follow what's been chosen and what the gateway holds.
    fn guide_choices(&self, step: GuideStep) -> Vec<Pick> {
        match step {
            GuideStep::Welcome => vec![Pick::Setup, Pick::Tour],
            GuideStep::TourEnd => vec![Pick::Setup, Pick::Finish],
            GuideStep::ChooseAgent => Agent::ALL
                .into_iter()
                .map(Pick::Agent)
                .chain((0..self.agents.custom.agents.len()).map(Pick::Custom))
                .collect(),
            GuideStep::ChooseGateway => vec![
                Pick::Gateway(Place::Docker),
                Pick::Gateway(Place::MicroVm),
                Pick::Gateway(Place::Another),
            ],
            GuideStep::ChooseKey => {
                let Some(guide) = &self.guide else {
                    return Vec::new();
                };
                let Some(agent) = &guide.agent else {
                    return Vec::new();
                };
                let held = match &guide.keys {
                    Some(Ok(keys)) => keys
                        .iter()
                        .enumerate()
                        .filter(|(_, key)| agent.takes(&key.kind))
                        .map(|(at, _)| Pick::Key(at))
                        .collect(),
                    _ => Vec::new(),
                };
                let kinds: Vec<Kind> = match agent {
                    Harness::Agent(agent) => agent.model_kinds().to_vec(),
                    Harness::Custom(custom) => custom.takes.clone(),
                };
                held.into_iter()
                    .chain(kinds.into_iter().map(Pick::NewKey))
                    .collect()
            }
            GuideStep::LearnMore => vec![Pick::Docs, Pick::Repository],
            _ => Vec::new(),
        }
    }

    /// A choice's title and the line under it.
    fn guide_pick_words(&self, pick: Pick) -> (String, String) {
        let kinds = |kinds: &[Kind]| {
            kinds
                .iter()
                .map(|kind| kind.name())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match pick {
            Pick::Tour => (
                t!("Show Me Around").into_owned(),
                t!("A look at each part of the window").into_owned(),
            ),
            Pick::Setup => (
                t!("Set Up").into_owned(),
                t!("Choose an agent and sign in, then make and run your first sandbox")
                    .into_owned(),
            ),
            Pick::Agent(agent) => (
                agent.name().to_string(),
                t!("Takes %{kinds}", kinds = kinds(agent.model_kinds())).into_owned(),
            ),
            Pick::Custom(at) => match self.agents.custom.agents.get(at) {
                Some(custom) => (
                    custom.name.clone(),
                    t!("Yours · %{kinds}", kinds = kinds(&custom.takes)).into_owned(),
                ),
                None => (String::new(), String::new()),
            },
            Pick::Gateway(Place::Docker) => (
                t!("Docker on this Mac").into_owned(),
                t!("Recommended: OpenShell's own gateway, ready once OpenShell runs").into_owned(),
            ),
            Pick::Gateway(Place::MicroVm) => (
                t!("A MicroVM on this Mac").into_owned(),
                t!("Each sandbox a small virtual machine, which Playpen sets up").into_owned(),
            ),
            Pick::Gateway(Place::Another) => (
                t!("Another gateway").into_owned(),
                t!("A gateway on a server or a cluster, by its address").into_owned(),
            ),
            Pick::Key(at) => {
                let key = self
                    .guide
                    .as_ref()
                    .and_then(|guide| guide.keys.as_ref()?.as_ref().ok()?.get(at).cloned());
                match key {
                    Some(key) => (
                        t!("Use “%{name}”", name = key.name).into_owned(),
                        t!(
                            "%{kind}, which the gateway holds already",
                            kind =
                                Kind::of(&key.kind).map_or(key.kind.as_str(), |kind| kind.name())
                        )
                        .into_owned(),
                    ),
                    None => (String::new(), String::new()),
                }
            }
            Pick::NewKey(kind) => (
                match kind {
                    Kind::ClaudeSubscription => t!("Sign In with Claude").into_owned(),
                    Kind::ChatGptSubscription => t!("Sign In with ChatGPT").into_owned(),
                    kind => t!("Add Your %{kind} Key", kind = kind.name()).into_owned(),
                },
                kind.where_to_get_a_key().to_string(),
            ),
            Pick::Docs => (
                t!("OpenShell's Guides").into_owned(),
                t!("How OpenShell's sandboxes, policies and providers work").into_owned(),
            ),
            Pick::Repository => (
                t!("OpenShell on GitHub").into_owned(),
                t!("Its code, releases and issues").into_owned(),
            ),
            Pick::Finish => (t!("Done").into_owned(), t!("Close the tour").into_owned()),
        }
    }

    /// Starts the guide's `stage`, over whatever else is running of it.
    pub(super) fn start_guide(
        &mut self,
        stage: Stage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if stage == Stage::Welcome {
            crate::restore::note_welcomed(cx);
        }
        self.card = None;
        self.agents.form = None;
        let (agent, place) = match self.guide.take() {
            Some(guide) if stage == Stage::SetupRest => (guide.agent, guide.place),
            _ => (None, None),
        };
        let steps = stage.steps();
        let setting_up = Stage::Setup.steps().len() + Stage::SetupRest.steps().len();
        let (before, all) = match stage {
            Stage::Setup => (0, setting_up),
            Stage::SetupRest => (Stage::Setup.steps().len(), setting_up),
            Stage::Welcome | Stage::Tour => (0, steps.len()),
        };
        let main = cx.entity().downgrade();
        let tour = cx.new(|cx| {
            Tour::new(steps.iter().map(|step| step.step()).collect(), cx)
                .with_card(move |card, window, cx| guide_card(&main, card, window, cx))
        });
        let events = cx.subscribe_in(&tour, window, |this, _, event, window, cx| match event {
            TourEvent::Shown(at) => this.guide_shown(*at, window, cx),
            TourEvent::Done | TourEvent::Skipped(_) => this.guide_ended(window, cx),
        });
        let watching = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(700))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| this.guide_look(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.guide = Some(Guide {
            tour: tour.clone(),
            stage,
            steps,
            at: None,
            came_back: false,
            before,
            all,
            parked: false,
            agent,
            place,
            keys: None,
            key: None,
            kind: None,
            kit: None,
            made: false,
            _events: events,
            _listing: None,
            _watching: watching,
        });
        // Once the window's drawn, so what the first step points at is there.
        cx.defer_in(window, move |_, window, cx| {
            tour.update(cx, |tour, cx| tour.start(window, cx));
        });
        cx.notify();
    }

    /// Does what a step needs of the window as it's shown: the place it's about, or the form it waits on. Each is safe to
    /// do again, since Back shows a step again.
    fn guide_shown(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(guide) = &mut self.guide else {
            return;
        };
        guide.came_back = guide.at.is_some_and(|before| at < before);
        guide.at = Some(at);
        let (agent, place, kind, kit) = (
            guide.agent.clone(),
            guide.place,
            guide.kind,
            guide.kit.clone(),
        );
        let Some(step) = guide.shown() else {
            return;
        };
        match step {
            GuideStep::Places | GuideStep::Gateways | GuideStep::Detail | GuideStep::NewAgent => {
                self.dismiss_active_form(window, cx);
                self.show_page(Page::Sandboxes, window, cx);
            }
            GuideStep::Kits | GuideStep::NewKit => self.show_page(Page::Kits, window, cx),
            GuideStep::Providers => self.show_page(Page::Providers, window, cx),
            GuideStep::Gateway => {
                self.show_page(Page::Sandboxes, window, cx);
                match place {
                    Some(Place::MicroVm) if self.microvm.is_none() => {
                        self.start_microvm_form(window, cx);
                    }
                    Some(Place::Another) if self.adding.is_none() => {
                        self.start_adding_gateway(window, cx);
                    }
                    _ => {}
                }
            }
            GuideStep::ChooseKey => {
                // The key goes on the gateway chosen, which Providers acts on.
                if let Some(gateway) = self.guide_gateway()
                    && self.gateway.name() != Some(gateway.as_str())
                {
                    self.show_gateway(&gateway, window, cx);
                }
                self.guide_list_keys(window, cx);
            }
            // Coming back to signing in is coming back to choosing the key, which may have been one held already.
            GuideStep::SignIn if self.guide.as_ref().is_some_and(|guide| guide.came_back) => {
                self.guide_show(GuideStep::ChooseKey, window, cx);
                return;
            }
            GuideStep::SignIn => {
                if let Some(kind) = kind {
                    self.add_provider_of(kind, window, cx);
                }
            }
            GuideStep::KitKey => {
                if let Some(name) = self.kits.selected.clone() {
                    if let Some(guide) = &mut self.guide {
                        guide.kit = Some(name.clone());
                    }
                    self.select_kit(name, window, cx);
                }
            }
            GuideStep::MakeSandbox => {
                if self.making.is_none()
                    && let Some(agent) = agent
                {
                    self.start_making_for(agent, window, cx);
                    if let Some(gateway) = self.guide_gateway() {
                        self.choose_making_gateway(gateway, window, cx);
                    }
                }
                if let Some(kit) = kit
                    && !self.making_uses_kit(&kit)
                {
                    self.use_kit(&kit, cx);
                }
            }
            GuideStep::OpenIt => self.show_page(Page::Sandboxes, window, cx),
            GuideStep::Welcome
            | GuideStep::Palette
            | GuideStep::TourEnd
            | GuideStep::ChooseAgent
            | GuideStep::ChooseGateway
            | GuideStep::LearnMore => {}
        }
        if step.waits()
            && let Some(guide) = &mut self.guide
        {
            guide.parked = true;
            let tour = guide.tour.clone();
            cx.defer_in(window, move |_, _, cx| {
                tour.update(cx, |tour, cx| tour.stop(cx))
            });
            // A step with a form leaves the keyboard in it; the rest give it back to the window.
            let opens_a_form = match step {
                GuideStep::SignIn | GuideStep::MakeSandbox => true,
                GuideStep::Gateway => matches!(place, Some(Place::MicroVm | Place::Another)),
                _ => false,
            };
            if !opens_a_form {
                self.focus.focus(window, cx);
            }
        }
        cx.notify();
    }

    /// Asks the chosen gateway for the keys it holds, off the window's thread. A capture or a test doesn't ask.
    fn guide_list_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(guide) = &mut self.guide else {
            return;
        };
        if self.watch == Watch::Nothing {
            guide.keys = Some(Ok(Vec::new()));
            return;
        }
        guide.keys = None;
        let key = self.gateway_key();
        let openshell = self.openshell_for(&key);
        let task = cx.spawn_in(window, async move |this, cx| {
            let listed = cx
                .background_spawn(
                    async move { openshell.and_then(|openshell| openshell.providers()) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(guide) = &mut this.guide {
                    guide.keys = Some(listed.map_err(|error| error.to_string()));
                }
                cx.notify();
            });
        });
        if let Some(guide) = &mut self.guide {
            guide._listing = Some(task);
        }
    }

    /// Brings the tour back at the step `step` places from the one waiting: the next once it's done, or the one before.
    fn guide_resume(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(guide) = &mut self.guide else {
            return;
        };
        let Some(at) = guide.at else {
            return;
        };
        guide.parked = false;
        let tour = guide.tour.clone();
        let to = at.saturating_add_signed(step);
        if !tour.update(cx, |tour, cx| tour.show(to, window, cx)) {
            self.guide = None;
        }
        cx.notify();
    }

    /// Shows the step `step`, wherever the tour is.
    fn guide_show(&mut self, step: GuideStep, window: &mut Window, cx: &mut Context<Self>) {
        let Some(guide) = &mut self.guide else {
            return;
        };
        let Some(to) = guide.steps.iter().position(|known| *known == step) else {
            return;
        };
        guide.parked = false;
        let tour = guide.tour.clone();
        tour.update(cx, |tour, cx| tour.show(to, window, cx));
    }

    fn guide_ended(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Stepping aside for a step that waits leaves the keyboard where the step put it, such as a form's first field.
        if self.guide.as_ref().is_some_and(|guide| guide.parked) {
            return;
        }
        // A choice that starts another stage has made its guide already; this one's ending is that one's beginning.
        if self
            .guide
            .as_ref()
            .is_some_and(|guide| guide.tour.read(cx).is_running())
        {
            return;
        }
        self.guide = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// What a card's choice does.
    fn guide_pick(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        match pick {
            Pick::Tour => self.start_guide(Stage::Tour, window, cx),
            Pick::Setup => self.start_guide(Stage::Setup, window, cx),
            Pick::Agent(agent) => self.guide_choose_agent(Harness::Agent(agent), window, cx),
            Pick::Custom(at) => {
                if let Some(custom) = self.agents.custom.agents.get(at).cloned() {
                    self.guide_choose_agent(Harness::Custom(custom), window, cx);
                }
            }
            Pick::Gateway(place) => {
                if let Some(guide) = &mut self.guide {
                    guide.place = Some(place);
                }
                self.start_guide(Stage::SetupRest, window, cx);
            }
            Pick::Key(at) => {
                let Some(guide) = &mut self.guide else {
                    return;
                };
                let Some(key) = guide
                    .keys
                    .as_ref()
                    .and_then(|keys| keys.as_ref().ok()?.get(at).cloned())
                else {
                    return;
                };
                guide.kind = Kind::of(&key.kind);
                guide.key = Some(key.name);
                // A key held already needs no signing in.
                self.guide_show(GuideStep::NewKit, window, cx);
            }
            Pick::NewKey(kind) => {
                if let Some(guide) = &mut self.guide {
                    guide.kind = Some(kind);
                    guide.key = None;
                }
                self.guide_signal("chosen", window, cx);
            }
            Pick::Docs => cx.open_url(OPENSHELL_DOCS),
            Pick::Repository => cx.open_url(OPENSHELL_REPOSITORY),
            Pick::Finish => {
                if let Some(guide) = &self.guide {
                    guide.tour.update(cx, |tour, cx| tour.stop(cx));
                }
            }
        }
    }

    fn guide_choose_agent(&mut self, agent: Harness, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(guide) = &mut self.guide {
            guide.agent = Some(agent);
        }
        self.guide_signal("chosen", window, cx);
    }

    fn guide_signal(&mut self, signal: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(guide) = &self.guide {
            let tour = guide.tour.clone();
            tour.update(cx, |tour, cx| tour.signal(signal, window, cx));
        }
    }

    /// Goes on from `step` if it's the one waiting: the person has done what it asks.
    pub(super) fn guide_done(
        &mut self,
        step: GuideStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if step == GuideStep::MakeSandbox
            && let Some(guide) = &mut self.guide
        {
            guide.made = true;
        }
        if self.guide.as_ref().and_then(Guide::shown) == Some(step)
            && self.guide.as_ref().is_some_and(|guide| guide.parked)
        {
            self.guide_resume(1, window, cx);
        }
    }

    /// A key was saved, by its provider's name: the one setting up uses, when it's signing in.
    pub(super) fn guide_key_saved(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.guide.as_ref().and_then(Guide::shown) != Some(GuideStep::SignIn) {
            return;
        }
        if let Some(guide) = &mut self.guide {
            guide.key = Some(name);
        }
        self.guide_done(GuideStep::SignIn, window, cx);
    }

    /// Whether what `step` waits on is done, however it was done.
    fn guide_step_done(&self, step: GuideStep) -> bool {
        let Some(guide) = &self.guide else {
            return false;
        };
        match step {
            GuideStep::Gateway => self.guide_gateway_answers(),
            GuideStep::SignIn => guide.key.is_some(),
            GuideStep::KitKey => self
                .kits
                .chosen()
                .is_some_and(|kit| kit.model_key.is_some()),
            GuideStep::MakeSandbox => guide.made,
            _ => false,
        }
    }

    /// Looks for what the step shown waits on that the window doesn't say when it happens. A step come back to waits
    /// for Next instead, since what it waits on is likely done already.
    fn guide_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(guide) = &self.guide else {
            return;
        };
        let Some(step) = guide.shown() else {
            return;
        };
        if guide.came_back || !matches!(step, GuideStep::Gateway | GuideStep::KitKey) {
            // Its card may say something new, such as the gateway now answering.
            cx.notify();
            return;
        }
        if self.guide_step_done(step) {
            self.guide_done(step, window, cx);
        }
    }

    /// The gateway setting up makes its sandbox on: the MicroVM one, OpenShell's own on Docker, or the one added, which
    /// is shown once it's added.
    fn guide_gateway(&self) -> Option<String> {
        match self.guide.as_ref()?.place? {
            Place::MicroVm => Some(MICROVM_GATEWAY.to_string()),
            Place::Docker => self
                .gateways
                .iter()
                .find(|gateway| gateway.is_local() && gateway.name != MICROVM_GATEWAY)
                .map(|gateway| gateway.name.clone())
                .or_else(|| self.gateway.name().map(str::to_string)),
            Place::Another => self.gateway.name().map(str::to_string),
        }
    }

    /// Whether the gateway chosen while setting up answers: its sandboxes have been listed.
    fn guide_gateway_answers(&self) -> bool {
        let place = self.guide.as_ref().and_then(|guide| guide.place);
        let listed = |name: &str| matches!(self.lists.get(name), Some(Listing::Listed(_)));
        match place {
            Some(Place::MicroVm) => listed(MICROVM_GATEWAY),
            // A gateway added is shown once it's added.
            Some(Place::Another) => {
                self.adding.is_none()
                    && self
                        .gateway
                        .name()
                        .is_some_and(|name| name != MICROVM_GATEWAY && listed(name))
            }
            Some(Place::Docker) | None => {
                self.gateways.iter().any(|gateway| {
                    gateway.is_local() && gateway.name != MICROVM_GATEWAY && listed(&gateway.name)
                }) || (self.gateways.is_empty() && listed(&self.gateway_key()))
            }
        }
    }

    /// What a step's card says, which can follow what's been chosen and what the window shows.
    fn guide_body(&self, step: GuideStep) -> String {
        let guide = self.guide.as_ref();
        let agent = guide.and_then(|guide| guide.agent.as_ref()).map_or_else(
            || Agent::ALL[0].name().to_string(),
            |agent| agent.name().to_string(),
        );
        let kind = guide.and_then(|guide| guide.kind);
        let key = guide.and_then(|guide| guide.key.clone());
        let gateway = self
            .guide_gateway()
            .unwrap_or_else(|| t!("the gateway").into_owned());
        match step {
            GuideStep::Welcome => t!(
                "Playpen runs coding agents in OpenShell sandboxes, where each reaches only its model and what you allow. Set up your first sandbox now, or look around first."
            ),
            GuideStep::Places => t!(
                "Sandboxes, the agents Playpen can run, kits of keys, and the providers that hold them. ⌘1 to ⌘4 go to each."
            ),
            GuideStep::Gateways => t!(
                "Each gateway OpenShell knows, with its sandboxes under it. A gateway runs sandboxes on this Mac's Docker, in MicroVMs, or on a server."
            ),
            GuideStep::Detail => t!(
                "The chosen sandbox: open its agent, see what it asked to reach, attach services, and bring its work back as a branch."
            ),
            GuideStep::NewAgent => t!(
                "Makes a sandbox for an agent, with copies of the folders you choose and the key it uses. ⌘N does it too."
            ),
            GuideStep::Kits => t!(
                "A kit is a set of keys to give a sandbox together, such as Work or Personal: a model key, services, and variables."
            ),
            GuideStep::Providers => t!(
                "The keys OpenShell holds for each gateway. A key never enters a sandbox: OpenShell adds it to the agent's requests."
            ),
            GuideStep::Palette => t!(
                "⌘K finds any command by name, and ⌘/ shows every key. Help ▸ Set Up Playpen… brings setting up back."
            ),
            GuideStep::TourEnd => t!("Set up your first sandbox now, or later from the Help menu."),
            GuideStep::ChooseAgent => t!(
                "Which coding agent will run in your sandbox? Under each are the keys it can use."
            ),
            GuideStep::ChooseGateway => t!(
                "A gateway keeps sandboxes and runs them, and holds their keys. OpenShell's own, on this Mac's Docker, is the simplest."
            ),
            GuideStep::Gateway => match guide.and_then(|guide| guide.place) {
                Some(Place::MicroVm) => t!(
                    "Set up the MicroVM gateway here. This goes on once it answers."
                ),
                Some(Place::Another) => t!(
                    "Give the gateway's address and add it. This goes on once it answers."
                ),
                _ if self.guide_gateway_answers() => t!("OpenShell's gateway is answering."),
                _ => t!(
                    "Start OpenShell's gateway with brew services start openshell, which needs Docker running. This goes on once it answers."
                ),
            },
            GuideStep::ChooseKey => match guide.and_then(|guide| guide.keys.as_ref()) {
                None => t!(
                    "Looking for the keys %{gateway} holds…",
                    gateway = gateway.clone()
                ),
                Some(Err(_)) => t!(
                    "%{gateway} didn't say which keys it holds. Add one for %{agent}:",
                    gateway = gateway.clone(),
                    agent = agent.clone()
                ),
                Some(Ok(_)) => t!(
                    "%{agent} reaches its model with a key %{gateway} keeps for it. Use one it holds, or add one:",
                    agent = agent.clone(),
                    gateway = gateway.clone()
                ),
            },
            GuideStep::SignIn => match kind {
                Some(Kind::ClaudeSubscription) => t!(
                    "Sign In with Claude opens Claude's sign-in in your browser and brings the token back. This goes on once it's saved."
                ),
                Some(Kind::ChatGptSubscription) => t!(
                    "Sign In… gives you a code to enter at OpenAI's sign-in, in any browser. This goes on once it's saved."
                ),
                Some(Kind::LiteLlm) => t!(
                    "Give your proxy's address and a virtual key from its admin, then Add Model Key. This goes on once it's saved."
                ),
                Some(kind) => t!(
                    "%{source} Paste it, then Add Model Key. This goes on once it's saved.",
                    source = kind.where_to_get_a_key()
                ),
                None => t!("Add the key, then save it. This goes on once it's saved."),
            },
            GuideStep::NewKit => t!("Click + to make a kit."),
            GuideStep::KitKey => match key {
                Some(key) => t!(
                    "Rename it if you like, and choose “%{key}” as its model key. A sandbox made with the kit gets it.",
                    key = key
                ),
                None => t!(
                    "Rename it if you like, and choose your key as its model key. A sandbox made with the kit gets it."
                ),
            },
            GuideStep::MakeSandbox => match kind {
                Some(Kind::LiteLlm) => t!(
                    "Your kit is chosen. Type the proxy's model under Model, add a folder if you like, and Make Sandbox. The first takes a few minutes, while %{agent}'s image is built.",
                    agent = agent.clone()
                ),
                _ => t!(
                    "Your kit is chosen. Add a folder for it to work on if you like, and Make Sandbox. The first takes a few minutes, while %{agent}'s image is built.",
                    agent = agent.clone()
                ),
            },
            GuideStep::OpenIt => {
                let ready = self
                    .selected_sandbox()
                    .is_some_and(|sandbox| sandbox.phase.is_ready());
                if ready {
                    t!("Open runs %{agent} in a terminal, in its sandbox.", agent = agent)
                } else {
                    t!(
                        "It's starting. Once it's Ready, Open runs %{agent} in a terminal, in its sandbox.",
                        agent = agent
                    )
                }
            }
            GuideStep::LearnMore => t!(
                "Your agent runs in its sandbox. When it asks to reach something new, Playpen shows the request for you to allow or deny. OpenShell's guides say more."
            ),
        }
        .into_owned()
    }

    /// The card in the window's corner for a step that waits on the person: what to do, Back and Skip, and Next when
    /// the step was come back to and is done already.
    pub(super) fn guide_banner(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let guide = self.guide.as_ref().filter(|guide| guide.parked)?;
        let step = guide.shown()?;
        let at = guide.at?;
        let theme = cx.theme();
        let number = guide.before + at;
        let next = guide.came_back && self.guide_step_done(step) || step == GuideStep::OpenIt;
        Some(
            v_flex()
                .id("guide-waiting")
                .absolute()
                .right(rems(24. / 13.))
                .bottom(rems(64. / 13.))
                .w(rems(280. / 13.))
                .p_4()
                .gap_2()
                .rounded(theme.radius * 1.5)
                .border_1()
                .border_color(theme.primary.opacity(0.6))
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .shadow_lg()
                // Clicks on it stay on it, not the form under it.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    h_flex()
                        .gap_2()
                        .justify_between()
                        .child(div().font_semibold().child(step.title()))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(
                                    t!("%{at} of %{count}", at = number + 1, count = guide.all)
                                        .into_owned(),
                                ),
                        ),
                )
                .child(div().text_sm().child(self.guide_body(step)))
                .child(
                    h_flex()
                        .gap_2()
                        .justify_between()
                        .items_center()
                        .child(
                            Button::new("guide-waiting-skip")
                                .ghost()
                                .xsmall()
                                .label(t!("Skip Setup").into_owned())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.guide = None;
                                    this.focus.focus(window, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .when(!next, |row| {
                                    row.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(t!("Waiting for you…").into_owned()),
                                    )
                                })
                                .when(at > 0, |row| {
                                    row.child(
                                        Button::new("guide-waiting-back")
                                            .xsmall()
                                            .label(t!("Back").into_owned())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.guide_back(window, cx);
                                            })),
                                    )
                                })
                                .when(next, |row| {
                                    row.child(
                                        Button::new("guide-waiting-next")
                                            .primary()
                                            .xsmall()
                                            .label(t!("Next").into_owned())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.guide_resume(1, window, cx);
                                            })),
                                    )
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Back from a step waiting in the corner: the step before.
    fn guide_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.guide_resume(-1, window, cx);
    }

    /// Shows Providers with the form open for a provider of `kind`.
    fn add_provider_of(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.show_page(Page::Providers, window, cx);
        if let Some(providers) = &self.providers {
            providers.update(cx, |providers, cx| providers.add(kind, window, cx));
        }
    }
}

/// A step's card: its title, what it says, its choices, and Back, Skip and Next where they apply.
fn guide_card(
    main: &WeakEntity<MainView>,
    card: &Card,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(view) = main.upgrade() else {
        return div().into_any_element();
    };
    let (step, (number, count), stage) = {
        let view = view.read(cx);
        let Some(guide) = &view.guide else {
            return div().into_any_element();
        };
        (
            guide.steps.get(card.at).copied(),
            (guide.before + card.at, guide.all),
            guide.stage,
        )
    };
    let Some(step) = step else {
        return div().into_any_element();
    };
    let body = view.read(cx).guide_body(step);
    let theme = cx.theme();
    let (background, foreground, muted, border) = (
        theme.popover,
        theme.popover_foreground,
        theme.muted_foreground,
        theme.border,
    );
    let primary = theme.primary;
    let picks = view.read(cx).guide_choices(step);
    let has_choices = !picks.is_empty();
    let words: Vec<(String, String)> = picks
        .iter()
        .map(|pick| view.read(cx).guide_pick_words(*pick))
        .collect();
    let theme = cx.theme();
    let (hover, radius) = (theme.list_hover, theme.radius);
    let choices = picks.into_iter().zip(words).map(|(pick, (title, detail))| {
        let main = main.clone();
        div()
            .id(ElementId::from((
                ElementId::from("guide-pick"),
                format!("{pick:?}"),
            )))
            .px_3()
            .py_1p5()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .child(div().text_sm().font_medium().child(title))
            .child(div().text_xs().text_color(muted).child(detail))
            .on_click(move |_, window, cx| {
                main.update(cx, |this, cx| this.guide_pick(pick, window, cx))
                    .ok();
            })
    });
    let (next, back, skip) = (card.clone(), card.clone(), card.clone());
    let last = number + 1 == count;
    let goes_on = card.advance == Advance::Next;
    let waiting = step.waits();
    let footer = h_flex()
        .gap_2()
        .justify_between()
        .items_center()
        .child(
            Button::new("guide-skip")
                .ghost()
                .xsmall()
                .label(if last || stage == Stage::Welcome {
                    t!("Close").into_owned()
                } else {
                    t!("Skip").into_owned()
                })
                .on_click(move |_, _, cx| skip.skip(cx)),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .when(waiting, |row| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("Waiting for you…").into_owned()),
                    )
                })
                .when(card.at > 0, |row| {
                    row.child(
                        Button::new("guide-back")
                            .xsmall()
                            .label(t!("Back").into_owned())
                            .on_click(move |_, window, cx| back.back(window, cx)),
                    )
                })
                .when(goes_on, |row| {
                    row.child(
                        Button::new("guide-next")
                            .primary()
                            .xsmall()
                            .label(if last {
                                t!("Done").into_owned()
                            } else {
                                t!("Next").into_owned()
                            })
                            .on_click(move |_, window, cx| next.next(window, cx)),
                    )
                }),
        );
    v_flex()
        .w(rems(340. / 13.))
        .p_4()
        .gap_3()
        .rounded(radius * 1.5)
        .border_1()
        .border_color(border)
        .bg(background)
        .text_color(foreground)
        .shadow_lg()
        .child(
            h_flex()
                .gap_2()
                .justify_between()
                .child(div().font_semibold().child(card.title.clone()))
                .when(count > 1, |header| {
                    header.child(div().flex_none().text_xs().text_color(muted).child(
                        t!("%{at} of %{count}", at = number + 1, count = count).into_owned(),
                    ))
                }),
        )
        .child(div().text_sm().child(body))
        .when(has_choices, |card| {
            card.child(v_flex().gap_1p5().children(choices))
        })
        .child(div().h(px(1.)).bg(border).opacity(0.6))
        .child(footer)
        .when(count > 1, |card_view| {
            card_view.child(
                div()
                    .h(px(2.))
                    .w(relative((number + 1) as f32 / count as f32))
                    .bg(primary)
                    .rounded_full(),
            )
        })
        .into_any_element()
}

#[cfg(test)]
impl MainView {
    pub(super) fn guide_step_shown(&self) -> Option<GuideStep> {
        self.guide.as_ref().and_then(Guide::shown)
    }

    pub(super) fn guide_banner_shown(&self) -> bool {
        self.guide.as_ref().is_some_and(|guide| guide.parked)
    }

    pub(super) fn pick_in_guide(
        &mut self,
        pick: Pick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.guide_pick(pick, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{GuideStep, Stage};

    #[test]
    fn setting_up_chooses_the_agent_and_gateway_before_the_key_it_keeps() {
        let all: Vec<GuideStep> = Stage::Setup
            .steps()
            .into_iter()
            .chain(Stage::SetupRest.steps())
            .collect();
        assert_eq!(
            all,
            vec![
                GuideStep::ChooseAgent,
                GuideStep::ChooseGateway,
                GuideStep::Gateway,
                GuideStep::ChooseKey,
                GuideStep::SignIn,
                GuideStep::NewKit,
                GuideStep::KitKey,
                GuideStep::MakeSandbox,
                GuideStep::OpenIt,
                GuideStep::LearnMore,
            ]
        );
        assert!(GuideStep::ChooseAgent.chooses());
        assert!(GuideStep::ChooseKey.chooses());
        assert!(GuideStep::SignIn.waits());
        assert!(!GuideStep::LearnMore.chooses(), "its links don't end it");
    }
}
