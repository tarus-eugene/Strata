use crate::input::TextInput;
use crate::tray::Tray;
use gpui::{prelude::*, *};
use std::{
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use strata::engine::{Download, Manager, Status};

const BG: u32 = 0x0c0d0f;
const SURFACE: u32 = 0x151719;
const RAISED: u32 = 0x242629;
const RULE: u32 = 0x252729;
const TEXT: u32 = 0xf0f1f2;
const MUTED: u32 = 0xb4b6b8;
const FAINT: u32 = 0x92969a;
const BLUE: u32 = 0x82b9e7;
const AMBER: u32 = 0xe4bc73;
const RED: u32 = 0xef8e91;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    All,
    Active,
    Finished,
    Paused,
    Settings,
}

#[derive(Clone, Copy)]
enum Menu {
    Filters,
    Download(u64),
}

#[derive(Clone, Copy)]
enum MenuAction {
    Filter(View),
    Pause(u64),
    Resume(u64),
    Cancel(u64),
    Delete(u64),
    Folder(u64),
    Copy(u64),
    Open(u64),
}

#[derive(Clone, Copy)]
enum Icon {
    Download,
    Minimize,
    Maximize,
    Restore,
    Play,
    Pause,
    Check,
    Folder,
    File,
    Search,
    Plus,
    Settings,
    Close,
    Arrow,
    Link,
    Copy,
    Filter,
    More,
    Trash,
}

pub struct Strata {
    manager: Arc<Manager>,
    tray: Option<Rc<Tray>>,
    tray_error: Option<String>,
    downloads: Vec<Download>,
    view: View,
    selected: Option<u64>,
    url: Entity<TextInput>,
    search: Entity<TextInput>,
    proxy: Entity<TextInput>,
    identity: Entity<TextInput>,
    known_hosts: Entity<TextInput>,
    transport_notice: Option<String>,
    adding: bool,
    error: Option<String>,
    focus: FocusHandle,
    compact: bool,
    menu: Option<(Menu, Point<Pixels>)>,
    menu_index: usize,
    menu_focus: FocusHandle,
    details_height: f32,
    resizing: Option<(Pixels, f32)>,
    browser_registered: bool,
    browser_notice: Option<String>,
    directory: PathBuf,
    connections: usize,
    rates: HashMap<u64, f64>,
    previous: HashMap<u64, u64>,
    sampled_at: Instant,
    _ticker: Task<()>,
    _search_observer: Subscription,
}

impl Strata {
    fn titlebar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let maximized = window.is_maximized();
        div()
            .h(px(44.))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(rgb(BG))
            .border_b_1()
            .border_color(rgb(RULE))
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .px(px(28.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        svg()
                            .path("brand/mark.svg")
                            .size(px(25.))
                            .text_color(rgb(BLUE)),
                    )
                    .child(label("strata", 17., TEXT).font_weight(FontWeight::SEMIBOLD)),
            )
            .child(label("Download manager", 12., FAINT).flex_1())
            .child(
                window_control("window-minimize", Icon::Minimize, WindowControlArea::Min).on_click(
                    |_, window, cx| {
                        cx.stop_propagation();
                        window.minimize_window();
                    },
                ),
            )
            .child(
                window_control(
                    "window-maximize",
                    if maximized {
                        Icon::Restore
                    } else {
                        Icon::Maximize
                    },
                    WindowControlArea::Max,
                )
                .on_click(|_, window, cx| {
                    cx.stop_propagation();
                    toggle_maximize(window);
                }),
            )
            .child(
                window_control("window-close", Icon::Close, WindowControlArea::Close).on_click(
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(tray) = &this.tray {
                            tray.hide();
                        } else {
                            cx.quit();
                        }
                    }),
                ),
            )
    }

    pub fn new(
        manager: Arc<Manager>,
        tray: Option<Rc<Tray>>,
        tray_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = manager.settings.lock().unwrap().clone();
        let url = cx.new(|cx| TextInput::new("URL, magnet link, torrent or Metalink file", cx));
        let search = cx.new(|cx| TextInput::new("Search downloads", cx));
        let proxy =
            cx.new(|cx| TextInput::new("http://, https:// or socks5:// proxy (optional)", cx));
        let identity = cx.new(|cx| TextInput::new("Automatic: ~/.ssh/id_ed25519 or id_rsa", cx));
        let known_hosts = cx.new(|cx| TextInput::new("Default: ~/.ssh/known_hosts", cx));
        proxy.update(cx, |input, cx| {
            input.set_value(
                settings
                    .transport
                    .proxy
                    .as_ref()
                    .map(|p| p.url.as_str())
                    .unwrap_or(""),
                cx,
            )
        });
        identity.update(cx, |input, cx| {
            input.set_value(
                &settings
                    .transport
                    .identity
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                cx,
            )
        });
        known_hosts.update(cx, |input, cx| {
            input.set_value(
                &settings
                    .transport
                    .known_hosts
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                cx,
            )
        });
        let observer = cx.observe(&search, |_, _, cx| cx.notify());
        let ticker = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if this.tray.as_ref().is_some_and(|tray| tray.quitting()) {
                            cx.quit();
                            return;
                        }
                        if this
                            .manager
                            .show_requested
                            .swap(false, std::sync::atomic::Ordering::SeqCst)
                        {
                            if let Some(tray) = &this.tray {
                                tray.show();
                            } else {
                                window.activate_window();
                            }
                        }
                        this.refresh();
                        if !this.tray.as_ref().is_some_and(|tray| tray.is_hidden()) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus);
        let downloads = manager.snapshots();
        let selected = downloads.last().map(|d| d.id);
        Self {
            manager,
            tray,
            tray_error,
            downloads,
            view: View::All,
            selected,
            url,
            search,
            proxy,
            identity,
            known_hosts,
            transport_notice: None,
            adding: false,
            error: None,
            focus,
            compact: false,
            menu: None,
            menu_index: 0,
            menu_focus: cx.focus_handle(),
            details_height: settings.details_height,
            resizing: None,
            browser_registered: strata::bridge::browsers_registered(),
            browser_notice: None,
            directory: settings.directory,
            connections: settings.connections,
            rates: HashMap::new(),
            previous: HashMap::new(),
            sampled_at: Instant::now(),
            _ticker: ticker,
            _search_observer: observer,
        }
    }

    fn refresh(&mut self) {
        let elapsed = self.sampled_at.elapsed().as_secs_f64();
        let previous_id = self.downloads.last().map(|d| d.id);
        self.downloads = self.manager.snapshots();
        if self
            .selected
            .is_some_and(|id| !self.downloads.iter().any(|d| d.id == id))
        {
            self.selected = None;
        }
        if let Some(id) = self.downloads.last().map(|d| d.id)
            && previous_id.is_none_or(|previous| id > previous)
        {
            self.selected = Some(id);
            self.view = View::All;
        }
        for d in &self.downloads {
            let old = self
                .previous
                .insert(d.id, d.downloaded)
                .unwrap_or(d.downloaded);
            let rate = if d.status == Status::Downloading {
                d.downloaded.saturating_sub(old) as f64 / elapsed
            } else {
                0.
            };
            let prior = self.rates.get(&d.id).copied().unwrap_or(0.);
            self.rates.insert(
                d.id,
                if rate == 0. && d.status != Status::Downloading {
                    0.
                } else {
                    prior * 0.45 + rate * 0.55
                },
            );
        }
        self.sampled_at = Instant::now();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.url.read(cx).value().to_owned();
        match self
            .manager
            .add(&value, self.directory.clone(), self.connections)
        {
            Ok(id) => {
                self.selected = Some(id);
                self.adding = false;
                self.error = None;
                self.view = View::All;
                self.url.update(cx, |input, cx| input.set_value("", cx));
                window.focus(&self.focus);
                self.refresh();
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
        cx.notify();
    }

    fn show_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.view = View::All;
        self.adding = true;
        self.error = None;
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text())
            && strata::protocol::Source::parse(&text, Default::default()).is_ok()
        {
            self.url
                .update(cx, |input, cx| input.set_value(text.trim(), cx));
        }
        self.url.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn choose_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose download folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.first()
            {
                let path = path.clone();
                let _ = this.update(cx, |this, cx| {
                    this.directory = path.clone();
                    this.manager.settings.lock().unwrap().directory = path;
                    this.manager.persist();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn set_connections(&mut self, n: usize, cx: &mut Context<Self>) {
        self.connections = n;
        self.manager.settings.lock().unwrap().connections = n;
        self.manager.persist();
        cx.notify();
    }

    fn save_transport(&mut self, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            let value = self.proxy.read(cx).value().trim();
            let old = self.manager.settings.lock().unwrap().transport.clone();
            let proxy = if value.is_empty() {
                None
            } else if old.proxy.as_ref().is_some_and(|p| p.url == value) {
                old.proxy
            } else {
                Some(strata::protocol::Proxy::parse(value)?)
            };
            let path = |input: &Entity<TextInput>| -> anyhow::Result<Option<PathBuf>> {
                let value = input.read(cx).value().trim();
                if value.is_empty() {
                    return Ok(None);
                }
                let path = std::path::absolute(value)?;
                anyhow::ensure!(path.is_file(), "Choose an existing SSH configuration file");
                Ok(Some(path))
            };
            let options = strata::protocol::TransferOptions {
                proxy,
                identity: path(&self.identity)?,
                known_hosts: path(&self.known_hosts)?,
                ..Default::default()
            };
            self.proxy.update(cx, |input, cx| {
                input.set_value(
                    options.proxy.as_ref().map(|p| p.url.as_str()).unwrap_or(""),
                    cx,
                )
            });
            self.manager.settings.lock().unwrap().transport = options;
            self.manager.persist();
            if let Some(error) = self.manager.persistence_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            Ok(())
        })();
        self.transport_notice = Some(match result {
            Ok(()) => "Saved. New transfers use these settings.".into(),
            Err(error) => format!("{error:#}"),
        });
        cx.notify();
    }

    fn choose_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a torrent or Metalink descriptor".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.first()
            {
                let path = path.display().to_string();
                let _ = this.update(cx, |this, cx| {
                    this.url.update(cx, |input, cx| input.set_value(&path, cx));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn filtered(&self, cx: &App) -> Vec<Download> {
        let query = self.search.read(cx).value().to_lowercase();
        self.downloads
            .iter()
            .rev()
            .filter(|d| {
                let view = match self.view {
                    View::All => true,
                    View::Active => d.status.active() || d.status == Status::Queued,
                    View::Finished => d.status == Status::Complete,
                    View::Paused => matches!(
                        d.status,
                        Status::Paused | Status::Failed | Status::Cancelled
                    ),
                    View::Settings => false,
                };
                view && (d.name.to_lowercase().contains(&query) || d.host().contains(&query))
            })
            .cloned()
            .collect()
    }

    fn topbar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(64.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px(px(28.))
            .border_b_1()
            .border_color(rgb(RULE))
            .child(
                button("top-filter", "Filter", Icon::Filter, false).on_click(cx.listener(
                    |this, event: &ClickEvent, window, cx| {
                        this.show_menu(Menu::Filters, event.position(), window, cx);
                    },
                )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        button("top-settings", "Settings", Icon::Settings, false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                this.view = if this.view == View::Settings {
                                    View::All
                                } else {
                                    View::Settings
                                };
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button("top-add", "New download", Icon::Plus, true)
                            .on_click(cx.listener(|this, _, window, cx| this.show_add(window, cx))),
                    ),
            )
    }

    fn show_menu(
        &mut self,
        menu: Menu,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = Some((menu, position));
        self.menu_index = 0;
        self.menu_focus.focus(window);
        cx.notify();
    }

    fn menu_items(&self, menu: Menu) -> Vec<(&'static str, Icon, MenuAction)> {
        match menu {
            Menu::Filters => vec![
                (
                    "All downloads",
                    Icon::Download,
                    MenuAction::Filter(View::All),
                ),
                ("In progress", Icon::Play, MenuAction::Filter(View::Active)),
                ("Completed", Icon::Check, MenuAction::Filter(View::Finished)),
                ("Stopped", Icon::Pause, MenuAction::Filter(View::Paused)),
            ],
            Menu::Download(id) => {
                let Some(d) = self.downloads.iter().find(|d| d.id == id) else {
                    return vec![];
                };
                let mut items = Vec::new();
                if matches!(
                    d.status,
                    Status::Queued | Status::Connecting | Status::Downloading
                ) {
                    items.push(("Pause", Icon::Pause, MenuAction::Pause(id)));
                } else if matches!(d.status, Status::Paused | Status::Failed) {
                    items.push(("Resume", Icon::Play, MenuAction::Resume(id)));
                }
                if d.status == Status::Complete {
                    items.push(("Open", Icon::Arrow, MenuAction::Open(id)));
                }
                items.push(("Open folder", Icon::Folder, MenuAction::Folder(id)));
                items.push(("Copy download link", Icon::Copy, MenuAction::Copy(id)));
                if !matches!(
                    d.status,
                    Status::Complete | Status::Cancelled | Status::Assembling
                ) {
                    items.push(("Cancel transfer", Icon::Close, MenuAction::Cancel(id)));
                }
                items.push(("Delete", Icon::Trash, MenuAction::Delete(id)));
                items
            }
        }
    }

    fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            MenuAction::Filter(view) => self.view = view,
            MenuAction::Pause(id) => self.manager.pause(id),
            MenuAction::Resume(id) => self.manager.resume(id),
            MenuAction::Cancel(id) => self.manager.cancel(id),
            MenuAction::Delete(id) => {
                self.manager.remove(id);
                self.rates.remove(&id);
                self.previous.remove(&id);
            }
            MenuAction::Folder(id) | MenuAction::Open(id) | MenuAction::Copy(id) => {
                if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                    match action {
                        MenuAction::Folder(_) => reveal(&d.directory),
                        MenuAction::Open(_) => reveal(&d.path()),
                        _ => cx.write_to_clipboard(ClipboardItem::new_string(d.url.clone())),
                    }
                }
            }
        }
        self.menu = None;
        self.refresh();
        self.focus.focus(window);
        cx.notify();
    }

    fn popup_menu(
        &self,
        menu: Menu,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let items = self.menu_items(menu);
        deferred(
            anchored()
                .position(position)
                .anchor(Corner::TopRight)
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .id("popup-menu")
                        .occlude()
                        .w(px(232.))
                        .p(px(4.))
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(rgb(0x44484c))
                        .flex()
                        .flex_col()
                        .track_focus(&self.menu_focus)
                        .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                            this.menu = None;
                            this.focus.focus(window);
                            cx.notify();
                        }))
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            let Some((menu, _)) = this.menu else {
                                return;
                            };
                            let items = this.menu_items(menu);
                            if items.is_empty() {
                                return;
                            }
                            match event.keystroke.key.as_str() {
                                "down" => this.menu_index = (this.menu_index + 1) % items.len(),
                                "up" => {
                                    this.menu_index =
                                        (this.menu_index.min(items.len() - 1) + items.len() - 1)
                                            % items.len()
                                }
                                "enter" | "space" => {
                                    if let Some((_, _, action)) = items.get(this.menu_index) {
                                        this.run_menu_action(*action, window, cx);
                                    }
                                }
                                "escape" => {
                                    this.menu = None;
                                    this.focus.focus(window);
                                }
                                _ => return,
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .children(items.into_iter().enumerate().map(
                            |(i, (text, glyph, action))| {
                                let chosen =
                                    matches!(action, MenuAction::Filter(view) if self.view == view);
                                div()
                                    .id(ElementId::Name(format!("menu-item-{i}").into()))
                                    .h(px(36.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .cursor_pointer()
                                    .when(i == self.menu_index, |d| d.bg(rgb(RAISED)))
                                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                        if this.menu_index != i {
                                            this.menu_index = i;
                                            cx.notify();
                                        }
                                    }))
                                    .child(icon(
                                        glyph,
                                        15.,
                                        if matches!(action, MenuAction::Delete(_)) {
                                            RED
                                        } else {
                                            MUTED
                                        },
                                    ))
                                    .child(label(text, 13., TEXT).flex_1())
                                    .when(chosen, |d| d.child(icon(Icon::Check, 14., BLUE)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.run_menu_action(action, window, cx);
                                    }))
                            },
                        )),
                ),
        )
    }

    fn finish_resize(&mut self) {
        if self.resizing.take().is_some() {
            self.manager.settings.lock().unwrap().details_height = self.details_height;
            self.manager.persist();
        }
    }

    fn queue(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let title = match self.view {
            View::Active => "In progress",
            View::Finished => "Completed",
            View::Paused => "Stopped",
            _ => "All downloads",
        };
        let files = self.filtered(cx);
        let speed: f64 = self.rates.values().sum();
        let active = self.downloads.iter().filter(|d| d.status.active()).count();
        let received: u64 = self.downloads.iter().map(|d| d.downloaded).sum();
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .px(px(28.))
            .pt(px(16.))
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(label(title, 23., TEXT).font_weight(FontWeight::SEMIBOLD))
                            .child(label(format!("{}", files.len()), 13., FAINT)),
                    )
                    .child(
                        button("pause-all", "Pause all", Icon::Pause, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.manager.pause_all();
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(64.))
                    .flex_shrink_0()
                    .bg(rgb(SURFACE))
                    .px(px(16.))
                    .gap(px(26.))
                    .child(metric(
                        "Transfer speed",
                        &format!("{}/s", bytes(speed as u64)),
                        BLUE,
                    ))
                    .child(div().w(px(1.)).h(px(42.)).bg(rgb(RULE)))
                    .child(metric("Active transfers", &active.to_string(), TEXT))
                    .child(div().w(px(1.)).h(px(42.)).bg(rgb(RULE)))
                    .child(metric("Total received", &bytes(received), TEXT)),
            )
            .when(self.adding, |d| d.child(self.add_form(window, cx)))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(12.))
                    .justify_between()
                    .child(
                        div()
                            .w(px(260.))
                            .h(px(36.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(10.))
                            .bg(rgb(SURFACE))
                            .child(icon(Icon::Search, 16., FAINT))
                            .child(self.search.clone()),
                    )
                    .child(label("Newest first", 12., FAINT)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(rgb(RULE))
                    .child(
                        table_row()
                            .h(px(32.))
                            .flex_shrink_0()
                            .child(label("File name", 12., FAINT).flex_1().min_w_0())
                            .child(
                                label("Size", 12., FAINT)
                                    .w(px(84.))
                                    .text_right()
                                    .flex_shrink_0(),
                            )
                            .child(label("Status", 12., FAINT).w(px(100.)).flex_shrink_0())
                            .child(label("Progress", 12., FAINT).w(px(132.)).flex_shrink_0())
                            .when(!self.compact, |d| {
                                d.child(
                                    label("Speed", 12., FAINT)
                                        .w(px(84.))
                                        .text_right()
                                        .flex_shrink_0(),
                                )
                            })
                            .child(div().w(px(30.)).flex_shrink_0()),
                    )
                    .child(
                        div()
                            .id("download-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .when(files.is_empty(), |d| d.child(self.empty(cx)))
                            .children(files.iter().map(|d| self.row(d, cx))),
                    ),
            )
            .child(
                div()
                    .h(px(32.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(rgb(RULE))
                    .child(label(
                        format!(
                            "{} {}",
                            files.len(),
                            if files.len() == 1 {
                                "download"
                            } else {
                                "downloads"
                            }
                        ),
                        12.,
                        FAINT,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(icon(Icon::Folder, 14., FAINT))
                            .child(label(short_path(&self.directory), 12., FAINT).truncate()),
                    ),
            )
    }

    fn empty(&self, cx: &mut Context<Self>) -> Div {
        let searching = !self.search.read(cx).value().is_empty();
        let (title, subtitle) = if searching {
            ("No matching downloads", "Try another file name or website.")
        } else if self.downloads.is_empty() {
            (
                "Make room for what’s next.",
                "Add a link. Strata takes care of the transfer.",
            )
        } else {
            (
                "Nothing here just yet",
                "Downloads in this category will appear here.",
            )
        };
        div()
            .w_full()
            .h_full()
            .min_h(px(160.))
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap(px(14.))
            .child(div().mb(px(6.)).child(icon(Icon::Download, 44., FAINT)))
            .child(label(title, 20., TEXT).font_weight(FontWeight::MEDIUM))
            .child(label(subtitle, 13., MUTED))
            .when(!searching, |d| {
                d.child(
                    button("empty-add", "Add your first link", Icon::Plus, true)
                        .mt(px(10.))
                        .on_click(cx.listener(|this, _, window, cx| this.show_add(window, cx))),
                )
            })
    }

    fn row(&self, d: &Download, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = d.id;
        let selected = self.selected == Some(id);
        let rate = self.rates.get(&id).copied().unwrap_or(0.);
        let color = status_color(d.status);
        table_row()
            .id(ElementId::Name(format!("download-{id}").into()))
            .h(px(48.))
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(SURFACE)))
            .hover(|s| s.bg(rgb(SURFACE)))
            .focusable()
            .focus(|s| s.bg(rgb(RAISED)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(icon(Icon::File, 18., if selected { TEXT } else { FAINT }))
                    .child(
                        label(d.name.clone(), 14., TEXT)
                            .flex_1()
                            .min_w_0()
                            .truncate(),
                    ),
            )
            .child(
                label(d.total.map(bytes).unwrap_or_else(|| "—".into()), 12., MUTED)
                    .w(px(84.))
                    .text_right()
                    .flex_shrink_0(),
            )
            .child(
                label(d.status.label(), 12., color)
                    .w(px(100.))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .w(px(132.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(div().flex().items_center().justify_between().child(label(
                        if d.total.is_some() {
                            format!("{:.0}%", d.fraction() * 100.)
                        } else {
                            "—".into()
                        },
                        11.,
                        FAINT,
                    )))
                    .child(progress(d.fraction(), color)),
            )
            .when(!self.compact, |row| {
                row.child(
                    label(
                        if rate > 0. {
                            format!("{}/s", bytes(rate as u64))
                        } else {
                            "—".into()
                        },
                        12.,
                        MUTED,
                    )
                    .w(px(84.))
                    .text_right()
                    .flex_shrink_0(),
                )
            })
            .child(
                icon_button(format!("menu-{id}"), Icon::More).on_click(cx.listener(
                    move |this, event: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.selected = Some(id);
                        this.show_menu(Menu::Download(id), event.position(), window, cx);
                    },
                )),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id);
                cx.notify();
            }))
    }

    fn add_form(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        div()
            .p(px(18.))
            .bg(rgb(SURFACE))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(label("New download", 15., TEXT).font_weight(FontWeight::MEDIUM))
                    .child(icon_button("close-add", Icon::Close).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.adding = false;
                            this.error = None;
                            window.focus(&this.focus);
                            cx.notify();
                        },
                    ))),
            )
            .child(label("Download link", 12., MUTED))
            .child(
                div()
                    .h(px(42.))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .gap(px(10.))
                    .bg(rgb(BG))
                    .border_1()
                    .border_color(rgb(0x50565c))
                    .child(icon(Icon::Link, 17., FAINT))
                    .child(self.url.clone()),
            )
            .children(self.error.as_ref().map(|e| label(e.clone(), 12., RED)))
            .child(
                button(
                    "choose-descriptor",
                    "Open torrent / Metalink file",
                    Icon::Folder,
                    false,
                )
                .on_click(cx.listener(|this, _, window, cx| this.choose_source(window, cx))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(icon(Icon::Folder, 15., FAINT))
                            .child(label(short_path(&self.directory), 12., MUTED).truncate()),
                    )
                    .child(
                        button("start-download", "Start download", Icon::Download, true)
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
            .child(label(
                format!(
                    "Up to {} connections · Enter to start · Esc to close",
                    self.connections
                ),
                11.,
                FAINT,
            ))
    }

    fn details_resizer(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("details-resize")
            .h(px(8.))
            .w_full()
            .flex_shrink_0()
            .cursor_ns_resize()
            .flex()
            .items_center()
            .justify_center()
            .hover(|s| s.bg(rgb(RAISED)))
            .focusable()
            .focus(|s| s.bg(rgb(RAISED)))
            .child(div().w(px(40.)).h(px(2.)).bg(rgb(FAINT)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.resizing = Some((event.position.y, this.details_height));
                    this.menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let change = match event.keystroke.key.as_str() {
                    "up" => 16.,
                    "down" => -16.,
                    _ => return,
                };
                this.details_height = clamp_details_height(
                    this.details_height + change,
                    window.viewport_size().height.into(),
                );
                this.manager.settings.lock().unwrap().details_height = this.details_height;
                this.manager.persist();
                cx.stop_propagation();
                cx.notify();
            }))
    }

    fn inspector(&self, cx: &mut Context<Self>) -> Div {
        let selected = self.downloads.iter().find(|d| Some(d.id) == self.selected);
        let panel = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(RULE))
            .flex()
            .flex_col();
        let Some(d) = selected else {
            return panel
                .h(px(self.details_height))
                .child(self.details_resizer(cx))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .px(px(28.))
                        .child(label("Click on a download to view more info.", 13., MUTED)),
                );
        };
        let id = d.id;
        let rate = self.rates.get(&id).copied().unwrap_or(0.);
        let eta = if rate > 0. {
            d.total
                .map(|n| duration(n.saturating_sub(d.downloaded) as f64 / rate))
        } else {
            None
        };
        let actions =
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .when(d.status == Status::Complete, |actions| {
                    actions.child(
                        button(
                            "open-file",
                            if d.protocol.folder_output() {
                                "Open download folder"
                            } else {
                                "Open file"
                            },
                            Icon::Arrow,
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, _| {
                            if let Some(d) = this.downloads.iter().find(|d| d.id == id) {
                                reveal(&d.path());
                            }
                        })),
                    )
                })
                .when(
                    matches!(d.status, Status::Failed | Status::Paused),
                    |actions| {
                        actions.child(
                            button("resume-selected", "Resume download", Icon::Play, true)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.manager.resume(id);
                                    cx.notify();
                                })),
                        )
                    },
                )
                .when(d.status.active() || d.status == Status::Queued, |actions| {
                    actions.child(
                        button("pause-selected", "Pause download", Icon::Pause, true).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.manager.pause(id);
                                cx.notify();
                            }),
                        ),
                    )
                })
                .when(
                    !matches!(
                        d.status,
                        Status::Complete | Status::Cancelled | Status::Assembling
                    ),
                    |actions| {
                        actions.child(
                            button("cancel-selected", "Cancel transfer", Icon::Close, false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.manager.cancel(id);
                                    cx.notify();
                                })),
                        )
                    },
                );
        panel
            .h(px(self.details_height))
            .child(self.details_resizer(cx))
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .px(px(28.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(RULE))
                    .child(label("Transfer details", 13., MUTED))
                    .child(actions),
            )
            .child(div().px(px(28.)).py(px(12.)).child(segmented_progress(d)))
            .child(
                div()
                    .id("details-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .p(px(20.))
                            .px(px(28.))
                            .flex()
                            .items_start()
                            .gap(px(28.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .child(
                                        label(d.name.clone(), 15., TEXT)
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate(),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(label(
                                                d.status.label(),
                                                12.,
                                                status_color(d.status),
                                            ))
                                            .child(label(
                                                if d.total.is_some() {
                                                    format!("{:.1}%", d.fraction() * 100.)
                                                } else {
                                                    "—".into()
                                                },
                                                14.,
                                                TEXT,
                                            )),
                                    )
                                    .child(label(
                                        format!(
                                            "{} of {}",
                                            bytes(d.downloaded),
                                            d.total
                                                .map(bytes)
                                                .unwrap_or_else(|| "unknown size".into())
                                        ),
                                        12.,
                                        MUTED,
                                    ))
                                    .child(label(
                                        eta.map(|eta| format!("{eta} left"))
                                            .unwrap_or_else(|| "—".into()),
                                        12.,
                                        FAINT,
                                    ))
                                    .children(
                                        d.error.as_ref().map(|error| {
                                            label(error, 12., RED).line_height(px(20.))
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .child(detail_line("Protocol", d.protocol.label()))
                                    .child(detail_line("Source", &d.host()))
                                    .child(detail_line(
                                        "Destination",
                                        &d.path().display().to_string(),
                                    ))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap(px(8.))
                                            .child(
                                                button(
                                                    "copy-link",
                                                    "Copy download link",
                                                    Icon::Copy,
                                                    false,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(d) =
                                                        this.downloads.iter().find(|d| d.id == id)
                                                    {
                                                        cx.write_to_clipboard(
                                                            ClipboardItem::new_string(
                                                                d.url.clone(),
                                                            ),
                                                        );
                                                    }
                                                })),
                                            )
                                            .child(
                                                button(
                                                    "show-file",
                                                    "Open folder",
                                                    Icon::Folder,
                                                    false,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(d) =
                                                        this.downloads.iter().find(|d| d.id == id)
                                                    {
                                                        reveal(&d.directory);
                                                    }
                                                    cx.notify();
                                                })),
                                            ),
                                    ),
                            ),
                    ),
            )
    }

    fn settings(&self, cx: &mut Context<Self>) -> Div {
        div().flex_1().min_h_0().min_w_0().child(div().id("settings-scroll").size_full().overflow_y_scroll().child(div().p(px(32.)).flex().flex_col().gap(px(24.))
            .child(label("Settings", 24., TEXT).font_weight(FontWeight::SEMIBOLD))
            .child(label("A few good defaults. The rest is automatic.", 14., MUTED))
            .child(div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Connections & authentication", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("Proxy settings apply to HTTP, HTTPS, WebDAV, Metalink HTTP mirrors, and video transfers. FTP, SFTP, and torrent peers use direct connections. URL passwords are protected by Windows.", 13., MUTED).max_w(px(620.)))
                .child(transport_input("HTTP / HTTPS / SOCKS5 proxy", self.proxy.clone()))
                .child(transport_input("SFTP private key (optional)", self.identity.clone()))
                .child(transport_input("SFTP known_hosts file (optional)", self.known_hosts.clone()))
                .child(label("SFTP refuses unknown or changed host keys. Verify the host with OpenSSH first. Encrypted keys can use strata-cli --key-passphrase-stdin.", 12., FAINT).max_w(px(620.)))
                .child(button("save-transport", "Save connection settings", Icon::Link, false).on_click(cx.listener(|this, _, _, cx| this.save_transport(cx))))
                .children(self.transport_notice.as_ref().map(|message| label(message, 12., MUTED))))
            .child(div().mt(px(10.)).p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Download folder", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("New downloads are saved here. Existing transfers keep their destination.", 13., MUTED))
                .child(label(self.directory.display().to_string(), 13., TEXT))
                .child(button("choose-folder", "Choose folder", Icon::Folder, false).on_click(cx.listener(|this, _, window, cx| this.choose_directory(window, cx)))))
            .child(div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Connections per download", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("More connections can help on some servers. Strata uses fewer for small files and servers that need a single connection.", 13., MUTED).max_w(px(620.)))
                .child(div().flex().gap(px(10.)).children([1, 4, 8, 16].into_iter().map(|n| {
                    div().id(ElementId::Name(format!("connections-{n}").into())).w(px(64.)).h(px(36.)).flex().items_center().justify_center().cursor_pointer()
                        .bg(rgb(if self.connections == n { TEXT } else { RAISED })).text_color(rgb(if self.connections == n { BG } else { MUTED }))
                        .focusable().focus(|s| s.border_1().border_color(rgb(BLUE))).hover(|s| s.opacity(0.85)).child(n.to_string())
                        .on_click(cx.listener(move |this, _, _, cx| this.set_connections(n, cx)))
                }))))
            .child(div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Browser companion", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("Capture file downloads in Chromium browsers and send them straight to Strata. The browser extension controls when capture is on.", 13., MUTED).max_w(px(620.)))
                .child(label(if self.browser_registered { "Local bridge registered" } else { "Connect the app, then load the extension in your browser." }, 12., if self.browser_registered { BLUE } else { FAINT }))
                .child(div().flex().gap(px(12.))
                    .child(button("connect-chrome", if self.browser_registered { "Reconnect browsers" } else { "Connect browsers" }, Icon::Link, true).on_click(cx.listener(|this, _, _, cx| {
                        match strata::bridge::register_browsers() {
                            Ok(()) => { this.browser_registered = true; this.browser_notice = Some("Connected. Load the extension folder from your browser extensions page to finish setup.".into()); },
                            Err(error) => this.browser_notice = Some(format!("Could not connect: {error:#}")),
                        } cx.notify();
                    })))
                    .child(button("extension-folder", "Extension folder", Icon::Folder, false).on_click(|_, _, _| reveal(&strata::bridge::extension_directory()))))
                .children(self.browser_notice.as_ref().map(|message| label(message, 12., MUTED)))
                .children(self.manager.browser_error.lock().unwrap().as_ref().map(|message| label(message, 12., RED))))
            .child(label("Up to 3 downloads run at once. Partial files and history are saved automatically.", 12., FAINT))
            .when(self.tray.is_some(), |view| view.child(label("Closing the window keeps downloads running in the system tray. Right-click the tray icon and choose Quit Strata to exit.", 12., FAINT)))
        ))
    }
}

impl Render for Strata {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.compact = window.viewport_size().width < px(1200.);
        self.details_height =
            clamp_details_height(self.details_height, window.viewport_size().height.into());
        // Native DPI stays in GPUI; there is no texture scaling or translucent text container.
        div()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font(Font {
                family: "Segoe UI".into(),
                features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])),
                ..font("Segoe UI")
            })
            .text_size(px(14.))
            .line_height(px(20.))
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(self.resizing.is_some(), |d| d.cursor_ns_resize())
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if let Some((start_y, start_height)) = this.resizing {
                    if event.dragging() {
                        let delta: f32 = (start_y - event.position.y).into();
                        this.details_height = clamp_details_height(
                            start_height + delta,
                            window.viewport_size().height.into(),
                        );
                    } else {
                        this.finish_resize();
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.finish_resize()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.finish_resize()),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if event.keystroke.modifiers.control && key == "n" {
                    this.show_add(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.modifiers.control && key == "f" {
                    this.menu = None;
                    this.search.focus_handle(cx).focus(window);
                } else if key == "escape" {
                    if this.menu.take().is_some() {
                        this.focus.focus(window);
                        cx.notify();
                        return;
                    }
                    this.adding = false;
                    this.error = None;
                    window.focus(&this.focus);
                    cx.notify();
                } else if key == "enter"
                    && this.adding
                    && this.url.focus_handle(cx).is_focused(window)
                {
                    this.add(window, cx);
                } else if key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                }
            }))
            .child(self.titlebar(window, cx))
            .child(
                div().flex_1().min_h_0().w_full().flex().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .when(self.view == View::Settings, |d| {
                            d.child(self.topbar(cx)).child(self.settings(cx))
                        })
                        .when(self.view != View::Settings, |d| {
                            d.child(self.topbar(cx))
                                .child(self.queue(window, cx))
                                .child(self.inspector(cx))
                        })
                        .children(
                            self.manager
                                .persistence_error
                                .lock()
                                .unwrap()
                                .clone()
                                .map(|e| label(e, 12., RED).px(px(28.)).py(px(10.))),
                        )
                        .children(
                            self.tray_error
                                .as_ref()
                                .map(|message| label(message, 12., AMBER).px(px(28.)).py(px(10.))),
                        ),
                ),
            )
            .children(
                self.menu
                    .map(|(menu, position)| self.popup_menu(menu, position, cx)),
            )
    }
}

fn label(text: impl AsRef<str>, size: f32, color: u32) -> Div {
    div()
        .text_size(px(size))
        .line_height(px((size * 1.4).round()))
        .text_color(rgb(color))
        .child(crate::text::CrispText(text.as_ref().to_owned().into()))
}

fn metric(title: &str, value: &str, color: u32) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(label(title, 12., FAINT))
        .child(
            label(value, 24., color)
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap(),
        )
}

fn table_row() -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(16.))
        .px(px(14.))
        .border_b_1()
        .border_color(rgb(RULE))
}

fn transport_input(title: &str, input: Entity<TextInput>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(label(title, 12., MUTED))
        .child(
            div()
                .h(px(40.))
                .px(px(12.))
                .flex()
                .items_center()
                .bg(rgb(BG))
                .border_1()
                .border_color(rgb(RULE))
                .child(input),
        )
}

fn detail_line(title: &str, value: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(label(title, 11., FAINT))
        .child(label(value, 12., MUTED).line_height(px(20.)))
}

fn button(id: impl Into<SharedString>, text: &str, glyph: Icon, primary: bool) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .h(px(36.))
        .px(px(13.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .cursor_pointer()
        .bg(rgb(if primary { TEXT } else { RAISED }))
        .text_color(rgb(if primary { BG } else { TEXT }))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.bg(rgb(if primary { 0xd5d8db } else { 0x33363a })))
        .focusable()
        .focus(|s| s.border_1().border_color(rgb(BLUE)))
        .child(icon(glyph, 15., if primary { BG } else { MUTED }))
        .child(text.to_owned())
}

fn icon_button(id: impl Into<SharedString>, glyph: Icon) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(RAISED)))
        .focusable()
        .focus(|s| s.bg(rgb(RAISED)))
        .child(icon(glyph, 16., MUTED))
}

fn window_control(id: &'static str, glyph: Icon, area: WindowControlArea) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(46.))
        .h(px(44.))
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .focusable()
        .focus(|s| s.bg(rgb(RAISED)))
        .hover(|s| {
            s.bg(rgb(if area == WindowControlArea::Close {
                0xb73d49
            } else {
                RAISED
            }))
        })
        .child(icon(glyph, 14., TEXT))
}

fn toggle_maximize(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{SW_MAXIMIZE, SW_RESTORE, ShowWindowAsync},
    };
    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        // GPUI 0.2.2's zoom() only maximizes on Windows; explicitly restore the saved native placement.
        unsafe {
            let _ = ShowWindowAsync(
                HWND(handle.hwnd.get() as *mut _),
                if window.is_maximized() {
                    SW_RESTORE
                } else {
                    SW_MAXIMIZE
                },
            );
        }
    }
}

fn progress(value: f32, color: u32) -> Div {
    div()
        .w_full()
        .h(px(4.))
        .flex_shrink_0()
        .bg(rgb(RAISED))
        .overflow_hidden()
        .child(
            div()
                .w(relative(value.clamp(0., 1.)))
                .h(px(4.))
                .bg(rgb(color)),
        )
}

fn clamp_details_height(requested: f32, viewport: f32) -> f32 {
    let requested = if requested.is_finite() {
        requested
    } else {
        240.
    };
    requested.clamp(120., (viewport - 460.).max(120.))
}

fn segment_layout(parts: &[strata::engine::Part], known_size: bool) -> Vec<(f32, f32)> {
    let total: f64 = parts.iter().map(|part| part.len() as f64).sum();
    parts
        .iter()
        .map(|part| {
            (
                (part.len() as f64 / total) as f32,
                if known_size { part.fraction() } else { 0. },
            )
        })
        .collect()
}

fn segmented_progress(download: &Download) -> Div {
    let segments = if download.parts.is_empty() {
        vec![(1., download.fraction())]
    } else {
        segment_layout(&download.parts, download.total.is_some())
    };
    div()
        .w_full()
        .h(px(16.))
        .flex()
        .overflow_hidden()
        .bg(rgb(RAISED))
        .children(
            segments
                .into_iter()
                .enumerate()
                .map(|(i, (width, filled))| {
                    div()
                        .w(relative(width))
                        .h_full()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .when(i > 0, |d| d.border_l_1().border_color(rgb(BG)))
                        .child(
                            div()
                                .h_full()
                                .w(relative(if download.status == Status::Complete {
                                    1.
                                } else {
                                    filled
                                }))
                                .bg(rgb(status_color(download.status))),
                        )
                }),
        )
}

fn status_color(status: Status) -> u32 {
    match status {
        Status::Downloading | Status::Complete => BLUE,
        Status::Paused | Status::Pausing => AMBER,
        Status::Failed => RED,
        _ => FAINT,
    }
}

pub fn bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1073741824. {
        format!("{:.2} GB", n / 1073741824.)
    } else if n >= 1048576. {
        format!("{:.1} MB", n / 1048576.)
    } else if n >= 1024. {
        format!("{:.0} KB", n / 1024.)
    } else {
        format!("{n:.0} B")
    }
}

fn duration(seconds: f64) -> String {
    let s = seconds as u64;
    if s >= 3600 {
        format!("{}h {}m", s / 3600, s / 60 % 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn short_path(path: &std::path::Path) -> String {
    let raw = path.display().to_string();
    std::env::var("USERPROFILE")
        .ok()
        .map_or(raw.clone(), |home| raw.replace(&home, "~"))
}

fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
}

fn icon(glyph: Icon, dimension: f32, color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let lines: Vec<Vec<(f32, f32)>> = match glyph {
                Icon::Download => vec![
                    vec![(12., 3.), (12., 15.)],
                    vec![(7., 10.), (12., 15.), (17., 10.)],
                    vec![(4., 16.), (4., 21.), (20., 21.), (20., 16.)],
                ],
                Icon::Minimize => vec![vec![(4., 12.), (20., 12.)]],
                Icon::Maximize => vec![vec![(5., 5.), (19., 5.), (19., 19.), (5., 19.), (5., 5.)]],
                Icon::Restore => vec![
                    vec![(8., 5.), (8., 3.), (21., 3.), (21., 16.), (19., 16.)],
                    vec![(3., 8.), (16., 8.), (16., 21.), (3., 21.), (3., 8.)],
                ],
                Icon::Play => vec![vec![(8., 4.), (20., 12.), (8., 20.), (8., 4.)]],
                Icon::Pause => vec![vec![(8., 5.), (8., 19.)], vec![(16., 5.), (16., 19.)]],
                Icon::Check => vec![vec![(4., 12.), (9., 17.), (20., 6.)]],
                Icon::Folder => vec![vec![
                    (3., 6.),
                    (9., 6.),
                    (11., 9.),
                    (21., 9.),
                    (21., 20.),
                    (3., 20.),
                    (3., 6.),
                ]],
                Icon::File => vec![
                    vec![
                        (5., 2.),
                        (14., 2.),
                        (20., 8.),
                        (20., 22.),
                        (5., 22.),
                        (5., 2.),
                    ],
                    vec![(14., 2.), (14., 8.), (20., 8.)],
                    vec![(9., 13.), (16., 13.)],
                    vec![(9., 17.), (14., 17.)],
                ],
                Icon::Search => vec![
                    vec![
                        (10., 3.),
                        (15., 5.),
                        (17., 10.),
                        (15., 15.),
                        (10., 17.),
                        (5., 15.),
                        (3., 10.),
                        (5., 5.),
                        (10., 3.),
                    ],
                    vec![(15., 15.), (21., 21.)],
                ],
                Icon::Plus => vec![vec![(12., 4.), (12., 20.)], vec![(4., 12.), (20., 12.)]],
                Icon::Close => vec![vec![(6., 6.), (18., 18.)], vec![(18., 6.), (6., 18.)]],
                Icon::Arrow => vec![
                    vec![(5., 19.), (19., 5.)],
                    vec![(7., 5.), (19., 5.), (19., 17.)],
                ],
                Icon::Link => vec![
                    vec![
                        (10., 6.),
                        (14., 2.),
                        (18., 2.),
                        (22., 6.),
                        (22., 10.),
                        (18., 14.),
                    ],
                    vec![
                        (14., 18.),
                        (10., 22.),
                        (6., 22.),
                        (2., 18.),
                        (2., 14.),
                        (6., 10.),
                    ],
                    vec![(8., 16.), (16., 8.)],
                ],
                Icon::Copy => vec![
                    vec![(8., 8.), (21., 8.), (21., 21.), (8., 21.), (8., 8.)],
                    vec![(4., 16.), (3., 16.), (3., 3.), (16., 3.), (16., 4.)],
                ],
                Icon::Filter => vec![
                    vec![(3., 5.), (21., 5.)],
                    vec![(6., 12.), (18., 12.)],
                    vec![(9., 19.), (15., 19.)],
                ],
                Icon::More => vec![
                    vec![(5., 11.), (5., 13.)],
                    vec![(12., 11.), (12., 13.)],
                    vec![(19., 11.), (19., 13.)],
                ],
                Icon::Trash => vec![
                    vec![(3., 6.), (21., 6.)],
                    vec![(8., 6.), (8., 3.), (16., 3.), (16., 6.)],
                    vec![(5., 6.), (6., 21.), (18., 21.), (19., 6.)],
                    vec![(10., 10.), (10., 17.)],
                    vec![(14., 10.), (14., 17.)],
                ],
                Icon::Settings => vec![
                    vec![(3., 6.), (21., 6.)],
                    vec![(3., 12.), (21., 12.)],
                    vec![(3., 18.), (21., 18.)],
                    vec![(8., 3.), (8., 9.)],
                    vec![(16., 9.), (16., 15.)],
                    vec![(10., 15.), (10., 21.)],
                ],
            };
            for line in lines {
                let mut path = PathBuilder::stroke(px(if dimension > 30. { 1.6 } else { 1.4 }));
                for (i, (x, y)) in line.into_iter().enumerate() {
                    let p = bounds.origin + point(px(x * dimension / 24.), px(y * dimension / 24.));
                    if i == 0 {
                        path.move_to(p);
                    } else {
                        path.line_to(p);
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(color));
                }
            }
        },
    )
    .size(px(dimension))
    .flex_shrink_0()
}

#[cfg(test)]
mod tests {
    use super::{clamp_details_height, segment_layout};
    use strata::engine::Part;

    #[test]
    fn segmented_progress_preserves_byte_positions_for_unequal_parts() {
        let segments = segment_layout(
            &[
                Part {
                    start: 0,
                    end: 99,
                    downloaded: 50,
                },
                Part {
                    start: 100,
                    end: 399,
                    downloaded: 300,
                },
            ],
            true,
        );
        assert_eq!(segments, vec![(0.25, 0.5), (0.75, 1.)]);
        assert!(segment_layout(&[], true).is_empty());
        assert_eq!(
            segment_layout(
                &[Part {
                    start: 0,
                    end: 0,
                    downloaded: 8192
                }],
                false
            ),
            vec![(1., 0.)]
        );
    }

    #[test]
    fn resizing_preserves_space_for_the_table_and_handles_saved_invalid_values() {
        assert_eq!(clamp_details_height(900., 700.), 240.);
        assert_eq!(clamp_details_height(900., 1000.), 540.);
        assert_eq!(clamp_details_height(-100., 700.), 120.);
        assert_eq!(clamp_details_height(f32::NAN, 880.), 240.);
    }
}
