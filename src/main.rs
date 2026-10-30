#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod brand;
mod input;
mod text;
mod ui;

use gpui::{prelude::*, *};
use std::sync::Arc;
use strata::engine::{Manager, history_path};

fn main() -> anyhow::Result<()> {
    let history = history_path();
    std::fs::create_dir_all(history.parent().unwrap())?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(history.with_extension("lock"))?;
    if lock.try_lock().is_err() {
        if std::env::args().any(|arg| arg == "--from-browser") {
            return Ok(());
        }
        show_startup_error(
            "Strata is already running. Open the existing Strata window to manage your downloads."
                .into(),
        );
        return Ok(());
    }
    let manager = match Manager::new(history.clone()) {
        Ok(manager) => manager,
        Err(error) => {
            show_startup_error(format!(
                "Strata could not open its download history.\n\n{error:#}"
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
                        title: Some("Strata — Download manager".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    window_background: WindowBackgroundAppearance::Opaque,
                    ..Default::default()
                },
                move |window, cx| {
                    window.set_rem_size(px(16.));
                    let app = cx.new(|cx| ui::Strata::new(Arc::clone(&manager), window, cx));
                    window.on_window_should_close(cx, |_, cx| {
                        cx.quit();
                        true
                    });
                    app
                },
            )
            .expect("Could not open the Strata window");
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
                .bg(rgb(0x0c0d0f))
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
                    title: Some("Strata".into()),
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
