use crate::input::TextInput;
use crate::scroll::{BAR_WIDTH, ScrollState, scroll_area};
use crate::speed::DownloadSpeed;
use crate::torrent_tree::{self, TorrentNode};
use crate::tray::Tray;
use gpui::{prelude::*, *};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use strata::engine::{Download, DownloadChange, Manager, Priority, Status};

const BG: u32 = 0x000000;
const SURFACE: u32 = 0x191919;
const RAISED: u32 = 0x333333;
const RULE: u32 = 0x333333;
const TEXT: u32 = 0xf9f9f9;
const MUTED: u32 = 0xa7a7a7;
const FAINT: u32 = 0xa7a7a7;
const ACCENT: u32 = 0xe85002;
const AMBER: u32 = 0xd9c3ab;
const RED: u32 = 0xf5988f;

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
    Priority(u64),
}

#[derive(Clone)]
enum EditModal {
    Rename(u64),
    SpeedLimit(Vec<u64>),
    GlobalSpeedLimit,
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
    Verify(u64),
    Options(u64),
    Rename(u64),
    SpeedLimit(u64),
    Priority(u64),
    SetPriority(u64, Priority),
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
    Flag,
    Link,
    Copy,
    Filter,
    More,
    Trash,
    ChevronDown,
    ChevronUp,
    ChevronRight,
}

pub struct Strata {
    manager: Arc<Manager>,
    tray: Option<Rc<Tray>>,
    tray_error: Option<String>,
    downloads: Vec<Download>,
    view: View,
    selected: Option<u64>,
    checked: BTreeSet<u64>,
    selection_anchor: Option<u64>,
    edit: Option<EditModal>,
    edit_input: Entity<TextInput>,
    edit_error: Option<String>,
    edit_saving: bool,
    torrent_picker: Option<u64>,
    torrent_checked: BTreeSet<usize>,
    torrent_focus: Vec<FocusHandle>,
    torrent_error: Option<String>,
    torrent_tree: Vec<TorrentNode>,
    torrent_expanded: BTreeSet<usize>,
    url: Entity<TextInput>,
    search: Entity<TextInput>,
    search_query: String,
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
    details_collapsed: bool,
    resizing: Option<(Pixels, f32)>,
    modal_close_focus: FocusHandle,
    modal_choose_focus: FocusHandle,
    modal_start_focus: FocusHandle,
    browser_registered: bool,
    browser_notice: Option<String>,
    update_checking: bool,
    update_available: Option<strata::updates::Update>,
    update_notice: Option<String>,
    directory: PathBuf,
    connections: usize,
    rates: HashMap<u64, DownloadSpeed>,
    queue_scroll: ScrollState,
    settings_scroll: ScrollState,
    add_scroll: ScrollState,
    edit_scroll: ScrollState,
    torrent_scroll: ScrollState,
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
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag)
                    .px(px(28.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(img("brand/mark.png").size(px(25.)))
                    .child(label("101 Downloader", 17., TEXT).font_weight(FontWeight::SEMIBOLD)),
            )
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
        let url =
            cx.new(|cx| TextInput::new("URL, magnet link, torrent or Metalink file", window, cx));
        let search = cx.new(|cx| TextInput::new("Search downloads", window, cx));
        let edit_input = cx.new(|cx| TextInput::new("", window, cx));
        let proxy = cx.new(|cx| {
            TextInput::new(
                "http://, https:// or socks5:// proxy (optional)",
                window,
                cx,
            )
        });
        let identity =
            cx.new(|cx| TextInput::new("Automatic: ~/.ssh/id_ed25519 or id_rsa", window, cx));
        let known_hosts = cx.new(|cx| TextInput::new("Default: ~/.ssh/known_hosts", window, cx));
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
        let observer = cx.observe(&search, |this, _, cx| {
            let query = this.search.read(cx).value().to_owned();
            if query == this.search_query {
                return;
            }
            this.search_query = query;
            if !this.search_query.is_empty() {
                this.view = View::All;
            }
            this.checked.clear();
            this.selection_anchor = None;
            this.selected = None;
            cx.notify();
        });
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
                        this.show_pending_torrent(window, cx);
                        if this.view != View::Settings
                            && !this.tray.as_ref().is_some_and(|tray| tray.is_hidden())
                        {
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
        let mut app = Self {
            manager,
            tray,
            tray_error,
            downloads,
            view: View::All,
            selected,
            checked: selected.into_iter().collect(),
            selection_anchor: selected,
            edit: None,
            edit_input,
            edit_error: None,
            edit_saving: false,
            torrent_picker: None,
            torrent_checked: BTreeSet::new(),
            torrent_focus: Vec::new(),
            torrent_error: None,
            torrent_tree: Vec::new(),
            torrent_expanded: BTreeSet::new(),
            url,
            search,
            search_query: String::new(),
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
            details_collapsed: false,
            resizing: None,
            modal_close_focus: cx.focus_handle(),
            modal_choose_focus: cx.focus_handle(),
            modal_start_focus: cx.focus_handle(),
            browser_registered: strata::bridge::browsers_registered(),
            browser_notice: None,
            update_checking: false,
            update_available: None,
            update_notice: None,
            directory: settings.directory,
            connections: settings.connections,
            rates: HashMap::new(),
            queue_scroll: ScrollState::default(),
            settings_scroll: ScrollState::default(),
            add_scroll: ScrollState::default(),
            edit_scroll: ScrollState::default(),
            torrent_scroll: ScrollState::default(),
            _ticker: ticker,
            _search_observer: observer,
        };
        app.check_for_updates(window, cx);
        app
    }

    fn refresh(&mut self) {
        let previous_id = self.downloads.last().map(|d| d.id);
        self.downloads = self.manager.snapshots();
        self.checked
            .retain(|id| self.downloads.iter().any(|d| d.id == *id));
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
            self.checked = [id].into_iter().collect();
            self.selection_anchor = Some(id);
            self.view = View::All;
        }
        let now = Instant::now();
        for d in &self.downloads {
            self.rates.entry(d.id).or_default().update(
                now,
                d.downloaded,
                d.status == Status::Downloading,
            );
        }
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
        self.finish_resize();
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

    fn show_pending_torrent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.torrent_picker {
            if !self.downloads.iter().any(|d| d.id == id) {
                self.torrent_picker = None;
                self.focus.focus(window);
                cx.notify();
            }
            return;
        }
        if self.adding || self.edit.is_some() || self.menu.is_some() {
            return;
        }
        if let Some(id) = self
            .downloads
            .iter()
            .find(|d| d.status == Status::SelectingFiles)
            .map(|download| download.id)
        {
            self.open_torrent_options(id, window, cx);
        }
    }

    fn open_torrent_options(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(download) = self.downloads.iter().find(|d| d.id == id) {
            self.menu = None;
            self.torrent_picker = Some(id);
            self.torrent_checked = download
                .selected_files
                .as_ref()
                .map(|files| files.iter().copied().collect())
                .unwrap_or_else(|| (0..download.torrent_files.len()).collect());
            let (tree, nodes) = torrent_tree::tree(&download.torrent_files);
            self.torrent_expanded = tree
                .iter()
                .filter(|node| !node.children.is_empty())
                .map(|node| node.id)
                .collect();
            self.torrent_tree = tree;
            self.torrent_focus = (0..=nodes).map(|_| cx.focus_handle()).collect();
            self.torrent_error = None;
            self.torrent_scroll = ScrollState::default();
            self.torrent_focus[0].focus(window);
            cx.notify();
        }
    }

    fn close_torrent_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.torrent_picker.take() {
            self.manager.pause(id);
            self.manager.persist();
        }
        self.torrent_error = None;
        self.torrent_focus.clear();
        self.focus.focus(window);
        self.refresh();
        cx.notify();
    }

    fn start_torrent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.torrent_picker else {
            return;
        };
        match self
            .manager
            .select_torrent_files(id, self.torrent_checked.iter().copied().collect())
        {
            Ok(()) => {
                self.torrent_picker = None;
                self.torrent_error = None;
                self.torrent_focus.clear();
                self.selected = Some(id);
                self.checked = [id].into_iter().collect();
                self.view = View::All;
                self.focus.focus(window);
                self.refresh();
            }
            Err(error) => self.torrent_error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    fn toggle_torrent_node(&mut self, id: Option<usize>, cx: &mut Context<Self>) {
        let files = if let Some(id) = id {
            self.torrent_tree
                .iter()
                .find_map(|node| node.find(id))
                .unwrap()
                .files
                .clone()
        } else {
            (0..self
                .downloads
                .iter()
                .find(|d| Some(d.id) == self.torrent_picker)
                .unwrap()
                .torrent_files
                .len())
                .collect()
        };
        torrent_tree::toggle(&files, &mut self.torrent_checked);
        if let Some(selected) = self
            .downloads
            .iter()
            .find(|d| Some(d.id) == self.torrent_picker)
            .and_then(|download| download.selected_files.as_ref())
        {
            self.torrent_checked.extend(selected);
        }
        self.torrent_error = None;
        cx.notify();
    }

    fn expand_torrent_node(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.torrent_expanded.remove(&id) {
            self.torrent_expanded.insert(id);
        }
        self.torrent_focus[id + 1].focus(window);
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
                    View::Finished => matches!(d.status, Status::Complete | Status::Missing),
                    View::Paused => {
                        matches!(
                            d.status,
                            Status::Paused | Status::Failed | Status::Cancelled
                        )
                    }
                    View::Settings => false,
                };
                view && (d.name.to_lowercase().contains(&query) || d.host().contains(&query))
            })
            .cloned()
            .collect()
    }

    fn toolbar_actions(&self) -> Vec<(&'static str, Icon, MenuAction)> {
        let Some(id) = self
            .selected
            .filter(|id| self.checked.contains(id))
            .or_else(|| self.checked.iter().next().copied())
        else {
            return vec![];
        };
        if self.checked.len() == 1 {
            return self.menu_items(Menu::Download(id));
        }
        let selected = self
            .downloads
            .iter()
            .filter(|d| self.checked.contains(&d.id))
            .collect::<Vec<_>>();
        let mut actions = Vec::new();
        if selected.iter().any(|d| {
            matches!(
                d.status,
                Status::Queued | Status::Connecting | Status::Downloading
            )
        }) {
            actions.push(("Pause selected", Icon::Pause, MenuAction::Pause(id)));
        }
        if selected
            .iter()
            .any(|d| !d.owns_output && matches!(d.status, Status::Paused | Status::Failed))
        {
            actions.push(("Resume selected", Icon::Play, MenuAction::Resume(id)));
        }
        actions.push(("Speed limit", Icon::Settings, MenuAction::SpeedLimit(id)));
        actions.push(("Set priority", Icon::Flag, MenuAction::Priority(id)));
        if selected.iter().any(|d| d.can_verify_integrity()) {
            actions.push(("Verify file integrity", Icon::Check, MenuAction::Verify(id)));
        }
        if selected.iter().any(|d| {
            !matches!(
                d.status,
                Status::Complete
                    | Status::Missing
                    | Status::Cancelled
                    | Status::Assembling
                    | Status::Verifying
                    | Status::Deleting
            ) && !d.owns_output
        }) {
            actions.push(("Cancel selected", Icon::Close, MenuAction::Cancel(id)));
        }
        if selected.iter().any(|d| d.status != Status::Deleting) {
            actions.push((
                "Delete selected from device",
                Icon::Trash,
                MenuAction::Delete(id),
            ));
        }
        actions
    }

    fn topbar(&self, cx: &mut Context<Self>) -> Div {
        let settings_open = self.view == View::Settings;
        let actions = self.toolbar_actions();
        div()
            .h(px(60.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(28.))
            .border_b_1()
            .border_color(rgb(RULE))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .when(self.checked.is_empty(), |row| {
                        row.child(
                            button("pause-all", "Pause all", Icon::Pause, false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.manager.pause_all();
                                    cx.notify();
                                }),
                            ),
                        )
                    })
                    .children(actions.into_iter().map(|(text, glyph, action)| {
                        icon_button(format!("selection-action-{text}"), glyph)
                            .tooltip(move |_, cx| cx.new(|_| ActionTooltip(text)).into())
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                if let MenuAction::Priority(id) = action {
                                    this.show_menu(
                                        Menu::Priority(id),
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                } else {
                                    this.run_menu_action(action, window, cx);
                                }
                            }))
                    })),
            )
            .child(
                div()
                    .w(px(if self.compact { 240. } else { 280. }))
                    .h(px(36.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .bg(rgb(SURFACE))
                    .child(icon(Icon::Search, 16., MUTED))
                    .child(self.search.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("top-filter", "Filter", Icon::Filter, false).on_click(cx.listener(
                            |this, event: &ClickEvent, window, cx| {
                                this.show_menu(Menu::Filters, event.position(), window, cx);
                            },
                        )),
                    )
                    .child(
                        button("top-settings", "Settings", Icon::Settings, false)
                            .border_1()
                            .border_color(rgb(if settings_open { ACCENT } else { RAISED }))
                            .focus(move |style| {
                                style.bg(rgb(0x444444)).border_color(rgb(if settings_open {
                                    ACCENT
                                } else {
                                    RAISED
                                }))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                this.view = if this.view == View::Settings {
                                    View::All
                                } else {
                                    View::Settings
                                };
                                cx.notify();
                            })),
                    )
                    .child(
                        button("top-add", "New download", Icon::Plus, true)
                            .on_click(cx.listener(|this, _, window, cx| this.show_add(window, cx))),
                    ),
            )
    }

    fn action_ids(&self, id: u64) -> Vec<u64> {
        if self.checked.contains(&id) {
            self.checked.iter().copied().collect()
        } else {
            vec![id]
        }
    }

    fn select_row(&mut self, id: u64, control: bool, shift: bool, cx: &mut Context<Self>) {
        let visible = self.filtered(cx).iter().map(|d| d.id).collect::<Vec<_>>();
        select_download(
            &mut self.checked,
            &mut self.selection_anchor,
            &visible,
            id,
            control,
            shift,
        );
        self.selected = self
            .checked
            .contains(&id)
            .then_some(id)
            .or_else(|| self.checked.iter().next_back().copied());
        cx.notify();
    }

    fn open_edit(&mut self, modal: EditModal, window: &mut Window, cx: &mut Context<Self>) {
        let (value, end) = match &modal {
            EditModal::Rename(id) => {
                let Some(download) = self.downloads.iter().find(|d| d.id == *id) else {
                    return;
                };
                (
                    download.name.clone(),
                    filename_selection_end(&download.name, download.protocol.folder_output()),
                )
            }
            EditModal::SpeedLimit(ids) => {
                let Some(download) = self.downloads.iter().find(|d| ids.first() == Some(&d.id))
                else {
                    return;
                };
                let value = (download.speed_limit / 1024).to_string();
                let end = value.len();
                (value, end)
            }
            EditModal::GlobalSpeedLimit => {
                let value = (self.manager.settings.lock().unwrap().speed_limit / 1024).to_string();
                let end = value.len();
                (value, end)
            }
        };
        self.edit_input.update(cx, |input, cx| {
            input.set_value(&value, cx);
            input.select_prefix(end, cx);
        });
        self.adding = false;
        self.menu = None;
        self.edit_error = None;
        self.edit = Some(modal);
        self.edit_input.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn close_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.edit_saving {
            self.edit = None;
            self.edit_error = None;
            self.focus.focus(window);
            cx.notify();
        }
    }

    fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_saving {
            return;
        }
        let Some(modal) = self.edit.clone() else {
            return;
        };
        let value = self.edit_input.read(cx).value().to_owned();
        let tasks = match modal {
            EditModal::Rename(id) => vec![(
                id,
                self.manager
                    .change_download(id, DownloadChange::Rename(value)),
            )],
            EditModal::SpeedLimit(ids) => {
                let limit = match parse_speed_limit(&value) {
                    Ok(limit) => limit,
                    Err(error) => {
                        self.edit_error = Some(error);
                        cx.notify();
                        return;
                    }
                };
                ids.into_iter()
                    .map(|id| {
                        (
                            id,
                            self.manager
                                .change_download(id, DownloadChange::SpeedLimit(limit)),
                        )
                    })
                    .collect()
            }
            EditModal::GlobalSpeedLimit => {
                let limit = match parse_speed_limit(&value) {
                    Ok(limit) => limit,
                    Err(error) => {
                        self.edit_error = Some(error);
                        cx.notify();
                        return;
                    }
                };
                vec![(0, self.manager.set_global_speed_limit(limit))]
            }
        };
        self.edit_saving = true;
        self.edit_error = None;
        let names = self
            .downloads
            .iter()
            .map(|d| (d.id, d.name.clone()))
            .collect::<HashMap<_, _>>();
        cx.spawn_in(window, async move |this, cx| {
            let mut errors = Vec::new();
            for (id, task) in tasks {
                let name = names
                    .get(&id)
                    .map(String::as_str)
                    .unwrap_or("Global speed limit");
                match task.await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => errors.push(format!("{name}: {error:#}")),
                    Err(error) => errors.push(format!("{name}: {error}")),
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.edit_saving = false;
                this.refresh();
                if errors.is_empty() {
                    this.close_edit(window, cx);
                } else {
                    this.edit_error = Some(errors.join("\n"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn edit_modal(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rename = matches!(self.edit, Some(EditModal::Rename(_)));
        let global = matches!(self.edit, Some(EditModal::GlobalSpeedLimit));
        let count = match &self.edit {
            Some(EditModal::SpeedLimit(ids)) => ids.len(),
            _ => 1,
        };
        deferred(div().absolute().inset_0().occlude().bg(rgba(0x000000a6)).flex().items_center().justify_center()
            .child(scroll_area("edit-download-modal", &self.edit_scroll,
                div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(16.))
                .child(div().flex().items_center().justify_between()
                    .child(label(if rename { "Rename download" } else if global { "Global speed limit" } else { "Speed limit" }, 18., TEXT).font_weight(FontWeight::MEDIUM))
                    .child(icon_button("close-edit", Icon::Close).track_focus(&self.modal_close_focus)
                        .on_click(cx.listener(|this, _, window, cx| this.close_edit(window, cx)))))
                .child(label(if rename { "Download name".to_owned() } else if global { "Shared limit in KiB/s".into() } else { format!("Limit per download (KiB/s) · {count} selected") }, 13., MUTED))
                .child(div().h(px(42.)).flex().items_center().px(px(12.)).bg(rgb(BG)).border_1().border_color(rgb(0x50565c)).child(self.edit_input.clone()))
                .when(!rename, |form| form.child(label(if global { "0 means unlimited; otherwise use at least 16 KiB/s. All downloads share this cap. FTP, video, torrent and Metalink transfers run one at a time while it is enabled." } else { "0 means unlimited. The limit is shared by all connections in each download. Torrent limits start at 16 KiB/s." }, 12., MUTED)))
                .child(label("Active transfers pause briefly while the change is saved, then resume.", 12., MUTED))
                .children(self.edit_error.as_ref().map(|error| label(error, 12., RED)))
                .child(div().flex().justify_end().gap(px(10.))
                    .child(button("cancel-edit", "Cancel", Icon::Close, false).track_focus(&self.modal_choose_focus)
                        .on_click(cx.listener(|this, _, window, cx| this.close_edit(window, cx))))
                    .child(button("save-edit", if self.edit_saving { "Saving..." } else if rename { "Rename" } else { "Apply limit" }, Icon::Check, true)
                        .track_focus(&self.modal_start_focus).when(self.edit_saving, |button| button.opacity(0.6))
                        .on_click(cx.listener(|this, _, window, cx| this.save_edit(window, cx))))), window, cx.entity_id())
                .w(px(560.)).max_w(relative(0.9)).max_h(relative(0.85)).bg(rgb(SURFACE)).border_1().border_color(rgb(0x44484c))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_edit(window, cx))))
        ).with_priority(2)
    }

    fn show_menu(
        &mut self,
        menu: Menu,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Menu::Download(id) = menu {
            if !self.checked.contains(&id) {
                self.checked = [id].into_iter().collect();
                self.selection_anchor = Some(id);
            }
            self.selected = Some(id);
        }
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
            Menu::Priority(id) => vec![
                (
                    "High",
                    Icon::ChevronUp,
                    MenuAction::SetPriority(id, Priority::High),
                ),
                (
                    "Normal",
                    Icon::Flag,
                    MenuAction::SetPriority(id, Priority::Normal),
                ),
                (
                    "Low",
                    Icon::ChevronDown,
                    MenuAction::SetPriority(id, Priority::Low),
                ),
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
                } else if !d.owns_output && matches!(d.status, Status::Paused | Status::Failed) {
                    items.push(("Resume", Icon::Play, MenuAction::Resume(id)));
                }
                if d.status == Status::Complete {
                    items.push(("Open", Icon::Arrow, MenuAction::Open(id)));
                }
                if d.can_verify_integrity() {
                    items.push(("Verify file integrity", Icon::Check, MenuAction::Verify(id)));
                }
                if d.protocol == strata::protocol::Protocol::Torrent
                    && !d.torrent_files.is_empty()
                    && !d.status.active()
                    && d.status != Status::Queued
                    && d.status != Status::Deleting
                {
                    items.push(("Options", Icon::Settings, MenuAction::Options(id)));
                }
                items.push(("Open folder", Icon::Folder, MenuAction::Folder(id)));
                items.push(("Copy download link", Icon::Copy, MenuAction::Copy(id)));
                if d.status != Status::Deleting {
                    items.push(("Rename", Icon::File, MenuAction::Rename(id)));
                    items.push(("Speed limit", Icon::Settings, MenuAction::SpeedLimit(id)));
                    items.push(("Set priority", Icon::Flag, MenuAction::Priority(id)));
                }
                if !matches!(
                    d.status,
                    Status::Complete
                        | Status::Missing
                        | Status::Cancelled
                        | Status::Assembling
                        | Status::Verifying
                        | Status::Deleting
                ) && !d.owns_output
                {
                    items.push(("Cancel transfer", Icon::Close, MenuAction::Cancel(id)));
                }
                if d.status != Status::Deleting {
                    items.push(("Delete from device", Icon::Trash, MenuAction::Delete(id)));
                }
                items
            }
        }
    }

    fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            MenuAction::Filter(view) => {
                self.view = view;
                self.checked.clear();
                self.selected = None;
                self.selection_anchor = None;
            }
            MenuAction::Pause(id) => {
                for id in self.action_ids(id) {
                    self.manager.pause(id);
                }
            }
            MenuAction::Resume(id) => {
                for id in self.action_ids(id) {
                    self.manager.resume(id);
                }
            }
            MenuAction::Verify(id) => {
                for id in self.action_ids(id) {
                    self.manager.verify_integrity(id);
                }
            }
            MenuAction::Options(id) => {
                self.open_torrent_options(id, window, cx);
                return;
            }
            MenuAction::Cancel(id) => {
                for id in self.action_ids(id) {
                    self.manager.cancel(id);
                }
            }
            MenuAction::Delete(id) => {
                for id in self.action_ids(id) {
                    self.manager.delete(id);
                    self.rates.remove(&id);
                }
            }
            MenuAction::Rename(id) => {
                self.open_edit(EditModal::Rename(id), window, cx);
                return;
            }
            MenuAction::SpeedLimit(id) => {
                self.open_edit(EditModal::SpeedLimit(self.action_ids(id)), window, cx);
                return;
            }
            MenuAction::Priority(id) => {
                let position = self.menu.map(|(_, position)| position).unwrap_or_default();
                self.show_menu(Menu::Priority(id), position, window, cx);
                return;
            }
            MenuAction::SetPriority(id, priority) => {
                self.manager.set_priority(&self.action_ids(id), priority)
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
                                let chosen = matches!(action, MenuAction::Filter(view) if self.view == view)
                                    || matches!(action, MenuAction::SetPriority(id, priority) if self.action_ids(id).iter().all(|id| self.downloads.iter().any(|d| d.id == *id && d.priority == priority)));
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
                                    .when(chosen, |d| d.child(icon(Icon::Check, 14., ACCENT)))
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
        let files = self.filtered(cx);
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .px(px(28.))
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
                            .mr(px(BAR_WIDTH))
                            .w_auto()
                            .h(px(32.))
                            .flex_shrink_0()
                            .child(
                                checkbox(
                                    "check-all",
                                    !files.is_empty()
                                        && files.iter().all(|d| self.checked.contains(&d.id)),
                                    files.iter().any(|d| self.checked.contains(&d.id))
                                        && !files.iter().all(|d| self.checked.contains(&d.id)),
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        let ids = this
                                            .filtered(cx)
                                            .iter()
                                            .map(|d| d.id)
                                            .collect::<Vec<_>>();
                                        if ids.iter().all(|id| this.checked.contains(id)) {
                                            for id in ids {
                                                this.checked.remove(&id);
                                            }
                                        } else {
                                            this.checked.extend(ids);
                                        }
                                        this.selected = this.checked.iter().next_back().copied();
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(label("File name", 12., FAINT).flex_1().min_w_0())
                            .child(
                                label("Size", 12., FAINT)
                                    .w(px(84.))
                                    .text_right()
                                    .flex_shrink_0(),
                            )
                            .child(label("Status", 12., FAINT).w(px(100.)).flex_shrink_0())
                            .child(label("Created", 12., FAINT).w(px(90.)).flex_shrink_0())
                            .child(label("Progress", 12., FAINT).w(px(132.)).flex_shrink_0())
                            .when(!self.compact, |d| {
                                d.child(
                                    label("Speed", 12., FAINT)
                                        .w(px(84.))
                                        .text_right()
                                        .flex_shrink_0(),
                                )
                            })
                            .child(
                                label("ETA", 12., FAINT)
                                    .w(px(84.))
                                    .text_right()
                                    .flex_shrink_0(),
                            )
                            .child(div().w(px(30.)).flex_shrink_0()),
                    )
                    .child(if files.is_empty() {
                        self.empty(cx).flex_1().min_h_0().into_any_element()
                    } else {
                        scroll_area(
                            "download-list",
                            &self.queue_scroll,
                            div().children(files.iter().map(|d| self.row(d, cx))),
                            window,
                            cx.entity_id(),
                        )
                        .flex_1()
                        .min_h_0()
                        .into_any_element()
                    }),
            )
    }

    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let count = if self.view == View::Settings {
            self.downloads.len()
        } else {
            self.filtered(cx).len()
        };
        div()
            .h(px(32.))
            .w_full()
            .flex_shrink_0()
            .px(px(28.))
            .border_t_1()
            .border_color(rgb(RULE))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(label(
                format!(
                    "{count} {}",
                    if count == 1 { "download" } else { "downloads" }
                ),
                12.,
                FAINT,
            ))
            .child(div().flex_1())
            .child(icon(Icon::Folder, 14., FAINT))
            .child(label(short_path(&self.directory), 12., FAINT).truncate())
            .when(self.view != View::Settings, |footer| {
                footer.child(
                    icon_button(
                        "toggle-details",
                        if self.details_collapsed {
                            Icon::ChevronUp
                        } else {
                            Icon::ChevronDown
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.finish_resize();
                        this.details_collapsed = !this.details_collapsed;
                        cx.notify();
                    })),
                )
            })
    }

    fn empty(&self, cx: &mut Context<Self>) -> Div {
        let searching = !self.search.read(cx).value().is_empty();
        let (title, subtitle) = if searching {
            ("No matching downloads", "Try another file name or website.")
        } else if self.downloads.is_empty() {
            (
                "Make room for what’s next.",
                "Add a link. 101 Downloader takes care of the transfer.",
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
        let selected = self.checked.contains(&id);
        let rate = self.rates.get(&id).map_or(0., DownloadSpeed::rate);
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
                checkbox(format!("check-{id}"), selected, false).on_click(cx.listener(
                    move |this, event: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.select_row(id, true, event.modifiers().shift, cx);
                    },
                )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(icon(Icon::File, 18., if selected { TEXT } else { FAINT }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                highlighted_label(d.name.clone(), 14., TEXT, &self.search_query)
                                    .w_full()
                                    .flex_shrink_0()
                                    .min_w_0()
                                    .truncate(),
                            )
                            .when(
                                !self.search_query.is_empty()
                                    && !crate::text::search_ranges(&d.host(), &self.search_query)
                                        .is_empty(),
                                |column| {
                                    column.child(
                                        highlighted_label(d.host(), 11., MUTED, &self.search_query)
                                            .w_full()
                                            .min_w_0()
                                            .flex_shrink_0()
                                            .truncate(),
                                    )
                                },
                            ),
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
                label(crate::datetime::date(d.created), 12., MUTED)
                    .w(px(90.))
                    .flex_shrink_0()
                    .truncate(),
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
                label(
                    if d.status == Status::Downloading {
                        remaining_time(d.total, d.downloaded, rate)
                    } else {
                        None
                    }
                    .unwrap_or_else(|| "—".into()),
                    12.,
                    MUTED,
                )
                .w(px(84.))
                .text_right()
                .flex_shrink_0(),
            )
            .child(
                icon_button(format!("menu-{id}"), Icon::More).on_click(cx.listener(
                    move |this, event: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.selected = Some(id);
                        this.show_menu(Menu::Download(id), event.position(), window, cx);
                    },
                )),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.show_menu(Menu::Download(id), event.position, window, cx);
                }),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                let modifiers = event.modifiers();
                this.select_row(id, modifiers.control, modifiers.shift, cx);
                if event.click_count() == 2
                    && let Some(download) = this.downloads.iter().find(|download| download.id == id)
                    && download.status == Status::Complete
                {
                    reveal(&download.path());
                }
            }))
    }

    fn torrent_modal(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let download = self
            .downloads
            .iter()
            .find(|d| Some(d.id) == self.torrent_picker)
            .unwrap();
        let count = download.torrent_files.len();
        let selected_size = self
            .torrent_checked
            .iter()
            .map(|&index| download.torrent_files[index].size)
            .sum();
        let visible = torrent_tree::rows(&self.torrent_tree, &self.torrent_expanded);
        let row_count = self.torrent_focus.len() - 1;
        let rows = div()
            .w_full()
            .flex()
            .flex_col()
            .children(visible.into_iter().map(|(node, depth)| {
                let id = node.id;
                let folder = !node.children.is_empty();
                let selected = node
                    .files
                    .iter()
                    .filter(|index| self.torrent_checked.contains(index))
                    .count();
                let size = node
                    .files
                    .iter()
                    .map(|&index| download.torrent_files[index].size)
                    .sum();
                div()
                    .id(("torrent-tree-row", id))
                    .h(px(40.))
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(8. + depth as f32 * 18.))
                    .pr(px(8.))
                    .border_b_1()
                    .border_color(rgb(RULE))
                    .hover(|style| style.bg(rgb(RAISED)))
                    .cursor_pointer()
                    .child(div().w(px(16.)).flex_shrink_0().when(folder, |disclosure| {
                        disclosure.child(icon(
                            if self.torrent_expanded.contains(&id) {
                                Icon::ChevronDown
                            } else {
                                Icon::ChevronRight
                            },
                            12.,
                            MUTED,
                        ))
                    }))
                    .child(
                        checkbox(
                            format!("torrent-node-{id}"),
                            selected == node.files.len(),
                            selected > 0 && selected < node.files.len(),
                        )
                        .track_focus(&self.torrent_focus[id + 1])
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.torrent_focus[id + 1].focus(window);
                                this.toggle_torrent_node(Some(id), cx);
                            },
                        )),
                    )
                    .child(icon(
                        if folder { Icon::Folder } else { Icon::File },
                        14.,
                        MUTED,
                    ))
                    .child(label(&node.name, 13., TEXT).flex_1().min_w_0().truncate())
                    .child(label(bytes(size), 12., MUTED).flex_shrink_0())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.torrent_focus[id + 1].focus(window);
                        if folder {
                            this.expand_torrent_node(id, window, cx);
                        } else {
                            this.toggle_torrent_node(Some(id), cx);
                        }
                    }))
            }));
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
                    div()
                        .w(px(660.))
                        .max_w(relative(0.9))
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(rgb(0x44484c))
                        .p(px(18.))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    label("Choose torrent files", 18., TEXT)
                                        .font_weight(FontWeight::MEDIUM),
                                )
                                .child(
                                    icon_button("close-torrent", Icon::Close)
                                        .track_focus(&self.modal_close_focus)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_torrent_picker(window, cx)
                                        })),
                                ),
                        )
                        .child(label(&download.name, 13., MUTED).truncate())
                        .child(label(
                            if download.selected_files.is_some() { "Previously selected files stay selected. Choose more files to download." } else { "Select files or folders. Expand folders to choose individual files." },
                            13.,
                            MUTED,
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    checkbox(
                                        "torrent-select-all",
                                        self.torrent_checked.len() == count,
                                        !self.torrent_checked.is_empty()
                                            && self.torrent_checked.len() != count,
                                    )
                                    .track_focus(&self.torrent_focus[0])
                                    .on_click(cx.listener(
                                        |this, _, _, cx| this.toggle_torrent_node(None, cx),
                                    )),
                                )
                                .child(label("Select all", 13., TEXT)),
                        )
                        .child(
                            scroll_area(
                                "torrent-file-list",
                                &self.torrent_scroll,
                                rows,
                                window,
                                cx.entity_id(),
                            )
                            .h(px((row_count as f32 * 40.).min(280.).min(
                                (f32::from(window.viewport_size().height) - 440.).max(80.),
                            )))
                            .w_full()
                            .bg(rgb(SURFACE))
                            .border_1()
                            .border_color(rgb(RULE)),
                        )
                        .child(label(
                            format!(
                                "{} of {count} files selected · {}",
                                self.torrent_checked.len(),
                                bytes(selected_size)
                            ),
                            13.,
                            MUTED,
                        ))
                        .children(
                            self.torrent_error
                                .as_ref()
                                .map(|error| label(error, 12., RED)),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(10.))
                                .child(
                                    button("cancel-torrent", "Cancel", Icon::Close, false)
                                        .track_focus(&self.modal_choose_focus)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_torrent_picker(window, cx)
                                        })),
                                )
                                .child(
                                    button(
                                        "start-torrent",
                                        "Download selected",
                                        Icon::Download,
                                        true,
                                    )
                                    .track_focus(&self.modal_start_focus)
                                    .when(self.torrent_checked.is_empty(), |button| {
                                        button.opacity(0.5)
                                    })
                                    .on_click(cx.listener(
                                        |this, _, window, cx| this.start_torrent(window, cx),
                                    )),
                                ),
                        )
                        .on_mouse_down_out(
                            cx.listener(|this, _, window, cx| {
                                this.close_torrent_picker(window, cx)
                            }),
                        ),
                ),
        )
        .with_priority(2)
    }

    fn add_modal(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                        "new-download-modal",
                        &self.add_scroll,
                        self.add_form(cx),
                        window,
                        cx.entity_id(),
                    )
                    .w(px(660.))
                    .max_w(relative(0.9))
                    .max_h(relative(0.85))
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgb(0x44484c))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                        this.adding = false;
                        this.error = None;
                        this.focus.focus(window);
                        cx.notify();
                    })),
                ),
        )
        .with_priority(2)
    }

    fn add_form(&self, cx: &mut Context<Self>) -> Div {
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
                    .child(
                        icon_button("close-add", Icon::Close)
                            .track_focus(&self.modal_close_focus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.adding = false;
                                this.error = None;
                                window.focus(&this.focus);
                                cx.notify();
                            })),
                    ),
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
                .track_focus(&self.modal_choose_focus)
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
                            .track_focus(&self.modal_start_focus)
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
        let panel = div()
            .w_full()
            .min_w_0()
            .h(px(self.details_height))
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(RULE))
            .flex()
            .flex_col()
            .child(self.details_resizer(cx));
        let Some(d) = self.downloads.iter().find(|d| Some(d.id) == self.selected) else {
            return panel.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(label("Click on a download to view more info.", 13., MUTED)),
            );
        };
        let id = d.id;
        let rate = self.rates.get(&id).map_or(0., DownloadSpeed::rate);
        let eta = if d.status == Status::Downloading {
            remaining_time(d.total, d.downloaded, rate)
        } else {
            None
        };
        panel
            .child(
                div()
                    .px(px(28.))
                    .py(px(8.))
                    .flex_shrink_0()
                    .child(segmented_progress(d)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(28.))
                    .py(px(8.))
                    .flex()
                    .gap(px(28.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                label(&d.name, 15., TEXT)
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate(),
                            )
                            .child(
                                label(
                                    format!(
                                        "{} · {:.1}% · {} of {}",
                                        d.status.label(),
                                        d.fraction() * 100.,
                                        bytes(d.downloaded),
                                        d.total.map(bytes).unwrap_or_else(|| "unknown size".into())
                                    ),
                                    12.,
                                    status_color(d.status),
                                )
                                .truncate(),
                            )
                            .child(
                                label(
                                    format!(
                                        "{} · {}",
                                        if rate > 0. {
                                            format!("{}/s", bytes(rate as u64))
                                        } else {
                                            "—".into()
                                        },
                                        eta.map(|eta| format!("{eta} left"))
                                            .unwrap_or_else(|| "No ETA".into())
                                    ),
                                    12.,
                                    MUTED,
                                )
                                .truncate(),
                            )
                            .children(
                                d.error
                                    .as_ref()
                                    .map(|error| label(error, 12., RED).truncate()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(detail_line("Protocol", d.protocol.label()))
                            .child(detail_line(
                                "Integrity",
                                if d.integrity.is_empty() {
                                    "No saved checksum"
                                } else if d.integrity_baseline_only {
                                    "Initial checksum recorded"
                                } else {
                                    "SHA-256 checked"
                                },
                            ))
                            .child(detail_line("Source", &d.host()))
                            .child(detail_line(
                                "Completed at",
                                &d.completed_at
                                    .map(crate::datetime::date_time)
                                    .unwrap_or_else(|| "—".into()),
                            ))
                            .child(detail_line("Destination", &d.path().display().to_string()))
                            .child(detail_line(
                                "Priority / limit",
                                &format!(
                                    "{} / {}",
                                    d.priority.label(),
                                    if d.speed_limit == 0 {
                                        "Unlimited".into()
                                    } else {
                                        format!("{}/s", bytes(d.speed_limit))
                                    }
                                ),
                            )),
                    ),
            )
    }

    fn check_for_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        self.update_available = None;
        self.update_notice = None;
        let check = cx
            .background_executor()
            .spawn(async { strata::updates::check() });
        cx.spawn_in(window, async move |this, cx| {
            let result = check.await;
            let _ = this.update(cx, |this, cx| {
                this.update_checking = false;
                match result {
                    Ok(Some(update)) => {
                        this.update_notice =
                            Some(format!("Version {} is available.", update.version));
                        this.update_available = Some(update);
                    }
                    Ok(None) => {
                        this.update_notice = Some("No newer stable version is available.".into())
                    }
                    Err(error) => {
                        this.update_notice = Some(format!("Could not check for updates: {error:#}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn download_update(&mut self, cx: &mut Context<Self>) {
        let Some(update) = &self.update_available else {
            return;
        };
        match self.manager.add(
            &update.installer_url,
            self.directory.clone(),
            self.connections,
        ) {
            Ok(id) => {
                self.update_notice = Some("Installer added to your downloads. When it finishes, quit through the tray menu, then run the installer.".into());
                self.update_available = None;
                self.selected = Some(id);
                self.view = View::All;
                self.refresh();
            }
            Err(error) => {
                self.update_notice = Some(format!("Could not download the update: {error:#}"))
            }
        }
        cx.notify();
    }

    fn settings(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let content = div().p(px(32.)).flex().flex_col().gap(px(24.))
            .child(label("Settings", 24., TEXT).font_weight(FontWeight::SEMIBOLD))
            .child(div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Global speed limit", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label({ let limit = self.manager.settings.lock().unwrap().speed_limit; if limit == 0 { "Unlimited".into() } else { format!("{}/s shared across all downloads", bytes(limit)) } }, 13., MUTED))
                .child(button("global-speed-limit", "Set global speed limit", Icon::Settings, false)
                    .on_click(cx.listener(|this, _, window, cx| this.open_edit(EditModal::GlobalSpeedLimit, window, cx)))))
            .child(div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("App updates", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label(format!("Installed version {}", env!("CARGO_PKG_VERSION")), 13., MUTED))
                .child(label("Download stable updates from GitHub. After the download finishes, quit through the tray menu and run the installer.", 13., MUTED).max_w(px(620.)))
                .child(div().flex().gap(px(12.))
                    .child(button("check-updates", if self.update_checking { "Checking GitHub..." } else { "Check for updates" }, Icon::Arrow, false)
                        .when(self.update_checking, |button| button.opacity(0.6))
                        .on_click(cx.listener(|this, _, window, cx| this.check_for_updates(window, cx))))
                    .when(self.update_available.is_some(), |row| row.child(
                        button("download-update", "Download update", Icon::Download, true)
                            .on_click(cx.listener(|this, _, _, cx| this.download_update(cx))))))
                .children(self.update_notice.as_ref().map(|message| label(message, 12., MUTED))))
            .child(div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Connections & authentication", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("Proxy settings apply to HTTP, HTTPS, WebDAV, Metalink HTTP mirrors, and video transfers. FTP, SFTP, and torrent peers use direct connections. URL passwords are protected by Windows.", 13., MUTED).max_w(px(620.)))
                .child(transport_input("HTTP / HTTPS / SOCKS5 proxy", self.proxy.clone()))
                .child(transport_input("SFTP private key (optional)", self.identity.clone()))
                .child(transport_input("SFTP known_hosts file (optional)", self.known_hosts.clone()))
                .child(label("SFTP refuses unknown or changed host keys. Verify the host with OpenSSH first. Encrypted keys can use 101Downloader-cli --key-passphrase-stdin.", 12., FAINT).max_w(px(620.)))
                .child(button("save-transport", "Save connection settings", Icon::Link, false).on_click(cx.listener(|this, _, _, cx| this.save_transport(cx))))
                .children(self.transport_notice.as_ref().map(|message| label(message, 12., MUTED))))
            .child(div().mt(px(10.)).p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Download folder", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("New downloads are saved here. Existing transfers keep their destination.", 13., MUTED))
                .child(label(self.directory.display().to_string(), 13., TEXT))
                .child(button("choose-folder", "Choose folder", Icon::Folder, false).on_click(cx.listener(|this, _, window, cx| this.choose_directory(window, cx)))))
            .child(div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Connections per download", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("More connections can help on some servers. 101 Downloader uses fewer for small files and servers that need a single connection.", 13., MUTED).max_w(px(620.)))
                .child(div().flex().gap(px(10.)).children([1, 4, 8, 16].into_iter().map(|n| {
                    div().id(ElementId::Name(format!("connections-{n}").into())).w(px(64.)).h(px(36.)).flex().items_center().justify_center().cursor_pointer()
                        .bg(rgb(if self.connections == n { TEXT } else { RAISED })).text_color(rgb(if self.connections == n { BG } else { MUTED }))
                        .focusable().focus(|s| s.border_1().border_color(rgb(ACCENT))).hover(|s| s.opacity(0.85)).child(n.to_string())
                        .on_click(cx.listener(move |this, _, _, cx| this.set_connections(n, cx)))
                }))))
            .child(div().p(px(22.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Browser companion", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("Capture file downloads in Chromium browsers and send them straight to 101 Downloader. The browser extension controls when capture is on.", 13., MUTED).max_w(px(620.)))
                .child(label(if self.browser_registered { "Local bridge registered" } else { "Connect the app, then load the extension in your browser." }, 12., if self.browser_registered { ACCENT } else { FAINT }))
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
            .when(self.tray.is_some(), |view| view.child(label("Closing the window keeps downloads running in the system tray. Right-click the tray icon and choose Quit 101 Downloader to exit.", 12., FAINT)))
        ;
        scroll_area(
            "settings-scroll",
            &self.settings_scroll,
            content,
            window,
            cx.entity_id(),
        )
        .flex_1()
        .min_h_0()
        .min_w_0()
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
                if this.torrent_picker.is_some() {
                    let focused = this
                        .torrent_focus
                        .iter()
                        .position(|handle| handle.is_focused(window));
                    let visible = torrent_tree::rows(&this.torrent_tree, &this.torrent_expanded)
                        .iter()
                        .map(|(node, depth)| (node.id, *depth, !node.children.is_empty()))
                        .collect::<Vec<_>>();
                    match key {
                        "escape" => this.close_torrent_picker(window, cx),
                        "space" | "enter" if focused.is_some() => {
                            this.toggle_torrent_node(focused.unwrap().checked_sub(1), cx);
                        }
                        "space" | "enter" => {
                            if this.modal_close_focus.is_focused(window)
                                || this.modal_choose_focus.is_focused(window)
                            {
                                this.close_torrent_picker(window, cx);
                            } else if this.modal_start_focus.is_focused(window) {
                                this.start_torrent(window, cx);
                            }
                        }
                        "left" | "right" => {
                            if let Some(position) = focused
                                .and_then(|index| index.checked_sub(1))
                                .and_then(|id| visible.iter().position(|row| row.0 == id))
                            {
                                let (id, depth, folder) = visible[position];
                                if folder
                                    && ((key == "right" && !this.torrent_expanded.contains(&id))
                                        || (key == "left" && this.torrent_expanded.contains(&id)))
                                {
                                    this.expand_torrent_node(id, window, cx);
                                } else {
                                    let next = if key == "right" && folder {
                                        visible.get(position + 1).map(|row| row.0)
                                    } else if key == "left" {
                                        visible[..position]
                                            .iter()
                                            .rev()
                                            .find(|row| row.1 < depth)
                                            .map(|row| row.0)
                                    } else {
                                        None
                                    };
                                    if let Some(id) = next {
                                        this.torrent_focus[id + 1].focus(window);
                                        let position =
                                            visible.iter().position(|row| row.0 == id).unwrap();
                                        this.torrent_scroll.reveal(
                                            position as f32 * 40.,
                                            (position + 1) as f32 * 40.,
                                        );
                                        cx.notify();
                                    }
                                }
                            }
                        }
                        "tab" | "up" | "down" => {
                            let mut handles = vec![this.torrent_focus[0].clone()];
                            handles.extend(
                                visible
                                    .iter()
                                    .map(|row| this.torrent_focus[row.0 + 1].clone()),
                            );
                            if key == "tab" {
                                handles.extend([
                                    this.modal_choose_focus.clone(),
                                    this.modal_start_focus.clone(),
                                    this.modal_close_focus.clone(),
                                ]);
                            }
                            let index = handles
                                .iter()
                                .position(|handle| handle.is_focused(window))
                                .unwrap_or(0);
                            let next = if key == "up"
                                || (key == "tab" && event.keystroke.modifiers.shift)
                            {
                                (index + handles.len() - 1) % handles.len()
                            } else {
                                (index + 1) % handles.len()
                            };
                            handles[next].focus(window);
                            if next > 0 && next <= visible.len() {
                                this.torrent_scroll
                                    .reveal((next - 1) as f32 * 40., next as f32 * 40.);
                            }
                            cx.notify();
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                    return;
                }
                if this.edit.is_some() {
                    match key {
                        "escape" => this.close_edit(window, cx),
                        "enter" => {
                            if this.edit_input.focus_handle(cx).is_focused(window)
                                || this.modal_start_focus.is_focused(window)
                            {
                                this.save_edit(window, cx);
                            }
                        }
                        "tab" => {
                            let handles = [
                                this.edit_input.focus_handle(cx),
                                this.modal_choose_focus.clone(),
                                this.modal_start_focus.clone(),
                                this.modal_close_focus.clone(),
                            ];
                            let index = handles
                                .iter()
                                .position(|handle| handle.is_focused(window))
                                .unwrap_or(0);
                            let next = if event.keystroke.modifiers.shift {
                                (index + handles.len() - 1) % handles.len()
                            } else {
                                (index + 1) % handles.len()
                            };
                            handles[next].focus(window);
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.modifiers.control
                    && key == "a"
                    && !this.adding
                    && !this.search.focus_handle(cx).is_focused(window)
                {
                    this.checked = this.filtered(cx).iter().map(|d| d.id).collect();
                    this.selected = this.checked.iter().next_back().copied();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if event.keystroke.modifiers.control && key == "n" {
                    this.show_add(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.modifiers.control && key == "f" {
                    this.menu = None;
                    if this.adding {
                        this.url.focus_handle(cx).focus(window);
                    } else {
                        this.search.focus_handle(cx).focus(window);
                    }
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
                    if this.adding {
                        let handles = [
                            this.url.focus_handle(cx),
                            this.modal_choose_focus.clone(),
                            this.modal_start_focus.clone(),
                            this.modal_close_focus.clone(),
                        ];
                        let index = handles
                            .iter()
                            .position(|handle| handle.is_focused(window))
                            .unwrap_or(0);
                        let next = if event.keystroke.modifiers.shift {
                            (index + handles.len() - 1) % handles.len()
                        } else {
                            (index + 1) % handles.len()
                        };
                        handles[next].focus(window);
                        cx.stop_propagation();
                        return;
                    }
                    if event.keystroke.modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                }
            }))
            .child(self.titlebar(window, cx))
            .children(
                self.update_available
                    .as_ref()
                    .filter(|_| self.view != View::Settings)
                    .map(|update| {
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .px(px(28.))
                            .py(px(12.))
                            .bg(rgb(SURFACE))
                            .border_b_1()
                            .border_color(rgb(RULE))
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .child(
                                label(
                                    format!("101 Downloader {} is available.", update.version),
                                    13.,
                                    TEXT,
                                )
                                .flex_1(),
                            )
                            .child(
                                button(
                                    "offer-download-update",
                                    "Download update",
                                    Icon::Download,
                                    true,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.download_update(cx))),
                            )
                    }),
            )
            .child(
                // Focusable ancestors consume native caption mouse-down events in GPUI.
                div()
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .when(self.view == View::Settings, |d| {
                                d.child(self.topbar(cx)).child(self.settings(window, cx))
                            })
                            .when(self.view != View::Settings, |d| {
                                d.child(self.topbar(cx))
                                    .child(self.queue(window, cx))
                                    .when(!self.details_collapsed, |body| {
                                        body.child(self.inspector(cx))
                                    })
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
                                self.tray_error.as_ref().map(|message| {
                                    label(message, 12., AMBER).px(px(28.)).py(px(10.))
                                }),
                            ),
                    ),
            )
            .child(self.footer(cx))
            .children(
                self.menu
                    .map(|(menu, position)| self.popup_menu(menu, position, cx)),
            )
            .children(self.adding.then(|| self.add_modal(window, cx)))
            .children(self.edit.as_ref().map(|_| self.edit_modal(window, cx)))
            .children(self.torrent_picker.map(|_| self.torrent_modal(window, cx)))
    }
}

struct ActionTooltip(&'static str);

impl Render for ActionTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(px(8.))
            .bg(rgb(RAISED))
            .border_1()
            .border_color(rgb(MUTED))
            .font_family("Segoe UI")
            .child(label(self.0, 12., TEXT))
    }
}

fn label(text: impl AsRef<str>, size: f32, color: u32) -> Div {
    highlighted_label(text, size, color, "")
}

fn highlighted_label(text: impl AsRef<str>, size: f32, color: u32, query: &str) -> Div {
    let highlights = crate::text::search_ranges(text.as_ref(), query)
        .into_iter()
        .map(|range| {
            (
                range,
                HighlightStyle {
                    color: Some(rgb(BG).into()),
                    background_color: Some(rgb(AMBER).into()),
                    font_weight: Some(FontWeight::MEDIUM),
                    ..Default::default()
                },
            )
        })
        .collect();
    div()
        .text_size(px(size))
        .line_height(px((size * 1.4).round()))
        .text_color(rgb(color))
        .child(crate::text::CrispText(
            text.as_ref().to_owned().into(),
            highlights,
        ))
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

fn checkbox(id: impl Into<SharedString>, checked: bool, mixed: bool) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .size(px(28.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .focusable()
        .focus(|style| style.bg(rgb(RAISED)))
        .child(
            div()
                .size(px(16.))
                .border_1()
                .border_color(rgb(if checked || mixed { TEXT } else { MUTED }))
                .bg(rgb(if checked || mixed { TEXT } else { BG }))
                .flex()
                .items_center()
                .justify_center()
                .when(checked, |box_| box_.child(icon(Icon::Check, 12., BG)))
                .when(mixed && !checked, |box_| {
                    box_.child(div().w(px(8.)).h(px(2.)).bg(rgb(BG)))
                }),
        )
}

fn select_download(
    checked: &mut BTreeSet<u64>,
    anchor: &mut Option<u64>,
    visible: &[u64],
    id: u64,
    control: bool,
    shift: bool,
) {
    if shift
        && let Some((start, end)) = anchor
            .and_then(|anchor| visible.iter().position(|candidate| *candidate == anchor))
            .zip(visible.iter().position(|candidate| *candidate == id))
    {
        if !control {
            checked.clear();
        }
        checked.extend(&visible[start.min(end)..=start.max(end)]);
        return;
    }
    if control {
        if !checked.remove(&id) {
            checked.insert(id);
        }
    } else {
        checked.clear();
        checked.insert(id);
    }
    *anchor = Some(id);
}

fn filename_selection_end(name: &str, folder: bool) -> usize {
    if folder {
        name.len()
    } else {
        std::path::Path::new(name)
            .file_stem()
            .map_or(name.len(), |stem| stem.len())
    }
}

fn parse_speed_limit(value: &str) -> Result<u64, String> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(1024))
        .filter(|value| *value <= u32::MAX as u64)
        .ok_or_else(|| "Enter a whole number from 0 to 4194303 KiB/s. Use 0 for unlimited.".into())
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
        .items_center()
        .gap(px(10.))
        .min_w_0()
        .child(label(title, 12., FAINT).w(px(94.)).flex_shrink_0())
        .child(label(value, 12., MUTED).flex_1().min_w_0().truncate())
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
        .focus(|s| s.border_1().border_color(rgb(ACCENT)))
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
                0xc10801
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
    requested.clamp(180., (viewport - 460.).max(180.))
}

fn segment_layout(parts: &[strata::engine::Part], known_size: bool) -> Vec<(f32, f32)> {
    let total: f64 = parts.iter().map(|part| part.len() as f64).sum();
    let mut ordered = parts.iter().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|part| part.start);
    ordered
        .into_iter()
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
        Status::Downloading | Status::Complete => ACCENT,
        Status::Paused | Status::Pausing => AMBER,
        Status::Failed | Status::Missing => RED,
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

fn remaining_time(total: Option<u64>, downloaded: u64, rate: f64) -> Option<String> {
    if !rate.is_finite() || rate <= 0. {
        return None;
    }
    total.map(|total| duration(total.saturating_sub(downloaded) as f64 / rate))
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
                Icon::Flag => vec![vec![
                    (5., 22.),
                    (5., 3.),
                    (13., 3.),
                    (13., 5.),
                    (21., 5.),
                    (18., 10.),
                    (21., 15.),
                    (13., 15.),
                    (13., 13.),
                    (5., 13.),
                ]],
                Icon::ChevronDown => vec![vec![(5., 8.), (12., 15.), (19., 8.)]],
                Icon::ChevronUp => vec![vec![(5., 16.), (12., 9.), (19., 16.)]],
                Icon::ChevronRight => vec![vec![(8., 5.), (15., 12.), (8., 19.)]],
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
    use super::{filename_selection_end, parse_speed_limit, select_download};
    use std::collections::BTreeSet;

    #[test]
    fn selection_supports_toggle_ranges_and_display_order() {
        let visible = [50, 40, 30, 20, 10];
        let mut selected = BTreeSet::new();
        let mut anchor = None;
        select_download(&mut selected, &mut anchor, &visible, 40, false, false);
        select_download(&mut selected, &mut anchor, &visible, 10, false, true);
        assert_eq!(selected, [40, 30, 20, 10].into_iter().collect());
        select_download(&mut selected, &mut anchor, &visible, 30, true, false);
        assert_eq!(selected, [40, 20, 10].into_iter().collect());
        select_download(&mut selected, &mut anchor, &visible, 50, true, true);
        assert_eq!(selected, [50, 40, 30, 20, 10].into_iter().collect());
        select_download(&mut selected, &mut anchor, &visible, 20, false, false);
        assert_eq!(selected, [20].into_iter().collect());
    }

    #[test]
    fn rename_selection_excludes_only_the_file_extension_and_handles_unicode() {
        for (name, expected) in [
            ("report.pdf", "report"),
            ("résumé.tar.gz", "résumé.tar"),
            ("README", "README"),
            (".gitignore", ".gitignore"),
        ] {
            assert_eq!(&name[..filename_selection_end(name, false)], expected);
        }
        assert_eq!(
            filename_selection_end("folder.with.dots", true),
            "folder.with.dots".len()
        );
        assert_eq!(parse_speed_limit("512").unwrap(), 512 * 1024);
        assert_eq!(parse_speed_limit("0").unwrap(), 0);
        for invalid in ["", "-1", "1.5", "4194304", "99999999999999999999999"] {
            assert!(parse_speed_limit(invalid).is_err());
        }
    }
    use super::{clamp_details_height, remaining_time, segment_layout};
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
        assert_eq!(
            segment_layout(
                &[
                    Part {
                        start: 100,
                        end: 399,
                        downloaded: 300
                    },
                    Part {
                        start: 0,
                        end: 99,
                        downloaded: 50
                    },
                ],
                true
            ),
            segments
        );
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
        assert_eq!(clamp_details_height(-100., 700.), 180.);
        assert_eq!(clamp_details_height(f32::NAN, 880.), 240.);
    }

    #[test]
    fn eta_requires_a_known_size_and_a_live_finite_rate() {
        assert_eq!(remaining_time(Some(1000), 400, 200.), Some("3s".into()));
        assert_eq!(remaining_time(None, 400, 200.), None);
        assert_eq!(remaining_time(Some(1000), 400, 0.), None);
        assert_eq!(remaining_time(Some(1000), 400, f64::NAN), None);
    }
}
