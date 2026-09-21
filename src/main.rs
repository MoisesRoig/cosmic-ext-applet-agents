// SPDX-License-Identifier: GPL-3.0-only

mod agents;
mod config;
mod usage;
mod window;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // `--scan` prints the figures behind the popup, in dollars, without a panel.
    if std::env::args().any(|arg| arg == "--scan") {
        let started = std::time::Instant::now();
        let snapshot = usage::scan(&config::Config::load());
        let elapsed = started.elapsed();

        for (label, stat) in [
            ("today", &snapshot.today),
            ("week ", &snapshot.week),
            ("month", &snapshot.month),
        ] {
            println!(
                "{label}  {:>8}  {:>8} tokens  {:>5} messages",
                usage::format_cost(stat.cost),
                usage::format_tokens(stat.tokens),
                stat.messages
            );
        }
        println!(
            "block  {:>8}  peak day {}  peak week {}",
            usage::format_cost(snapshot.block_cost),
            usage::format_cost(snapshot.peak_day),
            usage::format_cost(snapshot.peak_week)
        );
        for (model, cost) in &snapshot.by_model {
            println!("  {model}: {}", usage::format_cost(*cost));
        }
        for proc in agents::running() {
            println!("running  {} in {}", proc.agent_id, proc.cwd.display());
        }
        println!("installed {:?}", agents::installed());
        println!("scanned in {elapsed:?}");
        return Ok(());
    }

    let preview = std::env::args().any(|arg| arg == "--preview");
    cosmic::applet::run::<window::Window>(preview)
}
