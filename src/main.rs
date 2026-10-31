#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod brand;
mod datetime;
mod input;
mod scroll;
mod text;
mod tray;
mod ui;

use gpui::{prelude::*, *};
use std::rc::Rc;
use std::sync::Arc;
use strata::engine::{Manager, history_path};

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--register-browser-host") {
        return strata::bridge::register_browsers();
    }
    if std::env::args().any(|arg| arg == "--unregister-browser-host") {
        return strata::bridge::unregister_browsers();
    }
    if std::env::args().len() > 1 && !std::env::args().any(|arg| arg == "--from-browser") {
        use clap::Parser;
        strata::cli::enqueue(strata::cli::Cli::parse())?;
        return Ok(());
    }
    let history = history_path();
    std::fs::create_dir_all(history.parent().unwrap())?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(history.with_extension("lock"))?;
    if lock.try_lock().is_err() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        if runtime
            .block_on(strata::bridge::send(
                &strata::bridge::Request::Open,
                &history,
                false,
            ))
            .is_ok_and(|reply| reply.ok)
        {
            return Ok(());
        }
        if std::env::args().any(|arg| arg == "--from-browser") {
            return Ok(());
        }
        show_startup_error(
            "101 Downloader is already running. Open the existing 101 Downloader window to manage your downloads."
                .into(),
        );
        return Ok(());
    }
    // Keep the installer from replacing a running app, including its hidden tray window.
    let _installer_mutex = unsafe {
        windows::Win32::System::Threading::CreateMutexW(
            None,
            false,
            windows::core::w!("Local\\Strata.DownloadManager"),
        )?
    };
    let manager = match Manager::new(history.clone()) {
        Ok(manager) => manager,
        Err(error) => {
            show_startup_error(format!(
                "101 Downloader could not open its download history.\n\n{error:#}"
            ));
            return Ok(());
        }
    };
    let _browser_listener = match strata::bridge::start(manager.clone(), &history) {
        Ok(listener) => Some(listener),
        Err(error) => {
            *manager.browser_error.lock().unwrap() = Some(format!("{error:#}"));
            None
        }
    };
    let shutdown = manager.clone();
    Application::new()
        .with_assets(brand::BrandAssets)
        .run(move |cx: &mut App| {
            input::TextInput::bind_keys(cx);
            cx.on_app_quit(move |_| {
                shutdown.shutdown();
                async {}
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1380.), px(880.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1020.), px(700.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("101 Downloader".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    window_background: WindowBackgroundAppearance::Opaque,
                    ..Default::default()
                },
                move |window, cx| {
                    window.set_rem_size(px(16.));
                    let (tray, tray_error) = match tray::Tray::new(window) {
                        Ok(tray) => (Some(Rc::new(tray)), None),
                        Err(error) => (None, Some(format!("System tray unavailable. Closing the window will quit 101 Downloader. {error:#}"))),
                    };
                    if let Some(tray) = tray.clone() {
                        cx.on_app_quit(move |_| {
                            tray.remove_icon();
                            async {}
                        }).detach();
                    }
                    let app = cx.new(|cx| {
                        ui::Strata::new(Arc::clone(&manager), tray.clone(), tray_error, window, cx)
                    });
                    window.on_window_should_close(cx, move |_, cx| {
                        if let Some(tray) = &tray && !tray.quitting() {
                            tray.hide();
                            false
                        } else {
                            cx.quit();
                            true
                        }
                    });
                    app
                },
            )
            .expect("Could not open the 101 Downloader window");
            cx.activate(true);
        });
    Ok(())
}

fn show_startup_error(message: String) {
    struct StartupError(String);
    impl Render for StartupError {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .bg(rgb(0x000000))
                .p_8()
                .text_color(rgb(0xf0f1f2))
                .font_family("Segoe UI")
                .text_size(px(15.))
                .line_height(px(24.))
                .child(self.0.clone())
        }
    }
    Application::new().run(move |cx| {
        let bounds = Bounds::centered(None, size(px(560.), px(240.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("101 Downloader".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    true
                });
                cx.new(|_| StartupError(message))
            },
        )
        .expect("Could not open the error window");
        cx.activate(true);
    });
}
