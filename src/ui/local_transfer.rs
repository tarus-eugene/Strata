use super::*;
use strata::local_transfer::{Incoming, Operation, Service, State, Target};

pub(super) struct TransferUi {
    service: Service,
    transfer_mode: bool,
    dialog: Option<Dialog>,
    suspended_send: Option<Dialog>,
    address: Entity<TextInput>,
    name: Entity<TextInput>,
    directory: Entity<TextInput>,
    refresh_focus: FocusHandle,
    folder_focus: FocusHandle,
    peers: Vec<Target>,
    discovering: bool,
    discovery_notice: Option<String>,
    discovery_task: Option<Task<()>>,
    file_prompt: Option<Task<()>>,
    scroll: ScrollState,
}

enum Dialog {
    Send {
        item: Option<PathBuf>,
        operation: Option<Operation>,
        error: Option<String>,
    },
    Receive {
        incoming: Incoming,
        directory: PathBuf,
        accepted: bool,
        completion: Option<CompletionAction>,
        error: Option<String>,
    },
}

impl TransferUi {
    pub(super) fn new(manager: &Manager, window: &mut Window, cx: &mut Context<Strata>) -> Self {
        Self {
            service: Service::start(manager),
            transfer_mode: false,
            dialog: None,
            suspended_send: None,
            address: cx.new(|cx| TextInput::new("192.168.1.12", window, cx)),
            name: cx.new(|cx| TextInput::new("File name", window, cx)),
            directory: cx.new(|cx| TextInput::new("Choose a folder", window, cx)),
            refresh_focus: cx.focus_handle(),
            folder_focus: cx.focus_handle(),
            peers: Vec::new(),
            discovering: false,
            discovery_notice: None,
            discovery_task: None,
            file_prompt: None,
            scroll: ScrollState::default(),
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.dialog.is_some()
    }
}

impl Strata {
    pub(super) fn new_transfer_button(&self, cx: &mut Context<Self>) -> Div {
        let transfer = self.local_transfer.transfer_mode;
        let (title, hint) = if transfer {
            ("New transfer", "Switch to New download")
        } else {
            ("New download", "Switch to New transfer")
        };
        div()
            .flex()
            .flex_shrink_0()
            .child(
                button("top-add", title, Icon::Plus, true)
                    .on_click(cx.listener(|this, _, window, cx| this.activate_new(window, cx))),
            )
            .child(
                div()
                    .id("top-add-mode")
                    .w(px(30.))
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .focusable()
                    .bg(rgb(TEXT))
                    .border_l_1()
                    .border_color(rgb(0xd5d8db))
                    .hover(|style| style.bg(rgb(0xd5d8db)))
                    .focus(|style| style.border_1().border_color(rgb(ACCENT)))
                    .tooltip(move |_, cx| cx.new(|_| ActionTooltip(hint.into(), None)).into())
                    .child(icon(Icon::ChevronDown, 15., BG))
                    .on_click(cx.listener(|this, _, _, cx| this.switch_transfer_mode(cx)))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.switch_transfer_mode(cx);
                            cx.stop_propagation();
                        }
                    })),
            )
    }

    fn switch_transfer_mode(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.settings_open = false;
        self.local_transfer.transfer_mode = !self.local_transfer.transfer_mode;
        cx.notify();
    }

    pub(super) fn activate_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.local_transfer.transfer_mode {
            self.show_add(window, cx);
            return;
        }
        self.finish_resize();
        self.menu = None;
        self.settings_open = false;
        self.local_transfer.dialog = Some(Dialog::Send {
            item: None,
            operation: None,
            error: None,
        });
        self.local_transfer.scroll = ScrollState::default();
        self.local_transfer.address.focus_handle(cx).focus(window);
        self.discover_transfer_targets(window, cx);
    }

    fn discover_transfer_targets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.local_transfer.discovering {
            return;
        }
        self.local_transfer.discovering = true;
        self.local_transfer.discovery_notice = None;
        let scan = self.local_transfer.service.discover();
        self.local_transfer.discovery_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = scan.await;
            let _ = this.update(cx, |this, cx| {
                let transfer = &mut this.local_transfer;
                transfer.discovering = false;
                match result {
                    Ok(Ok(peers)) => transfer.peers = peers,
                    _ => transfer.discovery_notice = Some("Device discovery is unavailable. Enter the target’s Ethernet IPv4 address.".into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn browse_transfer(
        &mut self,
        choose_folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sending = matches!(
            self.local_transfer.dialog,
            Some(Dialog::Send {
                operation: None,
                ..
            })
        );
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: sending && !choose_folder,
            directories: !sending || choose_folder,
            multiple: false,
            prompt: Some(
                if sending && choose_folder {
                    "Choose a folder to send"
                } else if sending {
                    "Choose a file to send"
                } else {
                    "Choose a save location"
                }
                .into(),
            ),
        });
        self.local_transfer.file_prompt = Some(cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    let transfer = &mut this.local_transfer;
                    match &mut transfer.dialog {
                        Some(Dialog::Send {
                            item,
                            operation: None,
                            error,
                        }) if sending => {
                            *item = Some(path);
                            *error = None;
                        }
                        Some(Dialog::Receive {
                            directory,
                            accepted: false,
                            error,
                            ..
                        }) if !sending => {
                            transfer.directory.update(cx, |input, cx| {
                                input.set_value(&path.to_string_lossy(), cx)
                            });
                            *directory = path;
                            *error = None;
                        }
                        _ => return,
                    }
                    cx.notify();
                });
            }
        }));
    }

    fn start_local_transfer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let transfer = &mut self.local_transfer;
        match &mut transfer.dialog {
            Some(Dialog::Send {
                item,
                operation,
                error,
            }) if operation.is_none() => {
                let result = (|| {
                    let target = Target::manual(transfer.address.read(cx).value())?;
                    let target = transfer
                        .peers
                        .iter()
                        .find(|peer| peer.address == target.address)
                        .cloned()
                        .unwrap_or(target);
                    let item = item
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("Choose a file or folder to send"))?;
                    Ok::<_, anyhow::Error>(transfer.service.send(target, item))
                })();
                match result {
                    Ok(sending) => {
                        *operation = Some(sending);
                        *error = None;
                        self.modal_close_focus.focus(window);
                    }
                    Err(failure) => *error = Some(format!("{failure:#}")),
                }
            }
            Some(Dialog::Receive {
                incoming,
                directory,
                accepted,
                error,
                ..
            }) if !*accepted && matches!(incoming.operation.state(), State::Waiting(_)) => {
                let typed = transfer.directory.read(cx).value();
                let folder = if typed == directory.to_string_lossy().as_ref() {
                    directory.clone()
                } else {
                    PathBuf::from(typed.trim())
                };
                match incoming.accept(folder, transfer.name.read(cx).value()) {
                    Ok(()) => {
                        *accepted = true;
                        *error = None;
                        self.modal_close_focus.focus(window);
                    }
                    Err(failure) => *error = Some(format!("{failure:#}")),
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn close_local_transfer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_local_completion(window, cx);
        if let Some(Dialog::Receive {
            mut incoming,
            accepted: false,
            ..
        }) = self.local_transfer.dialog.take()
        {
            incoming.decline();
        }
        self.local_transfer.discovery_task = None;
        self.local_transfer.discovering = false;
        self.local_transfer.file_prompt = None;
        self.local_transfer.dialog = self.local_transfer.suspended_send.take();
        if self.local_transfer.is_open() {
            self.modal_close_focus.focus(window);
        } else {
            self.focus.focus(window);
        }
        cx.notify();
    }

    fn run_local_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Dialog::Receive {
            incoming,
            completion,
            ..
        }) = &mut self.local_transfer.dialog
            && let State::Finished(Some(path)) = incoming.operation.state()
            && let Some(action) = completion.take()
        {
            let task = cx
                .background_executor()
                .spawn(async move { crate::completion::run(action, &path) });
            cx.spawn_in(window, async move |this, cx| {
                if let Err(error) = task.await {
                    let _ = this.update(cx, |this, cx| {
                        this.completion_error =
                            Some(format!("Could not run completion action: {error:#}"));
                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    pub(super) fn poll_local_transfers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_local_completion(window, cx);
        let sending = matches!(
            self.local_transfer.dialog,
            Some(Dialog::Send {
                operation: Some(_),
                ..
            })
        );
        if (self.local_transfer.is_open() && !sending)
            || self.adding
            || self.confirmation.is_some()
            || self.edit.is_some()
            || self.torrent_picker.is_some()
            || self.menu.is_some()
        {
            return;
        }
        while let Some(incoming) = self.local_transfer.service.next_incoming() {
            if !matches!(incoming.operation.state(), State::Waiting(_)) {
                continue;
            }
            self.finish_resize();
            self.settings_open = false;
            let transfer = &mut self.local_transfer;
            transfer.suspended_send = transfer.dialog.take();
            transfer.name.update(cx, |input, cx| {
                input.set_value(&incoming.offer.name, cx);
                input.select_prefix(
                    filename_selection_end(
                        &incoming.offer.name,
                        incoming.offer.folder_entries.is_some(),
                    ),
                    cx,
                );
            });
            transfer.directory.update(cx, |input, cx| {
                input.set_value(&self.directory.to_string_lossy(), cx)
            });
            transfer.scroll = ScrollState::default();
            transfer.dialog = Some(Dialog::Receive {
                incoming,
                directory: self.directory.clone(),
                accepted: false,
                completion: None,
                error: None,
            });
            if let Some(tray) = &self.tray {
                tray.show();
            } else {
                window.activate_window();
            }
            transfer.name.focus_handle(cx).focus(window);
            cx.notify();
            break;
        }
    }

    pub(super) fn transfer_modal(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let content = match self.local_transfer.dialog.as_ref().unwrap() {
            Dialog::Send {
                item,
                operation,
                error,
            } => self.send_transfer_form(item.as_ref(), operation.as_ref(), error.as_deref(), cx),
            Dialog::Receive {
                incoming,
                accepted,
                completion,
                error,
                ..
            } => self.receive_transfer_form(incoming, *accepted, *completion, error.as_deref(), cx),
        };
        deferred(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(rgba(0x000000a6))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    scroll_area(
                        "local-transfer-modal",
                        &self.local_transfer.scroll,
                        content,
                        window,
                        cx.entity_id(),
                    )
                    .w(px(620.))
                    .max_w(relative(0.9))
                    .max_h(relative(0.85))
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgb(OVERLAY_BORDER)),
                ),
        )
        .with_priority(2)
    }

    fn transfer_header(&self, title: &str, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(label(title, 18., TEXT).font_weight(FontWeight::SEMIBOLD))
            .child(
                icon_button("close-transfer", Icon::Close)
                    .track_focus(&self.modal_close_focus)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_local_transfer(window, cx)),
                    ),
            )
    }

    fn send_transfer_form(
        &self,
        item: Option<&PathBuf>,
        operation: Option<&Operation>,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        let transfer = &self.local_transfer;
        let form = div()
            .p(px(22.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(self.transfer_header("New transfer", cx));
        if let Some(operation) = operation {
            return form
                .children(item.map(|item| {
                    label(
                        item.file_name().unwrap_or_default().to_string_lossy(),
                        13.,
                        TEXT,
                    )
                    .truncate()
                }))
                .child(self.transfer_progress(operation.state(), true, cx));
        }
        let address = transfer.address.read(cx).value().trim();
        form.child(div().flex().items_center().justify_between()
                .child(label("Nearby devices", 14., TEXT).font_weight(FontWeight::MEDIUM))
                .child(button("refresh-transfer-targets", if transfer.discovering { "Searching…" } else { "Refresh" }, Icon::Search, false)
                    .track_focus(&transfer.refresh_focus)
                    .on_click(cx.listener(|this, _, window, cx| this.discover_transfer_targets(window, cx)))))
            .children(transfer.discovery_notice.as_deref().map(|notice| label(notice, 12., MUTED)))
            .when(transfer.peers.is_empty() && !transfer.discovering && transfer.discovery_notice.is_none(), |form| {
                form.child(label("No devices found. Open 101 Downloader on the target computer or enter its Ethernet IPv4 address.", 12., MUTED))
            })
            .child(div().flex().flex_wrap().gap(px(6.)).children(transfer.peers.iter().map(|peer| {
                let ip = peer.address.to_string();
                let selected = address == ip;
                let title = if peer.name.is_empty() { ip.clone() } else { format!("{} · {ip}", peer.name) };
                button(format!("transfer-target-{ip}"), &title, Icon::Link, selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.local_transfer.address.update(cx, |input, cx| input.set_value(&ip, cx));
                        cx.notify();
                    }))
            })))
            .child(transport_input("Target Ethernet IPv4 address", transfer.address.clone()))
            .child(div().flex().items_center().gap(px(10.))
                .child(button("select-transfer-file", "Choose file", Icon::File, false)
                    .track_focus(&self.modal_choose_focus)
                    .on_click(cx.listener(|this, _, window, cx| this.browse_transfer(false, window, cx))))
                .child(button("select-transfer-folder", "Choose folder", Icon::Folder, false)
                    .track_focus(&transfer.folder_focus)
                    .on_click(cx.listener(|this, _, window, cx| this.browse_transfer(true, window, cx))))
                .children(item.map(|item| label(item.display().to_string(), 12., MUTED).min_w_0().flex_1().truncate())))
            .children(transfer.service.receive_error.as_deref().map(|error| label(error, 12., AMBER)))
            .children(error.map(|error| label(error, 12., RED)))
            .child(div().flex().justify_end().child(button("send-local-transfer", "Send request", Icon::Arrow, true)
                .track_focus(&self.modal_start_focus)
                .on_click(cx.listener(|this, _, window, cx| this.start_local_transfer(window, cx)))))
    }

    fn receive_transfer_form(
        &self,
        incoming: &Incoming,
        accepted: bool,
        completion: Option<CompletionAction>,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = incoming.operation.state();
        let folder = incoming.offer.folder_entries.is_some();
        let form = div()
            .p(px(22.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(self.transfer_header("Incoming transfer", cx))
            .child(
                div()
                    .p(px(12.))
                    .bg(rgb(BG))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(detail_line(
                        "From",
                        &format!("{} · {}", incoming.offer.sender, incoming.address.ip()),
                    ))
                    .child(detail_line(
                        if folder { "Folder" } else { "File" },
                        &incoming.offer.name,
                    ))
                    .children(
                        incoming
                            .offer
                            .folder_entries
                            .map(|count| detail_line("Items", &count.to_string())),
                    )
                    .child(detail_line("Size", &bytes(incoming.offer.size))),
            );
        let State::Waiting(code) = state else {
            return form.child(self.transfer_progress(state, false, cx));
        };
        if accepted {
            return form.child(label("Preparing destination…", 13., MUTED));
        }
        form.child(label(
            format!("Compare the target code with the sender’s screen: {code}"),
            13.,
            MUTED,
        ))
        .child(transport_input(
            if folder { "Folder name" } else { "File name" },
            self.local_transfer.name.clone(),
        ))
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(8.))
                .child(
                    transport_input("Save to", self.local_transfer.directory.clone())
                        .min_w_0()
                        .flex_1(),
                )
                .child(
                    button("browse-transfer-folder", "Browse", Icon::Folder, false)
                        .h(px(40.))
                        .track_focus(&self.modal_choose_focus)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.browse_transfer(true, window, cx)),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(label("On transfer complete", 13., TEXT).font_weight(FontWeight::MEDIUM))
                .child(
                    div().flex().flex_wrap().gap(px(6.)).children(
                        COMPLETION_CHOICES
                            .into_iter()
                            .enumerate()
                            .map(|(i, action)| {
                                let selected = action == completion;
                                button(
                                    format!("transfer-completion-{i}"),
                                    action.map_or("Do nothing", CompletionAction::label),
                                    if selected {
                                        Icon::Check
                                    } else {
                                        Icon::Completion
                                    },
                                    selected,
                                )
                                .track_focus(&self.confirmation_choices[i])
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.choose_transfer_completion(i, cx),
                                ))
                            }),
                    ),
                )
                .children(match completion {
                    None => None,
                    Some(CompletionAction::Open) if folder => Some(label("Opens the received folder.", 12., MUTED)),
                    Some(CompletionAction::Open) => Some(label("Opens the file if it is a Windows program, or opens the destination folder.", 12., MUTED)),
                    Some(_) => Some(label("This action affects the whole PC when the transfer finishes.", 12., MUTED)),
                }),
        )
        .children(error.map(|error| label(error, 12., RED)))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    button("decline-transfer", "Decline", Icon::Close, false)
                        .track_focus(&self.modal_back_focus)
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.close_local_transfer(window, cx)
                            }),
                        ),
                )
                .child(
                    button("accept-transfer", "Accept", Icon::Download, true)
                        .track_focus(&self.modal_start_focus)
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.start_local_transfer(window, cx)
                            }),
                        ),
                ),
        )
    }

    fn choose_transfer_completion(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(Dialog::Receive { completion, .. }) = &mut self.local_transfer.dialog {
            *completion = COMPLETION_CHOICES[index];
            cx.notify();
        }
    }

    fn transfer_progress(&self, state: State, sending: bool, cx: &mut Context<Self>) -> Div {
        let (message, terminal, tint) = match &state {
            State::Connecting => ("Preparing transfer…".into(), false, MUTED),
            State::Waiting(code) => (
                format!("Waiting for the target to accept. Target code: {code}"),
                false,
                MUTED,
            ),
            State::Transferring { bytes: done, total } => (
                format!(
                    "{} · {} of {}",
                    if sending { "Sending" } else { "Receiving" },
                    bytes(*done),
                    bytes(*total)
                ),
                false,
                TEXT,
            ),
            State::Finished(Some(path)) => (format!("Saved to {}", path.display()), true, TEXT),
            State::Finished(None) => ("Transfer complete.".into(), true, TEXT),
            State::Declined => ("Transfer declined or expired.".into(), true, MUTED),
            State::Failed(error) => (error.clone(), true, RED),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(label(message, 13., tint))
            .when(matches!(state, State::Transferring { .. }), |form| {
                let State::Transferring { bytes, total } = state else {
                    unreachable!()
                };
                let fraction = if total == 0 {
                    1.
                } else {
                    (bytes as f64 / total as f64).clamp(0., 1.) as f32
                };
                form.child(
                    div()
                        .w_full()
                        .h(px(4.))
                        .bg(rgb(RAISED))
                        .child(div().w(relative(fraction)).h_full().bg(rgb(ACCENT))),
                )
            })
            .child(
                div().flex().justify_end().child(
                    button(
                        "end-transfer",
                        if terminal { "Done" } else { "Cancel" },
                        if terminal { Icon::Check } else { Icon::Close },
                        terminal,
                    )
                    .track_focus(&self.modal_start_focus)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_local_transfer(window, cx)),
                    ),
                ),
            )
    }

    pub(super) fn transfer_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let transfer = &self.local_transfer;
        let (sending, editable) = match transfer.dialog.as_ref() {
            Some(Dialog::Send { operation, .. }) => (true, operation.is_none()),
            Some(Dialog::Receive {
                incoming, accepted, ..
            }) => (
                false,
                !accepted && matches!(incoming.operation.state(), State::Waiting(_)),
            ),
            None => return,
        };
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => self.close_local_transfer(window, cx),
            "tab" => {
                let mut handles = Vec::new();
                if editable && sending {
                    handles.extend([
                        transfer.address.focus_handle(cx),
                        transfer.refresh_focus.clone(),
                        self.modal_choose_focus.clone(),
                        transfer.folder_focus.clone(),
                    ]);
                } else if editable {
                    handles.extend([
                        transfer.name.focus_handle(cx),
                        transfer.directory.focus_handle(cx),
                        self.modal_choose_focus.clone(),
                    ]);
                    handles.extend(self.confirmation_choices.iter().cloned());
                    handles.push(self.modal_back_focus.clone());
                }
                handles.extend([
                    self.modal_start_focus.clone(),
                    self.modal_close_focus.clone(),
                ]);
                let index = handles
                    .iter()
                    .position(|handle| handle.is_focused(window))
                    .unwrap_or(handles.len() - 1);
                let step = if event.keystroke.modifiers.shift {
                    handles.len() - 1
                } else {
                    1
                };
                handles[(index + step) % handles.len()].focus(window);
            }
            "enter" | "space"
                if self.modal_close_focus.is_focused(window)
                    || (!sending && editable && self.modal_back_focus.is_focused(window)) =>
            {
                self.close_local_transfer(window, cx)
            }
            "enter" | "space" if editable && self.modal_choose_focus.is_focused(window) => {
                self.browse_transfer(!sending, window, cx)
            }
            "enter" | "space"
                if editable && sending && transfer.folder_focus.is_focused(window) =>
            {
                self.browse_transfer(true, window, cx)
            }
            "enter" | "space"
                if editable && sending && transfer.refresh_focus.is_focused(window) =>
            {
                self.discover_transfer_targets(window, cx)
            }
            "enter" | "space" if self.modal_start_focus.is_focused(window) => {
                if editable {
                    self.start_local_transfer(window, cx);
                } else {
                    self.close_local_transfer(window, cx);
                }
            }
            "enter" | "space" if editable && !sending => {
                let Some(index) = self
                    .confirmation_choices
                    .iter()
                    .position(|handle| handle.is_focused(window))
                else {
                    return;
                };
                self.choose_transfer_completion(index, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}
