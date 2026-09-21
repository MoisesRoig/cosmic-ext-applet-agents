// User settings, stored as plain JSON so the file can be edited by hand.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Input and output price per million tokens, keyed by a model id substring.
/// The longest matching key wins, so `sonnet-4-6` overrides `sonnet`.
fn default_pricing() -> BTreeMap<String, [f64; 2]> {
    [
        ("default", [5.0, 25.0]),
        ("fable", [10.0, 50.0]),
        ("mythos", [10.0, 50.0]),
        ("opus", [5.0, 25.0]),
        ("sonnet", [2.0, 10.0]),
        ("sonnet-4-6", [3.0, 15.0]),
        ("haiku", [1.0, 5.0]),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Terminal invocation. `%d` is the working directory, `%c` the agent command line
/// and `%s` the login shell, which is what puts the agent CLIs on PATH.
fn default_terminal() -> Vec<String> {
    ["cosmic-term", "-w", "%d", "-e", "%s", "-lc", "%c"]
        .into_iter()
        .map(String::from)
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub terminal: Vec<String>,
    /// Drop into an interactive shell when the agent exits, instead of closing the window.
    pub keep_shell_open: bool,
    /// Directories pinned to the top of the launch target list.
    pub project_dirs: Vec<String>,
    pub pricing: BTreeMap<String, [f64; 2]>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            terminal: default_terminal(),
            keep_shell_open: true,
            project_dirs: Vec::new(),
            pricing: default_pricing(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("cosmic-ext-applet-agents/config.json")
    }

    /// Loads the config, writing out the defaults the first time so there is a file to edit.
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
                tracing::warn!(?err, "invalid config, falling back to defaults");
                Self::default()
            }),
            Err(_) => {
                let config = Self::default();
                config.save();
                config
            }
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(raw) => {
                if let Err(err) = std::fs::write(&path, raw) {
                    tracing::warn!(?err, "could not write config");
                }
            }
            Err(err) => tracing::warn!(?err, "could not serialize config"),
        }
    }

    pub fn price_per_mtok(&self, model: &str) -> (f64, f64) {
        let matched = self
            .pricing
            .iter()
            .filter(|(key, _)| key.as_str() != "default" && model.contains(key.as_str()))
            .max_by_key(|(key, _)| key.len())
            .map(|(_, price)| *price);

        let price = matched
            .or_else(|| self.pricing.get("default").copied())
            .unwrap_or([5.0, 25.0]);
        (price[0], price[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_pricing_key_wins() {
        let cfg = Config::default();
        assert_eq!(cfg.price_per_mtok("claude-sonnet-4-6"), (3.0, 15.0));
        assert_eq!(cfg.price_per_mtok("claude-sonnet-5"), (2.0, 10.0));
        assert_eq!(cfg.price_per_mtok("claude-opus-5"), (5.0, 25.0));
        assert_eq!(cfg.price_per_mtok("claude-haiku-4-5"), (1.0, 5.0));
        // Unknown ids fall back to the default row rather than costing nothing.
        assert_eq!(cfg.price_per_mtok("some-future-model"), (5.0, 25.0));
    }
}
