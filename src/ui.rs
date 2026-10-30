use crate::input::TextInput;
use gpui::{prelude::*, *};
use std::{
    collections::HashMap,
    path::PathBuf,
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
enum Icon {
    Download,
    Layers,
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
    Grid,
}

pub struct Strata {
    manager: Arc<Manager>,
    downloads: Vec<Download>,
    view: View,
    selected: Option<u64>,
    url: Entity<TextInput>,
    search: Entity<TextInput>,
    adding: bool,
    error: Option<String>,
    focus: FocusHandle,
    compact: bool,
    directory: PathBuf,
    connections: usize,
    rates: HashMap<u64, f64>,
    previous: HashMap<u64, u64>,
    sampled_at: Instant,
    _ticker: Task<()>,
    _search_observer: Subscription,
}

impl Strata {
    pub fn new(manager: Arc<Manager>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = manager.settings.lock().unwrap().clone();
        let url = cx.new(|cx| TextInput::new("https://example.com/file.zip", cx));
        let search = cx.new(|cx| TextInput::new("Search downloads", cx));
        let observer = cx.observe(&search, |_, _, cx| cx.notify());
        let ticker = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.refresh();
                        cx.notify();
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
            downloads,
            view: View::All,
            selected,
            url,
            search,
            adding: false,
            error: None,
            focus,
            compact: false,
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
        self.downloads = self.manager.snapshots();
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
        self.view = View::All;
        self.adding = true;
        self.error = None;
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text())
            && strata::engine::validate_url(&text).is_ok()
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
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(label("Personal", 13., MUTED))
                    .child(label("/", 13., FAINT))
                    .child(label("Downloads", 13., TEXT)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().size(px(6.)).rounded_full().bg(rgb(BLUE)))
                    .child(label("Local workspace", 12., MUTED)),
            )
            .child(
                button("top-add", "New download", Icon::Plus, true)
                    .on_click(cx.listener(|this, _, window, cx| this.show_add(window, cx))),
            )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let active = self
            .downloads
            .iter()
            .filter(|d| d.status.active() || d.status == Status::Queued)
            .count();
        let completed = self
            .downloads
            .iter()
            .filter(|d| d.status == Status::Complete)
            .count();
        let paused = self
            .downloads
            .iter()
            .filter(|d| {
                matches!(
                    d.status,
                    Status::Paused | Status::Failed | Status::Cancelled
                )
            })
            .count();
        div()
            .w(px(if self.compact { 188. } else { 212. }))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(RULE))
            .px(px(18.))
            .child(
                div()
                    .h(px(88.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(8.))
                    .child(icon(Icon::Layers, 27., TEXT))
                    .child(label("strata", 25., TEXT).font_weight(FontWeight::SEMIBOLD)),
            )
            .child(
                label("Library", 12., FAINT)
                    .px(px(12.))
                    .mt(px(18.))
                    .mb(px(14.)),
            )
            .child(self.nav("nav-all", "All downloads", Icon::Grid, View::All, None, cx))
            .child(self.nav(
                "nav-active",
                "In progress",
                Icon::Download,
                View::Active,
                Some(active),
                cx,
            ))
            .child(self.nav(
                "nav-complete",
                "Completed",
                Icon::Check,
                View::Finished,
                Some(completed),
                cx,
            ))
            .child(self.nav(
                "nav-paused",
                "Stopped",
                Icon::Pause,
                View::Paused,
                Some(paused),
                cx,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .px(px(12.))
                    .py(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label("Your files. Your bandwidth.", 12., MUTED))
                    .child(label("Up to 16 parallel connections", 11., FAINT)),
            )
            .child(self.nav(
                "nav-settings",
                "Settings",
                Icon::Settings,
                View::Settings,
                None,
                cx,
            ))
            .child(
                div()
                    .h(px(58.))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .child(label("Strata  /  0.1.0", 11., FAINT)),
            )
    }

    fn nav(
        &self,
        id: &'static str,
        text: &str,
        glyph: Icon,
        view: View,
        count: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let chosen = self.view == view;
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(12.))
            .h(px(42.))
            .px(px(12.))
            .rounded(px(8.))
            .mb(px(4.))
            .cursor_pointer()
            .text_color(rgb(if chosen { TEXT } else { MUTED }))
            .when(chosen, |d| d.bg(rgb(SURFACE)))
            .hover(|s| s.bg(rgb(RAISED)))
            .focusable()
            .focus(|s| s.bg(rgb(RAISED)))
            .child(icon(glyph, 17., if chosen { TEXT } else { FAINT }))
            .child(div().flex_1().text_size(px(13.)).child(text.to_owned()))
            .children(
                count
                    .filter(|n| *n > 0)
                    .map(|n| label(n.to_string(), 12., FAINT)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.view = view;
                cx.notify();
            }))
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
            .flex()
            .flex_col()
            .px(px(28.))
            .pt(px(28.))
            .gap(px(20.))
            .child(
                div()
                    .flex()
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
                    .h(px(104.))
                    .rounded(px(12.))
                    .bg(rgb(SURFACE))
                    .px(px(22.))
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
                            .rounded(px(8.))
                            .child(icon(Icon::Search, 16., FAINT))
                            .child(self.search.clone()),
                    )
                    .child(label("Newest first", 12., FAINT)),
            )
            .child(
                div()
                    .flex()
                    .h(px(24.))
                    .items_center()
                    .px(px(14.))
                    .child(label("File name", 12., FAINT).flex_1())
                    .child(label("Progress", 12., FAINT).w(px(132.)))
                    .when(!self.compact, |d| {
                        d.child(label("Speed", 12., FAINT).w(px(90.)).text_right())
                    })
                    .child(div().w(px(42.))),
            )
            .child(
                div()
                    .id("download-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(files.is_empty(), |d| d.child(self.empty(cx)))
                    .children(files.iter().map(|d| self.row(d, cx))),
            )
            .child(
                div()
                    .h(px(48.))
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
            .h(px(300.))
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
        let resumable = matches!(d.status, Status::Paused | Status::Failed);
        div()
            .id(ElementId::Name(format!("download-{id}").into()))
            .w_full()
            .h(px(88.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(12.))
            .rounded(px(10.))
            .mb(px(6.))
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(SURFACE)))
            .hover(|s| s.bg(rgb(SURFACE)))
            .focusable()
            .focus(|s| s.bg(rgb(RAISED)))
            .child(div().w(px(32.)).flex_shrink_0().child(icon(
                Icon::File,
                27.,
                if selected { TEXT } else { FAINT },
            )))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(7.))
                    .child(
                        label(d.name.clone(), 14., TEXT)
                            .font_weight(FontWeight::MEDIUM)
                            .truncate(),
                    )
                    .child(
                        label(
                            format!(
                                "{}  ·  {}",
                                d.host(),
                                d.total.map(bytes).unwrap_or_else(|| "Unknown size".into())
                            ),
                            12.,
                            FAINT,
                        )
                        .truncate(),
                    ),
            )
            .child(
                div()
                    .w(px(132.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(label(d.status.label(), 11., color))
                            .child(label(
                                if d.total.is_some() {
                                    format!("{:.0}%", d.fraction() * 100.)
                                } else {
                                    "—".into()
                                },
                                11.,
                                FAINT,
                            )),
                    )
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
                    .w(px(78.))
                    .text_right()
                    .flex_shrink_0(),
                )
            })
            .child(div().w(px(30.)).flex_shrink_0().children(
                (d.status.active() || d.status == Status::Queued || resumable).then(|| {
                    icon_button(
                        format!("toggle-{id}"),
                        if resumable { Icon::Play } else { Icon::Pause },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if resumable {
                            this.manager.resume(id);
                        } else {
                            this.manager.pause(id);
                        }
                        this.refresh();
                        cx.notify();
                    }))
                }),
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id);
                cx.notify();
            }))
    }

    fn add_form(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        div()
            .p(px(18.))
            .bg(rgb(SURFACE))
            .rounded(px(12.))
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
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(0x50565c))
                    .child(icon(Icon::Link, 17., FAINT))
                    .child(self.url.clone()),
            )
            .children(self.error.as_ref().map(|e| label(e.clone(), 12., RED)))
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

    fn inspector(&self, cx: &mut Context<Self>) -> Div {
        let selected = self
            .downloads
            .iter()
            .find(|d| Some(d.id) == self.selected)
            .cloned();
        let width = if self.compact { 272. } else { 304. };
        let mut panel = div()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(rgb(RULE))
            .flex()
            .flex_col();
        panel = panel.child(
            div()
                .h(px(64.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(22.))
                .border_b_1()
                .border_color(rgb(RULE))
                .child(label("Transfer details", 13., MUTED)),
        );
        let Some(d) = selected else {
            return panel.child(div().p(px(24.)).flex().flex_col().gap(px(16.))
                .child(label("A little more visibility.", 17., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("Select a download to see its progress, connections, and destination.", 13., MUTED).line_height(px(22.)))
                .child(div().h(px(1.)).bg(rgb(RULE)).my(px(10.)))
                .child(label("Built for the bigger files", 13., TEXT))
                .child(label("Strata splits compatible files into parts and downloads them together. Each connection gets its own lane here.", 13., FAINT).line_height(px(22.))));
        };
        let id = d.id;
        let rate = self.rates.get(&id).copied().unwrap_or(0.);
        let eta = if rate > 0. {
            d.total
                .map(|n| duration(n.saturating_sub(d.downloaded) as f64 / rate))
        } else {
            None
        };
        panel.child(
            div()
                .id("details-scroll")
                .overflow_y_scroll()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .w_full()
                        .p(px(22.))
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(icon(Icon::File, 30., MUTED))
                                .child(
                                    div().flex_1().min_w_0().child(
                                        label(d.name.clone(), 15., TEXT)
                                            .font_weight(FontWeight::MEDIUM),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .child(
                                    div()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(rgb(status_color(d.status))),
                                )
                                .child(label(d.status.label(), 12., status_color(d.status))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_end()
                                .justify_between()
                                .child(label(format!("{:.1}%", d.fraction() * 100.), 32., TEXT))
                                .child(
                                    label(
                                        eta.map(|e| format!("{e} left"))
                                            .unwrap_or_else(|| "—".into()),
                                        12.,
                                        FAINT,
                                    )
                                    .pb(px(5.)),
                                ),
                        )
                        .child(progress(d.fraction(), status_color(d.status)).w(px(width - 44.)))
                        .child(label(
                            format!(
                                "{} of {}",
                                bytes(d.downloaded),
                                d.total.map(bytes).unwrap_or_else(|| "unknown size".into())
                            ),
                            12.,
                            MUTED,
                        ))
                        .child(div().h(px(1.)).bg(rgb(RULE)).my(px(4.)))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    label("Connections", 14., TEXT).font_weight(FontWeight::MEDIUM),
                                )
                                .child(label(format!("{:02}", d.parts.len()), 12., FAINT)),
                        )
                        .child(label(
                            if d.ranged {
                                "Parallel transfer · byte-range supported"
                            } else if d.parts.is_empty() {
                                "Waiting for the server"
                            } else {
                                "Single connection · full-file transfer"
                            },
                            11.,
                            FAINT,
                        ))
                        .children(d.parts.iter().enumerate().map(|(i, part)| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(9.))
                                .child(label(format!("{:02}", i + 1), 11., FAINT).w(px(19.)))
                                .child(div().flex_1().child(progress(
                                    if d.total.is_some() {
                                        part.fraction()
                                    } else {
                                        0.
                                    },
                                    if d.status == Status::Paused {
                                        AMBER
                                    } else {
                                        BLUE
                                    },
                                )))
                                .child(
                                    label(
                                        if d.total.is_some() {
                                            format!("{:.0}%", part.fraction() * 100.)
                                        } else {
                                            "—".into()
                                        },
                                        11.,
                                        FAINT,
                                    )
                                    .w(px(34.))
                                    .text_right(),
                                )
                        }))
                        .children(d.error.clone().map(|error| {
                            div()
                                .p(px(12.))
                                .rounded(px(8.))
                                .bg(rgb(0x291c1e))
                                .child(label(error, 12., RED).line_height(px(20.)))
                        }))
                        .child(div().h(px(1.)).bg(rgb(RULE)).my(px(4.)))
                        .child(detail_line("Source", &d.host()))
                        .child(detail_line("Destination", &d.path().display().to_string()))
                        .child(
                            button("copy-link", "Copy download link", Icon::Copy, false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(d) = this.downloads.iter().find(|d| d.id == id) {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            d.url.clone(),
                                        ));
                                    }
                                }),
                            ),
                        )
                        .child(
                            button("show-file", "Open folder", Icon::Folder, false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(d) = this.downloads.iter().find(|d| d.id == id) {
                                        reveal(&d.directory);
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                        .when(d.status == Status::Complete, |panel| {
                            panel.child(
                                button("open-file", "Open file", Icon::Arrow, true).on_click(
                                    cx.listener(move |this, _, _, _| {
                                        if let Some(d) = this.downloads.iter().find(|d| d.id == id)
                                        {
                                            reveal(&d.path());
                                        }
                                    }),
                                ),
                            )
                        })
                        .when(
                            matches!(d.status, Status::Failed | Status::Paused),
                            |panel| {
                                panel.child(
                                    button("resume-selected", "Resume download", Icon::Play, true)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.manager.resume(id);
                                            cx.notify();
                                        })),
                                )
                            },
                        )
                        .when(d.status.active() || d.status == Status::Queued, |panel| {
                            panel.child(
                                button("pause-selected", "Pause download", Icon::Pause, true)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.manager.pause(id);
                                        cx.notify();
                                    })),
                            )
                        })
                        .when(
                            !matches!(
                                d.status,
                                Status::Complete | Status::Cancelled | Status::Assembling
                            ),
                            |panel| {
                                panel.child(
                                    button(
                                        "cancel-selected",
                                        "Cancel transfer",
                                        Icon::Close,
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.manager.cancel(id);
                                            cx.notify();
                                        },
                                    )),
                                )
                            },
                        ),
                ),
        )
    }

    fn settings(&self, cx: &mut Context<Self>) -> Div {
        div().flex_1().min_h_0().min_w_0().child(div().id("settings-scroll").size_full().overflow_y_scroll().child(div().p(px(32.)).flex().flex_col().gap(px(24.))
            .child(label("Settings", 24., TEXT).font_weight(FontWeight::SEMIBOLD))
            .child(label("A few good defaults. The rest is automatic.", 14., MUTED))
            .child(div().mt(px(10.)).p(px(22.)).rounded(px(12.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Download folder", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("New downloads are saved here. Existing transfers keep their destination.", 13., MUTED))
                .child(label(self.directory.display().to_string(), 13., TEXT))
                .child(button("choose-folder", "Choose folder", Icon::Folder, false).on_click(cx.listener(|this, _, window, cx| this.choose_directory(window, cx)))))
            .child(div().p(px(22.)).rounded(px(12.)).bg(rgb(SURFACE)).flex().flex_col().gap(px(14.))
                .child(label("Connections per download", 15., TEXT).font_weight(FontWeight::MEDIUM))
                .child(label("More connections can help on some servers. Strata uses fewer for small files and servers that need a single connection.", 13., MUTED).max_w(px(620.)))
                .child(div().flex().gap(px(10.)).children([1, 4, 8, 16].into_iter().map(|n| {
                    div().id(ElementId::Name(format!("connections-{n}").into())).w(px(64.)).h(px(36.)).rounded(px(8.)).flex().items_center().justify_center().cursor_pointer()
                        .bg(rgb(if self.connections == n { TEXT } else { RAISED })).text_color(rgb(if self.connections == n { BG } else { MUTED }))
                        .focusable().focus(|s| s.border_1().border_color(rgb(BLUE))).hover(|s| s.opacity(0.85)).child(n.to_string())
                        .on_click(cx.listener(move |this, _, _, cx| this.set_connections(n, cx)))
                }))))
            .child(label("Up to 3 downloads run at once. Partial files and history are saved automatically.", 12., FAINT))))
    }
}

impl Render for Strata {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.compact = window.viewport_size().width < px(1200.);
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
            .overflow_hidden()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if event.keystroke.modifiers.control && key == "n" {
                    this.show_add(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.modifiers.control && key == "f" {
                    this.search.focus_handle(cx).focus(window);
                } else if key == "escape" {
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
            .child(self.sidebar(cx))
            .child(
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
                        d.child(self.topbar(cx)).child(self.queue(window, cx))
                    })
                    .children(
                        self.manager
                            .persistence_error
                            .lock()
                            .unwrap()
                            .clone()
                            .map(|e| label(e, 12., RED).px(px(28.)).py(px(10.))),
                    ),
            )
            .when(self.view != View::Settings, |d| d.child(self.inspector(cx)))
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
        .gap(px(12.))
        .child(label(title, 12., FAINT))
        .child(
            label(value, 24., color)
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap(),
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
        .rounded(px(8.))
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
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(RAISED)))
        .focusable()
        .focus(|s| s.bg(rgb(RAISED)))
        .child(icon(glyph, 16., MUTED))
}

fn progress(value: f32, color: u32) -> Div {
    div()
        .w_full()
        .h(px(4.))
        .flex_shrink_0()
        .rounded_full()
        .bg(rgb(RAISED))
        .overflow_hidden()
        .child(
            div()
                .w(relative(value.clamp(0., 1.)))
                .h(px(4.))
                .rounded_full()
                .bg(rgb(color)),
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
                Icon::Layers => vec![
                    vec![(3., 7.), (12., 2.), (21., 7.), (12., 12.), (3., 7.)],
                    vec![(3., 12.), (12., 17.), (21., 12.)],
                    vec![(3., 17.), (12., 22.), (21., 17.)],
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
                Icon::Grid => vec![
                    vec![(3., 3.), (9., 3.), (9., 9.), (3., 9.), (3., 3.)],
                    vec![(15., 3.), (21., 3.), (21., 9.), (15., 9.), (15., 3.)],
                    vec![(3., 15.), (9., 15.), (9., 21.), (3., 21.), (3., 15.)],
                    vec![(15., 15.), (21., 15.), (21., 21.), (15., 21.), (15., 15.)],
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
