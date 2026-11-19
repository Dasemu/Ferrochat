//! Entry point: logging, config, `iced::application(..).run()`.
#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;

use ferrochat::app::App;
use ferrochat::config::{APP_ID, Cli, Config, USAGE};
use ferrochat::ui::{self, theme};

fn init_logging(cfg: &Config) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,whatsapp_rust=warn,wacore=warn,iced=warn,wgpu=warn,naga=warn")
    });
    let file = (!cfg.mock)
        .then(|| ferrochat::config::open_log_file(&cfg.state_dir))
        .flatten();
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_target(false));
    match file {
        Some(f) => registry
            .with(
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(f)),
            )
            .init(),
        None => registry.init(),
    }
}

fn main() -> iced::Result {
    // Default to the CPU renderer: in measurements it idles at ~43 MB RSS
    // versus ~177 MB for wgpu (software Vulkan); ICED_BACKEND=wgpu opts in.
    if std::env::var_os("ICED_BACKEND").is_none() {
        // SAFETY: single-threaded at this point, before any thread is spawned.
        unsafe { std::env::set_var("ICED_BACKEND", "tiny-skia") };
    }
    let cfg = match Config::from_args(std::env::args().skip(1)) {
        Cli::Run(cfg) => *cfg,
        Cli::Help => {
            let _ = std::io::stdout().write_all(USAGE.as_bytes());
            return Ok(());
        }
        Cli::Invalid(msg) => {
            eprintln!("ferrochat: {msg}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    init_logging(&cfg);

    let size = if cfg.screenshot.is_some() {
        iced::Size::new(1280.0, 800.0)
    } else {
        iced::Size::new(1200.0, 780.0)
    };
    let window = iced::window::Settings {
        size,
        min_size: Some(iced::Size::new(760.0, 520.0)),
        platform_specific: iced::window::settings::PlatformSpecific {
            #[cfg(target_os = "linux")]
            application_id: APP_ID.into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let _ = APP_ID;

    iced::application(move || App::boot(cfg.clone()), App::update, ui::view)
        .title(App::title)
        .theme(App::iced_theme)
        .subscription(App::subscription)
        .font(theme::FONT_REGULAR)
        .font(theme::FONT_MEDIUM)
        .font(theme::FONT_SEMIBOLD)
        .default_font(theme::INTER)
        .window(window)
        .antialiasing(true)
        .run()
}
