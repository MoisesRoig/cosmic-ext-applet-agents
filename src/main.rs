// SPDX-License-Identifier: GPL-3.0-only

mod agents;
mod config;
mod usage;
mod window;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // `--scan` prints what the popup would show, without a panel.
    if std::env::args().any(|arg| arg == "--scan") {
        let started = std::time::Instant::now();
        let snapshot = usage::scan(&config::Config::load());
        println!("{snapshot:#?}\nscanned in {:?}", started.elapsed());
        println!("running: {:#?}", agents::running());
        println!("installed: {:?}", agents::installed());
        return Ok(());
    }

    let preview = std::env::args().any(|arg| arg == "--preview");
    cosmic::applet::run::<window::Window>(preview)
}
