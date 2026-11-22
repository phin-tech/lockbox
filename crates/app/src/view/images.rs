//! Gateway ▸ Image Source…: where the shown gateway gets the images its agents run in, in place of the chosen sandbox.
//! It's what `[gateways.<name>]` in the settings file says, so the file needn't be edited by hand.

use playpen_core::{DockerError, GatewaySettings, ImageSourceError, ImagesFrom, Platform};

use super::*;

/// The image source of a gateway, being chosen.
pub struct Imaging {
    gateway: String,
    from: ImagesFrom,
    registry: Entity<InputState>,
    platform: Option<Platform>,
    failed: Option<String>,
    _changed: Subscription,
}

impl MainView {
    /// Shows the form for the shown gateway, with what's chosen for it already, or what suits where it is.
    pub(super) fn start_images_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.gateway.name().map(str::to_string) else {
            return;
        };
        let info = self.current_gateway();
        let chosen = crate::settings::current(cx)
            .gateways
            .get(&name)
            .cloned()
            .unwrap_or_default();
        // A gateway on this Mac shares its Docker; any other needs a registry to pull from.
        let from = chosen.images.unwrap_or(if info.is_local() {
            ImagesFrom::Docker
        } else {
            ImagesFrom::Registry
        });
        let registry = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("ghcr.io/your-name");
            state.set_value(chosen.registry.clone(), window, cx);
            state
        });
        let changed = cx.subscribe_in(
            &registry,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_images(window, cx);
                }
            },
        );
        self.making = None;
        self.adding = None;
        self.failed = None;
        self.imaging = Some(Imaging {
            gateway: name,
            from,
            registry,
            platform: chosen.platform,
            failed: None,
            _changed: changed,
        });
        cx.notify();
    }

    pub(super) fn cancel_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.imaging = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn choose_images_from(&mut self, from: ImagesFrom, cx: &mut Context<Self>) {
        if let Some(imaging) = &mut self.imaging {
            imaging.from = from;
            imaging.failed = None;
            cx.notify();
        }
    }

    fn choose_platform(&mut self, platform: Platform, cx: &mut Context<Self>) {
        if let Some(imaging) = &mut self.imaging {
            imaging.platform = Some(platform);
            imaging.failed = None;
            cx.notify();
        }
    }

    /// Saves what's chosen, once it says enough to make a sandbox from.
    pub(super) fn save_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let info = self.current_gateway();
        let Some(imaging) = &mut self.imaging else {
            return;
        };
        let registry = imaging.registry.read(cx).value().trim().to_string();
        let set = GatewaySettings {
            images: Some(imaging.from),
            // A registry doesn't belong to this Mac's Docker, so it isn't kept with it.
            registry: if imaging.from == ImagesFrom::Docker {
                String::new()
            } else {
                registry
            },
            platform: (imaging.from != ImagesFrom::Docker)
                .then_some(imaging.platform)
                .flatten(),
        };
        // The same checks making a sandbox does, so what's saved is what a sandbox can be made from. Whether Docker is
        // installed isn't one of them.
        let mut trial = crate::settings::current(cx);
        trial.gateways.insert(imaging.gateway.clone(), set.clone());
        if let Err(
            error @ (ImageSourceError::NotSet(_)
            | ImageSourceError::NoRegistry(_)
            | ImageSourceError::NoPlatform(_)),
        ) = trial.image_source(&info, || Err(DockerError::NotInstalled))
        {
            imaging.failed = Some(error.to_string());
            cx.notify();
            return;
        }
        tracing::info!(gateway = %imaging.gateway, from = ?imaging.from, "choosing where a gateway's images come from");
        crate::settings::set_gateway(&imaging.gateway.clone(), Some(set), cx);
        self.imaging = None;
        self.failed = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn images_panel(&self, imaging: &Imaging, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let header = Self::panel_header(t!("Image Source"), cx).child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("cancel-images")
                        .ghost()
                        .xsmall()
                        .label(t!("Cancel").into_owned())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_images(window, cx)),
                        ),
                )
                .child(
                    Button::new("save-images")
                        .primary()
                        .xsmall()
                        .label(t!("Save").into_owned())
                        .on_click(cx.listener(|this, _, window, cx| this.save_images(window, cx))),
                ),
        );
        let option = |from: ImagesFrom, label: String, about: String| {
            v_flex()
                .gap_0p5()
                .child(
                    Radio::new(ElementId::from((
                        ElementId::from("images-from"),
                        label.clone(),
                    )))
                    .label(label)
                    .checked(imaging.from == from)
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_images_from(from, cx))),
                )
                .child(div().pl_6().child(hint(about, cx)))
        };
        let uses_registry = imaging.from != ImagesFrom::Docker;
        let platform = |platform: Platform, label: String| {
            Radio::new(ElementId::from((
                ElementId::from("images-platform"),
                label.clone(),
            )))
            .label(label)
            .checked(imaging.platform == Some(platform))
            .on_click(cx.listener(move |this, _, _, cx| this.choose_platform(platform, cx)))
        };
        let on_docker_elsewhere =
            imaging.from == ImagesFrom::Docker && !self.current_gateway().is_local();
        let body = v_flex()
            .gap_5()
            .p_4()
            .child(hint(
                t!(
                    "Where “%{gateway}” gets the image an agent runs in. A cluster can't pull from this Mac, so it needs a registry.",
                    gateway = imaging.gateway.clone()
                ),
                cx,
            ))
            .child(
                section(t!("Images come from"), cx)
                    .child(option(
                        ImagesFrom::Docker,
                        t!("This Mac's Docker").into_owned(),
                        t!("For a gateway that shares it, such as the one OpenShell runs here, or OrbStack's Kubernetes. Playpen builds the image once for each version.").into_owned(),
                    ))
                    .child(option(
                        ImagesFrom::Registry,
                        t!("A registry Playpen pushes to").into_owned(),
                        t!("Playpen builds the image and pushes it, if it isn't there already. Docker has to be signed in to the registry.").into_owned(),
                    ))
                    .child(option(
                        ImagesFrom::Provided,
                        t!("An image I provide").into_owned(),
                        t!("Playpen only names it. Build it and push it yourself, and OpenShell says if the gateway can't find it.").into_owned(),
                    )),
            )
            .when(on_docker_elsewhere, |body| {
                body.child(
                    div()
                        .text_sm()
                        .text_color(theme.warning)
                        .child(t!("This gateway isn't on this Mac. Its nodes can only use an image built here if they share this Mac's Docker.").into_owned()),
                )
            })
            .when(uses_registry, |body| {
                body.child(
                    section(t!("Registry"), cx)
                        .child(div().w(rems(24.)).child(small_field(&imaging.registry)))
                        .child(hint(
                            t!("The registry and folder the images are in: the name of an image is added to it."),
                            cx,
                        )),
                )
                .child(
                    section(t!("Platform"), cx)
                        .child(
                            h_flex()
                                .gap_4()
                                .child(platform(Platform::Amd64, t!("amd64").into_owned()))
                                .child(platform(Platform::Arm64, t!("arm64").into_owned())),
                        )
                        .child(hint(
                            t!("What the gateway's nodes run. It isn't guessed, since an image for the wrong one won't start. Most clusters are amd64; Apple Silicon and Graviton are arm64."),
                            cx,
                        )),
                )
            })
            .when_some(imaging.failed.clone(), |body, failed| {
                body.child(div().text_sm().text_color(theme.danger).child(failed))
            });
        v_flex()
            .size_full()
            .child(header)
            .child(div().id("images").flex_1().overflow_y_scroll().child(body))
            .into_any_element()
    }
}

#[cfg(test)]
impl Imaging {
    pub(super) fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    pub(super) fn from(&self) -> ImagesFrom {
        self.from
    }

    pub(super) fn type_registry(&self, text: &str, window: &mut Window, cx: &mut App) {
        self.registry.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx)
        });
    }
}

#[cfg(test)]
impl MainView {
    pub(super) fn choose_images(
        &mut self,
        from: ImagesFrom,
        platform: Option<Platform>,
        cx: &mut Context<Self>,
    ) {
        self.choose_images_from(from, cx);
        if let Some(platform) = platform {
            self.choose_platform(platform, cx);
        }
    }
}
