// Panel button and popup: usage gauges on top, running agents, launcher grid.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use cosmic::app::{self, Core};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::widget::{column, row};
use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Color, Length, Subscription};
use cosmic::widget::{
    autosize, button, container, divider, dropdown, flex_row, icon, text, toggler,
};
use cosmic::{theme, Element, Task};

use crate::agents::{self, Agent, Running};
use crate::config::Config;
use crate::usage::{self, Snapshot};

const ID: &str = "io.github.MoisesRoig.cosmic-ext-applet-agents";
const ICON: &[u8] = include_bytes!("../data/icon.svg");
const POPUP_WIDTH: f32 = 380.0;
const SPARKLINE_HEIGHT: f32 = 34.0;
/// Process list refresh; the usage rescan runs every USAGE_EVERY ticks.
const TICK: Duration = Duration::from_secs(4);
const USAGE_EVERY: u32 = 8;

/// The panel button grows when a running count is shown, so it has to autosize.
static AUTOSIZE_ID: LazyLock<cosmic::widget::Id> =
    LazyLock::new(|| cosmic::widget::Id::new("agents-applet"));

pub struct Window {
    core: Core,
    popup: Option<Id>,
    config: Config,
    snapshot: Snapshot,
    running: Vec<Running>,
    installed: std::collections::BTreeSet<&'static str>,
    dirs: Vec<PathBuf>,
    dir_labels: Vec<String>,
    selected_dir: usize,
    ticks: u32,
    scanning: bool,
    /// Draw the popup contents in the main window, for designing it outside a panel.
    preview: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    Tick,
    UsageLoaded(Box<Snapshot>),
    Launch(&'static str),
    SelectDir(usize),
    OpenDir(PathBuf),
    OpenConfig,
    ToggleKeepShell(bool),
}

impl Window {
    fn rescan(&mut self) -> app::Task<Message> {
        if self.scanning {
            return Task::none();
        }
        self.scanning = true;
        let config = self.config.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || usage::scan(&config))
                    .await
                    .unwrap_or_default()
            },
            |snapshot| cosmic::action::app(Message::UsageLoaded(Box::new(snapshot))),
        )
    }

    /// Rebuilds the launch target list, keeping the current selection pinned to its path.
    fn refresh_dirs(&mut self) {
        let selected = self.dirs.get(self.selected_dir).cloned();
        let home = std::env::var_os("HOME").map(PathBuf::from);

        let candidates = self
            .config
            .project_dirs
            .iter()
            .map(PathBuf::from)
            .chain(self.running.iter().map(|proc| proc.cwd.clone()))
            .chain(self.snapshot.recent_dirs.iter().cloned())
            .chain(home.clone());

        let mut dirs: Vec<PathBuf> = Vec::new();
        for dir in candidates {
            if dir.is_dir() && !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        if dirs.is_empty() {
            dirs.push(home.unwrap_or_else(|| PathBuf::from(".")));
        }

        self.selected_dir = selected
            .and_then(|prev| dirs.iter().position(|dir| *dir == prev))
            .unwrap_or(0);
        self.dir_labels = dirs.iter().map(|dir| tilde(dir)).collect();
        self.dirs = dirs;
    }

    fn launch_dir(&self) -> PathBuf {
        self.dirs
            .get(self.selected_dir)
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

impl cosmic::Application for Window {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = bool;
    type Message = Message;
    const APP_ID: &'static str = ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, preview: Self::Flags) -> (Self, app::Task<Self::Message>) {
        let mut window = Self {
            core,
            preview,
            popup: None,
            config: Config::load(),
            snapshot: Snapshot::default(),
            running: agents::running(),
            installed: agents::installed(),
            dirs: Vec::new(),
            dir_labels: Vec::new(),
            selected_dir: 0,
            ticks: 0,
            scanning: false,
        };
        if preview {
            window.snapshot = usage::demo();
            window.running = agents::demo_running();
            window.installed = agents::demo_installed();
            window.dirs = vec![PathBuf::from("/home/you/code/storefront")];
            window.dir_labels = vec!["~/code/storefront".into()];
            return (window, Task::none());
        }

        window.refresh_dirs();
        let task = window.rescan();
        (window, task)
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        if self.preview {
            return Subscription::none();
        }
        cosmic::iced::time::every(TICK).map(|_| Message::Tick)
    }

    fn update(&mut self, message: Self::Message) -> app::Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    return destroy_popup(id);
                }
                let Some(parent) = self.core.main_window_id() else {
                    return Task::none();
                };
                let new_id = Id::unique();
                self.popup = Some(new_id);
                let settings = self
                    .core
                    .applet
                    .get_popup_settings(parent, new_id, None, None, None);
                self.running = agents::running();
                self.installed = agents::installed();
                self.refresh_dirs();
                return Task::batch([get_popup(settings), self.rescan()]);
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
            Message::Tick => {
                self.ticks = self.ticks.wrapping_add(1);
                let running = agents::running();
                if running != self.running {
                    self.running = running;
                    self.refresh_dirs();
                }
                if self.ticks.is_multiple_of(USAGE_EVERY) {
                    return self.rescan();
                }
            }
            Message::UsageLoaded(snapshot) => {
                self.scanning = false;
                self.snapshot = *snapshot;
                self.refresh_dirs();
            }
            Message::Launch(id) => {
                let Some(agent) = agents::by_id(id) else {
                    return Task::none();
                };
                let dir = self.launch_dir();
                match agents::launch_command(agent, &dir, &self.config) {
                    Some(cmd) => {
                        tokio::spawn(cosmic::process::spawn(cmd));
                    }
                    None => tracing::warn!("empty terminal template in config, nothing launched"),
                }
                if let Some(popup) = self.popup.take() {
                    return destroy_popup(popup);
                }
            }
            Message::SelectDir(index) => self.selected_dir = index,
            Message::OpenDir(dir) => {
                let mut cmd = std::process::Command::new("xdg-open");
                cmd.arg(dir);
                tokio::spawn(cosmic::process::spawn(cmd));
            }
            Message::OpenConfig => {
                let mut cmd = std::process::Command::new("xdg-open");
                cmd.arg(Config::path());
                tokio::spawn(cosmic::process::spawn(cmd));
            }
            Message::ToggleKeepShell(value) => {
                self.config.keep_shell_open = value;
                let config = self.config.clone();
                std::thread::spawn(move || config.save());
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if self.preview {
            return self.view_window(Id::unique());
        }
        let suggested = self.core.applet.suggested_size(true);
        let (major, minor) = self.core.applet.suggested_padding(true);
        let glyph = icon::icon(icon::from_svg_bytes(ICON).symbolic(true))
            .class(theme::Svg::custom(|theme| {
                cosmic::iced::widget::svg::Style {
                    color: Some(theme.cosmic().background(theme.transparent).on.into()),
                }
            }))
            .width(Length::Fixed(f32::from(suggested.0)))
            .height(Length::Fixed(f32::from(suggested.1)));

        let content: Element<'_, Message> =
            if self.core.applet.is_horizontal() && !self.running.is_empty() {
                row![glyph, self.core.applet.text(self.running.len().to_string())]
                    .align_y(Alignment::Center)
                    .spacing(4)
                    .into()
            } else {
                glyph.into()
            };

        let (horizontal, vertical) = if self.core.applet.is_horizontal() {
            (major, minor)
        } else {
            (minor, major)
        };

        let button = button::custom(content)
            .padding([vertical, horizontal])
            .class(theme::Button::AppletIcon)
            .on_press_down(Message::TogglePopup);

        autosize::autosize(button, AUTOSIZE_ID.clone()).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let cosmic::cosmic_theme::Spacing {
            space_xxxs,
            space_xxs,
            space_xs,
            space_s,
            ..
        } = theme::active().cosmic().spacing;

        let mut content = column![
            header(&self.running),
            usage_card(&self.snapshot, &self.config)
        ]
        .spacing(space_xs)
        .padding([space_xs, space_s]);

        if let Some(error) = &self.snapshot.error {
            content = content.push(text::caption(error.clone()));
        }

        if !self.running.is_empty() {
            content = content.push(divider::horizontal::default());
            content = content.push(section("Running now"));
            let mut list = column![].spacing(space_xxxs);
            for proc in &self.running {
                list = list.push(running_row(proc));
            }
            content = content.push(list);
        }

        content = content.push(divider::horizontal::default());
        content = content.push(section(format!(
            "Launch  ({} of {} agents installed)",
            self.installed.len(),
            agents::CATALOG.len()
        )));
        content = content.push(
            dropdown(
                &self.dir_labels,
                Some(self.selected_dir),
                Message::SelectDir,
            )
            .width(Length::Fill),
        );

        let tiles: Vec<Element<'_, Message>> = agents::CATALOG
            .iter()
            .filter(|agent| self.installed.contains(agent.id))
            .map(agent_tile)
            .collect();

        content = content.push(if tiles.is_empty() {
            Element::from(text::caption(
                "No agent CLIs found on PATH. Install one, then reopen this menu.",
            ))
        } else {
            flex_row(tiles)
                .row_spacing(space_xxs)
                .column_spacing(space_xxs)
                .into()
        });

        content = content.push(divider::horizontal::default());
        content = content.push(
            toggler(self.config.keep_shell_open)
                .label("Keep terminal open on exit".to_string())
                .text_size(12)
                .width(Length::Fill)
                .on_toggle(Message::ToggleKeepShell),
        );
        content = content.push(
            button::custom(text::body("Edit settings file").width(Length::Fill))
                .padding([6, 8])
                .width(Length::Fill)
                .class(theme::Button::MenuItem)
                .on_press(Message::OpenConfig),
        );

        self.core
            .applet
            .popup_container(container(content).width(Length::Fixed(POPUP_WIDTH)))
            .into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

// ---------------------------------------------------------------------------
// View helpers
// ---------------------------------------------------------------------------

fn rgb(accent: [u8; 3]) -> Color {
    Color::from_rgb8(accent[0], accent[1], accent[2])
}

fn header(running: &[Running]) -> Element<'_, Message> {
    let badge: Element<'_, Message> = if running.is_empty() {
        text::caption("idle").into()
    } else {
        pill(
            format!("{} running", running.len()),
            Color::from_rgb8(46, 204, 113),
        )
    };

    row![
        text::title4("AI Agents").width(Length::Fill),
        container(badge).align_y(Alignment::Center),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn section(label: impl AsRef<str>) -> Element<'static, Message> {
    text::caption_heading(label.as_ref().to_uppercase()).into()
}

fn pill(label: String, color: Color) -> Element<'static, Message> {
    container(text::caption(label))
        .padding([2, 8])
        .class(theme::Container::custom(move |t| {
            cosmic::widget::container::Style {
                text_color: Some(color),
                background: Some(cosmic::iced::Background::Color(Color { a: 0.18, ..color })),
                border: cosmic::iced::Border {
                    radius: t.cosmic().corner_radii.radius_m.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

fn usage_card<'a>(snapshot: &'a Snapshot, cfg: &Config) -> Element<'a, Message> {
    let spacing = theme::active().cosmic().spacing;

    let stats = row![
        share_tile(
            "Today",
            snapshot.today.cost,
            cfg.daily_budget,
            snapshot.peak_day
        ),
        share_tile(
            "This week",
            snapshot.week.cost,
            cfg.weekly_budget,
            snapshot.peak_week
        ),
        stat_tile("Tokens today", usage::format_tokens(snapshot.today.tokens)),
    ]
    .spacing(spacing.space_xxs);

    let mut card =
        column![stats, sparkline(snapshot), block_gauge(snapshot)].spacing(spacing.space_xxs);

    if !snapshot.by_model.is_empty() {
        let total: f64 = snapshot.by_model.iter().map(|(_, cost)| cost).sum();
        let chips: Vec<Element<'_, Message>> = snapshot
            .by_model
            .iter()
            .take(3)
            .map(|(model, cost)| {
                let share = if total > 0.0 {
                    cost / total * 100.0
                } else {
                    0.0
                };
                text::caption(format!("{model} {share:.0}%")).into()
            })
            .collect();
        card = card.push(row(chips).spacing(spacing.space_s));
    }

    container(card)
        .padding(spacing.space_xxs)
        .class(theme::Container::Card)
        .width(Length::Fill)
        .into()
}

/// A percentage of the configured budget, or of the busiest period on record when
/// no budget is set. Keeping spend out of the panel also keeps it out of screenshots.
fn share_tile<'a>(
    label: &'a str,
    spent: f64,
    budget: Option<f64>,
    peak: f64,
) -> Element<'a, Message> {
    let (basis, basis_name) = match budget {
        Some(budget) if budget > 0.0 => (budget, "of budget"),
        _ => (peak, "of peak"),
    };
    let ratio = if basis > 0.0 {
        (spent / basis) as f32
    } else {
        0.0
    };
    container(
        column![
            text::title4(format!("{}%", percent(ratio))),
            text::caption(format!("{label}, {basis_name}")),
        ]
        .spacing(2)
        .align_x(Alignment::Start),
    )
    .width(Length::Fill)
    .into()
}

fn percent(ratio: f32) -> u32 {
    (ratio * 100.0).round().clamp(0.0, 999.0) as u32
}

fn stat_tile(label: &str, value: String) -> Element<'_, Message> {
    container(
        column![text::title4(value), text::caption(label)]
            .spacing(2)
            .align_x(Alignment::Start),
    )
    .width(Length::Fill)
    .into()
}

/// Fourteen daily bars; today is drawn in the accent colour.
fn sparkline(snapshot: &Snapshot) -> Element<'_, Message> {
    let max = snapshot
        .daily
        .iter()
        .map(|(_, cost)| *cost)
        .fold(0.0_f64, f64::max);
    if max <= 0.0 {
        return text::caption("No usage recorded in the last 14 days").into();
    }

    let last = snapshot.daily.len().saturating_sub(1);
    let bars: Vec<Element<'_, Message>> = snapshot
        .daily
        .iter()
        .enumerate()
        .map(|(index, (_, cost))| {
            let ratio = (cost / max) as f32;
            let height = (SPARKLINE_HEIGHT * ratio).max(2.0);
            let today = index == last;
            container(
                container(cosmic::widget::Space::new().width(Length::Fill))
                    .height(Length::Fixed(height))
                    .width(Length::Fill)
                    .class(theme::Container::custom(move |t| {
                        let cosmic = t.cosmic();
                        let color: Color = if today {
                            cosmic.accent_color().into()
                        } else {
                            let mut base: Color = cosmic.on_bg_color().into();
                            base.a = 0.35;
                            base
                        };
                        cosmic::widget::container::Style {
                            background: Some(cosmic::iced::Background::Color(color)),
                            border: cosmic::iced::Border {
                                radius: 2.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })),
            )
            .height(Length::Fixed(SPARKLINE_HEIGHT))
            .align_y(Alignment::End)
            .width(Length::Fill)
            .into()
        })
        .collect();

    column![
        row(bars).spacing(3).align_y(Alignment::End),
        text::caption("Last 14 days"),
    ]
    .spacing(2)
    .into()
}

/// Rolling five-hour spend against the busiest five hours on record.
fn block_gauge(snapshot: &Snapshot) -> Element<'_, Message> {
    let ratio = snapshot.block_ratio();
    let bar = container(cosmic::widget::Space::new().width(Length::Fill))
        .height(Length::Fixed(6.0))
        .width(Length::FillPortion((ratio * 1000.0) as u16 + 1))
        .class(theme::Container::custom(|t| {
            cosmic::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(
                    t.cosmic().accent_color().into(),
                )),
                border: cosmic::iced::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));
    let rest = cosmic::widget::Space::new()
        .width(Length::FillPortion(((1.0 - ratio) * 1000.0) as u16 + 1));

    let track = container(row![bar, rest].height(Length::Fixed(6.0)))
        .width(Length::Fill)
        .class(theme::Container::custom(|t| {
            let mut color: Color = t.cosmic().on_bg_color().into();
            color.a = 0.15;
            cosmic::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(color)),
                border: cosmic::iced::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));

    column![
        row![
            text::caption("Last 5 hours").width(Length::Fill),
            text::caption(format!("{}% of peak", percent(ratio))),
        ],
        track,
    ]
    .spacing(3)
    .into()
}

fn running_row(proc: &Running) -> Element<'_, Message> {
    let accent = agents::by_id(proc.agent_id)
        .map(|agent| rgb(agent.accent))
        .unwrap_or(Color::WHITE);
    let name = agents::by_id(proc.agent_id)
        .map(|agent| agent.name)
        .unwrap_or(proc.agent_id);

    let dot = container(cosmic::widget::Space::new().width(Length::Fixed(8.0)))
        .height(Length::Fixed(8.0))
        .class(theme::Container::custom(move |_| {
            cosmic::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(accent)),
                border: cosmic::iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));

    button::custom(
        row![
            dot,
            column![text::body(name), text::caption(tilde(&proc.cwd))]
                .spacing(0)
                .width(Length::Fill),
            text::caption(format_uptime(proc.uptime)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .width(Length::Fill)
    .class(theme::Button::MenuItem)
    .on_press(Message::OpenDir(proc.cwd.clone()))
    .into()
}

fn agent_tile(agent: &'static Agent) -> Element<'static, Message> {
    let accent = rgb(agent.accent);
    let badge = container(text::heading(agent.badge()))
        .center_x(Length::Fixed(30.0))
        .center_y(Length::Fixed(30.0))
        .class(theme::Container::custom(move |t| {
            cosmic::widget::container::Style {
                text_color: Some(accent),
                background: Some(cosmic::iced::Background::Color(Color { a: 0.2, ..accent })),
                border: cosmic::iced::Border {
                    color: Color { a: 0.5, ..accent },
                    width: 1.0,
                    radius: t.cosmic().corner_radii.radius_s.into(),
                },
                ..Default::default()
            }
        }));

    button::custom(
        row![badge, text::body(agent.name)]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([6, 8])
    .class(theme::Button::MenuItem)
    .on_press(Message::Launch(agent.id))
    .into()
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

fn tilde(path: &Path) -> String {
    let display = path.to_string_lossy();
    match std::env::var("HOME") {
        Ok(home) if display.starts_with(&home) => format!("~{}", &display[home.len()..]),
        _ => display.into_owned(),
    }
}

fn format_uptime(uptime: Duration) -> String {
    let secs = uptime.as_secs();
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_reads_naturally() {
        assert_eq!(format_uptime(Duration::from_secs(5)), "5s");
        assert_eq!(format_uptime(Duration::from_secs(600)), "10m");
        assert_eq!(format_uptime(Duration::from_secs(3_900)), "1h 05m");
    }

    #[test]
    fn home_is_abbreviated() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert_eq!(tilde(&PathBuf::from(format!("{home}/code"))), "~/code");
        }
        assert_eq!(tilde(&PathBuf::from("/srv/app")), "/srv/app");
    }
}
