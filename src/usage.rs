// Token accounting over Claude Code transcripts, cached incrementally on disk.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Hourly buckets older than this are dropped from the cache.
const RETENTION_DAYS: i64 = 60;
const BLOCK_HOURS: i64 = 5;
const SPARKLINE_DAYS: i64 = 14;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub messages: u64,
}

impl Tokens {
    fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
        self.messages += other.messages;
    }

    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    fn cost(&self, model: &str, cfg: &Config) -> f64 {
        let (input, output) = cfg.price_per_mtok(model);
        let m = 1_000_000.0;
        (self.input as f64 * input
            + self.output as f64 * output
            + self.cache_write as f64 * input * 1.25
            + self.cache_read as f64 * input * 0.1)
            / m
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stat {
    pub cost: f64,
    pub tokens: u64,
    pub messages: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub today: Stat,
    pub week: Stat,
    pub month: Stat,
    /// Cost inside the rolling BLOCK_HOURS window ending now.
    pub block_cost: f64,
    /// Busiest equivalent window in the retained history, used as the gauge ceiling.
    pub block_record: f64,
    /// (day, cost) for the last SPARKLINE_DAYS days, oldest first.
    pub daily: Vec<(NaiveDate, f64)>,
    /// Today's spend per model, highest first.
    pub by_model: Vec<(String, f64)>,
    /// Working directories seen in recent transcripts, most recently used first.
    pub recent_dirs: Vec<PathBuf>,
    pub error: Option<String>,
}

impl Snapshot {
    pub fn block_ratio(&self) -> f32 {
        if self.block_record <= 0.0 {
            return 0.0;
        }
        (self.block_cost / self.block_record).clamp(0.0, 1.0) as f32
    }
}

// ---------------------------------------------------------------------------
// On-disk cache
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct FileState {
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    len: u64,
    /// Last working directory this session ran in, used to populate the launch targets.
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    files: BTreeMap<String, FileState>,
    /// Flattened (unix hour, model, tokens) triples; a map would need a composite JSON key.
    #[serde(default)]
    buckets: Vec<(i64, String, Tokens)>,
}

impl Cache {
    fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(raw) = serde_json::to_string(self) {
            let _ = std::fs::write(path, raw);
        }
    }
}

// ---------------------------------------------------------------------------
// Transcript parsing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Record {
    timestamp: Option<String>,
    cwd: Option<String>,
    message: Option<RecordMessage>,
}

#[derive(Deserialize)]
struct RecordMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

fn projects_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".claude/projects");
    dir.is_dir().then_some(dir)
}

fn cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("cosmic-ext-applet-agents/usage.json")
}

fn transcripts(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => transcripts(&path, out),
            Ok(t) if t.is_file() && path.extension().is_some_and(|e| e == "jsonl") => {
                out.push(path)
            }
            _ => {}
        }
    }
}

/// Reads the bytes appended since the last scan and folds them into `buckets`.
fn ingest(path: &Path, state: &mut FileState, buckets: &mut BTreeMap<(i64, String), Tokens>) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    let len = meta.len();
    if len == state.offset {
        state.len = len;
        return;
    }

    let Ok(mut file) = File::open(path) else {
        return;
    };
    if file.seek(SeekFrom::Start(state.offset)).is_err() {
        return;
    }

    // Transcripts are append-only, so deduping within the newly read bytes is enough;
    // a shrunk file discards the whole cache in `scan` rather than being patched here.
    let mut seen: HashSet<String> = HashSet::new();
    let mut reader = BufReader::new(&mut file);
    let mut consumed = state.offset;
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(n) => {
                // A trailing partial line means an agent is mid-write; stop before it so
                // the next scan picks it up whole.
                if !line.ends_with('\n') {
                    break;
                }
                consumed += n as u64;
                if !line.contains("\"output_tokens\"") {
                    continue;
                }
                let Ok(record) = serde_json::from_str::<Record>(&line) else {
                    continue;
                };
                let (Some(ts), Some(msg)) = (record.timestamp, record.message) else {
                    continue;
                };
                if record.cwd.is_some() {
                    state.cwd = record.cwd;
                }
                let Some(raw) = msg.usage else { continue };
                if let Some(id) = msg.id {
                    if !seen.insert(id) {
                        continue;
                    }
                }
                let Ok(at) = DateTime::parse_from_rfc3339(&ts) else {
                    continue;
                };
                let hour = at.with_timezone(&Utc).timestamp().div_euclid(3600);
                let model = msg.model.unwrap_or_else(|| "unknown".into());
                buckets.entry((hour, model)).or_default().add(&Tokens {
                    input: raw.input_tokens,
                    output: raw.output_tokens,
                    cache_write: raw.cache_creation_input_tokens,
                    cache_read: raw.cache_read_input_tokens,
                    messages: 1,
                });
            }
            Err(_) => break,
        }
    }

    state.offset = consumed;
    state.len = len;
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

/// Rescans the transcripts and rebuilds the displayed figures. Blocking: keep it off the UI thread.
pub fn scan(cfg: &Config) -> Snapshot {
    let Some(root) = projects_dir() else {
        return Snapshot {
            error: Some("No ~/.claude/projects directory".into()),
            ..Default::default()
        };
    };

    let cache_file = cache_path();
    let mut cache = Cache::load(&cache_file);
    let mut buckets: BTreeMap<(i64, String), Tokens> = cache
        .buckets
        .drain(..)
        .map(|(hour, model, tokens)| ((hour, model), tokens))
        .collect();

    let mut files = Vec::new();
    transcripts(&root, &mut files);
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();

    let mut states = std::mem::take(&mut cache.files);
    // A transcript that shrank was rewritten, so every cached offset is suspect.
    let rewritten = files.iter().any(|path| {
        states
            .get(&path.to_string_lossy().into_owned())
            .zip(std::fs::metadata(path).ok())
            .is_some_and(|(state, meta)| meta.len() < state.len)
    });
    if rewritten {
        tracing::info!("transcript rewritten, rebuilding the usage cache");
        states.clear();
        buckets.clear();
    }

    let mut next_states = BTreeMap::new();
    for path in &files {
        let key = path.to_string_lossy().into_owned();
        let mut state = states.remove(&key).unwrap_or_default();
        ingest(path, &mut state, &mut buckets);
        let touched = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if let Some(cwd) = state.cwd.clone() {
            dirs.push((touched, PathBuf::from(cwd)));
        }
        next_states.insert(key, state);
    }

    let now = Local::now();
    let cutoff_hour = (now - Duration::days(RETENTION_DAYS))
        .timestamp()
        .div_euclid(3600);
    buckets.retain(|(hour, _), _| *hour >= cutoff_hour);

    cache.files = next_states;
    cache.buckets = buckets
        .iter()
        .map(|((hour, model), tokens)| (*hour, model.clone(), *tokens))
        .collect();
    cache.save(&cache_file);

    let mut snapshot = aggregate(&buckets, cfg, now);
    snapshot.recent_dirs = rank_dirs(dirs);
    snapshot
}

/// Most recently used existing directories, newest first, without duplicates.
fn rank_dirs(mut dirs: Vec<(std::time::SystemTime, PathBuf)>) -> Vec<PathBuf> {
    dirs.sort_by_key(|(touched, _)| std::cmp::Reverse(*touched));
    let mut seen = HashSet::new();
    dirs.into_iter()
        .map(|(_, dir)| dir)
        .filter(|dir| dir.is_dir() && seen.insert(dir.clone()))
        .take(12)
        .collect()
}

fn hour_start(at: DateTime<Local>) -> i64 {
    at.timestamp().div_euclid(3600)
}

fn aggregate(
    buckets: &BTreeMap<(i64, String), Tokens>,
    cfg: &Config,
    now: DateTime<Local>,
) -> Snapshot {
    let today = now.date_naive();
    let week_start = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let month_start = today.with_day(1).unwrap_or(today);
    let sparkline_start = today - Duration::days(SPARKLINE_DAYS - 1);
    let block_start = hour_start(now) - (BLOCK_HOURS - 1);

    let mut snapshot = Snapshot::default();
    let mut daily: BTreeMap<NaiveDate, f64> = BTreeMap::new();
    let mut by_model: BTreeMap<String, f64> = BTreeMap::new();
    let mut hourly_cost: BTreeMap<i64, f64> = BTreeMap::new();

    for ((hour, model), tokens) in buckets {
        let cost = tokens.cost(model, cfg);
        *hourly_cost.entry(*hour).or_default() += cost;

        let Some(at) = Utc.timestamp_opt(hour * 3600, 0).single() else {
            continue;
        };
        let day = at.with_timezone(&Local).date_naive();

        if day >= sparkline_start {
            *daily.entry(day).or_default() += cost;
        }
        if day >= month_start {
            accumulate(&mut snapshot.month, cost, tokens);
        }
        if day >= week_start {
            accumulate(&mut snapshot.week, cost, tokens);
        }
        if day == today {
            accumulate(&mut snapshot.today, cost, tokens);
            *by_model.entry(pretty_model(model)).or_default() += cost;
        }
        if *hour >= block_start {
            snapshot.block_cost += cost;
        }
    }

    // The busiest BLOCK_HOURS window in the retained history sets the gauge ceiling.
    let hours: Vec<i64> = hourly_cost.keys().copied().collect();
    for end in &hours {
        let window: f64 = hourly_cost
            .range((end - BLOCK_HOURS + 1)..=*end)
            .map(|(_, cost)| cost)
            .sum();
        snapshot.block_record = snapshot.block_record.max(window);
    }

    snapshot.daily = (0..SPARKLINE_DAYS)
        .map(|offset| {
            let day = sparkline_start + Duration::days(offset);
            (day, daily.get(&day).copied().unwrap_or(0.0))
        })
        .collect();

    let mut models: Vec<(String, f64)> = by_model.into_iter().collect();
    models.sort_by(|a, b| b.1.total_cmp(&a.1));
    snapshot.by_model = models;
    snapshot
}

fn accumulate(stat: &mut Stat, cost: f64, tokens: &Tokens) {
    stat.cost += cost;
    stat.tokens += tokens.total();
    stat.messages += tokens.messages;
}

/// Turns `claude-opus-5` into `Opus 5`, leaving unknown ids readable.
pub fn pretty_model(model: &str) -> String {
    let stripped = model.strip_prefix("claude-").unwrap_or(model);
    let mut parts = stripped.split('-');
    let Some(family) = parts.next() else {
        return model.to_string();
    };
    let version: Vec<&str> = parts.collect();
    let mut name: String = family
        .char_indices()
        .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c })
        .collect();
    if !version.is_empty() {
        name.push(' ');
        name.push_str(&version.join("."));
    }
    name
}

pub fn format_tokens(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}K", tokens as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", tokens as f64 / 1e6),
        _ => format!("{:.2}B", tokens as f64 / 1e9),
    }
}

pub fn format_cost(cost: f64) -> String {
    if cost >= 100.0 {
        format!("${cost:.0}")
    } else if cost >= 10.0 {
        format!("${cost:.1}")
    } else {
        format!("${cost:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(hour: i64, model: &str, tokens: Tokens) -> ((i64, String), Tokens) {
        ((hour, model.to_string()), tokens)
    }

    #[test]
    fn costs_and_windows() {
        let cfg = Config::default();
        let now = Local.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
        let this_hour = hour_start(now);

        let buckets: BTreeMap<(i64, String), Tokens> = [
            // 1M input + 1M output on Opus 5 -> $5 + $25.
            bucket(
                this_hour,
                "claude-opus-5",
                Tokens {
                    input: 1_000_000,
                    output: 1_000_000,
                    messages: 3,
                    ..Default::default()
                },
            ),
            // Ten hours back: inside the month, outside the 5h block.
            bucket(
                this_hour - 10,
                "claude-sonnet-5",
                Tokens {
                    input: 1_000_000,
                    messages: 1,
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect();

        let snapshot = aggregate(&buckets, &cfg, now);
        assert!((snapshot.today.cost - 32.0).abs() < 1e-6, "{snapshot:?}");
        assert!((snapshot.block_cost - 30.0).abs() < 1e-6);
        assert!((snapshot.block_record - 30.0).abs() < 1e-6);
        assert_eq!(snapshot.today.messages, 4);
        assert_eq!(snapshot.daily.len() as i64, SPARKLINE_DAYS);
        assert_eq!(snapshot.by_model[0].0, "Opus 5");
    }

    #[test]
    fn model_names_and_units() {
        assert_eq!(pretty_model("claude-opus-5"), "Opus 5");
        assert_eq!(pretty_model("claude-haiku-4-5"), "Haiku 4.5");
        assert_eq!(format_tokens(1_500), "1.5K");
        assert_eq!(format_tokens(2_400_000), "2.4M");
        assert_eq!(format_cost(3.456), "$3.46");
    }

    #[test]
    fn ingest_is_incremental() {
        let dir = std::env::temp_dir().join(format!("agents-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let line = |id: &str| {
            format!(
                r#"{{"timestamp":"2026-09-21T10:00:00.000Z","type":"assistant","message":{{"id":"{id}","model":"claude-opus-5","usage":{{"input_tokens":10,"output_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
            )
        };
        std::fs::write(&path, format!("{}\n{}\n", line("a"), line("a"))).unwrap();

        let mut state = FileState::default();
        let mut buckets = BTreeMap::new();
        ingest(&path, &mut state, &mut buckets);
        // The duplicate id is dropped.
        assert_eq!(buckets.values().next().unwrap().messages, 1);

        // A second pass over unchanged bytes adds nothing.
        let before = state.offset;
        ingest(&path, &mut state, &mut buckets);
        assert_eq!(state.offset, before);
        assert_eq!(buckets.values().next().unwrap().messages, 1);

        // Appended lines are picked up, a partial trailing line is not.
        let mut appended = std::fs::read_to_string(&path).unwrap();
        appended.push_str(&format!("{}\n{}", line("b"), r#"{"partial":"#));
        std::fs::write(&path, appended).unwrap();
        ingest(&path, &mut state, &mut buckets);
        assert_eq!(buckets.values().next().unwrap().messages, 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
