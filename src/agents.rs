// Agent catalog, install detection, running-process discovery and launching.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::config::Config;

/// One launchable coding agent. `accent` drives the tile colour in the popup.
pub struct Agent {
    pub id: &'static str,
    pub name: &'static str,
    pub bin: &'static str,
    pub args: &'static [&'static str],
    pub accent: [u8; 3],
}

impl Agent {
    /// Short badge shown on the tile.
    pub fn badge(&self) -> String {
        self.name
            .split(|c: char| c.is_whitespace() || c == '-')
            .filter_map(|word| word.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase()
    }
}

pub const CATALOG: &[Agent] = &[
    Agent {
        id: "claude",
        name: "Claude Code",
        bin: "claude",
        args: &[],
        accent: [217, 119, 87],
    },
    Agent {
        id: "codex",
        name: "Codex",
        bin: "codex",
        args: &[],
        accent: [16, 163, 127],
    },
    Agent {
        id: "opencode",
        name: "OpenCode",
        bin: "opencode",
        args: &[],
        accent: [250, 178, 131],
    },
    Agent {
        id: "copilot",
        name: "Copilot",
        bin: "copilot",
        args: &[],
        accent: [139, 92, 246],
    },
    Agent {
        id: "gemini",
        name: "Gemini",
        bin: "gemini",
        args: &[],
        accent: [66, 133, 244],
    },
    Agent {
        id: "grok",
        name: "Grok",
        bin: "grok",
        args: &[],
        accent: [148, 163, 184],
    },
    Agent {
        id: "crush",
        name: "Crush",
        bin: "crush",
        args: &[],
        accent: [236, 72, 153],
    },
    Agent {
        id: "aider",
        name: "Aider",
        bin: "aider",
        args: &[],
        accent: [34, 197, 94],
    },
    Agent {
        id: "cursor",
        name: "Cursor Agent",
        bin: "cursor-agent",
        args: &[],
        accent: [100, 116, 139],
    },
    Agent {
        id: "goose",
        name: "Goose",
        bin: "goose",
        args: &[],
        accent: [234, 179, 8],
    },
];

pub fn by_id(id: &str) -> Option<&'static Agent> {
    CATALOG.iter().find(|agent| agent.id == id)
}

// ---------------------------------------------------------------------------
// Install detection
// ---------------------------------------------------------------------------

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

/// PATH as a login shell sees it. Panel children inherit a minimal environment, so the
/// inherited PATH misses the per-user bin directories the agent CLIs install into.
fn search_path() -> &'static OsString {
    static PATH: OnceLock<OsString> = OnceLock::new();
    PATH.get_or_init(|| {
        let from_shell = Command::new(login_shell())
            .args(["-lc", "printf %s \"$PATH\""])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|path| !path.is_empty());

        match from_shell {
            Some(path) => OsString::from(path),
            None => std::env::var_os("PATH").unwrap_or_default(),
        }
    })
}

fn lookup(bin: &str) -> Option<PathBuf> {
    std::env::split_paths(search_path())
        .map(|dir| dir.join(bin))
        .find(|candidate| is_executable(candidate))
}

/// Ids of the catalog agents present on PATH.
pub fn installed() -> BTreeSet<&'static str> {
    CATALOG
        .iter()
        .filter(|agent| lookup(agent.bin).is_some())
        .map(|agent| agent.id)
        .collect()
}

// ---------------------------------------------------------------------------
// Running processes
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    pub pid: u32,
    pub agent_id: &'static str,
    pub cwd: PathBuf,
    pub uptime: Duration,
}

fn matches_agent(comm: &str, cmdline: &[String]) -> Option<&'static Agent> {
    // Node-based CLIs rewrite their process title, so `comm` is usually enough;
    // argv[0] covers the ones launched through an absolute path.
    let argv0 = cmdline
        .first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str());

    CATALOG
        .iter()
        .find(|agent| comm == agent.bin || argv0 == Some(agent.bin))
}

/// Agent processes owned by the current user, most recently started first.
pub fn running() -> Vec<Running> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut found = Vec::new();

    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = entry.path();

        let Ok(comm) = std::fs::read_to_string(dir.join("comm")) else {
            continue;
        };
        let comm = comm.trim();
        let cmdline: Vec<String> = std::fs::read(dir.join("cmdline"))
            .map(|raw| {
                raw.split(|b| *b == 0)
                    .filter(|part| !part.is_empty())
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect()
            })
            .unwrap_or_default();

        let Some(agent) = matches_agent(comm, &cmdline) else {
            continue;
        };
        // An unreadable cwd means the process belongs to another user.
        let Ok(cwd) = std::fs::read_link(dir.join("cwd")) else {
            continue;
        };
        // The /proc entry is created when the process starts, so its mtime is the start time.
        let uptime = std::fs::metadata(&dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|started| now.duration_since(started).ok())
            .unwrap_or_default();

        found.push(Running {
            pid,
            agent_id: agent.id,
            cwd,
            uptime,
        });
    }

    found.sort_by_key(|proc| proc.uptime);
    found
}

// ---------------------------------------------------------------------------
// Launching
// ---------------------------------------------------------------------------

fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@".contains(c))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// The command line handed to the terminal's shell.
pub fn agent_command(agent: &Agent, cfg: &Config) -> String {
    let mut parts = vec![shell_quote(agent.bin)];
    parts.extend(agent.args.iter().map(|arg| shell_quote(arg)));
    let mut command = parts.join(" ");
    if cfg.keep_shell_open {
        command.push_str("; exec \"$SHELL\" -i");
    }
    command
}

/// Builds the terminal invocation from the configured template. Returns `None` when
/// the template is empty, which would otherwise spawn nothing.
pub fn launch_command(agent: &Agent, dir: &Path, cfg: &Config) -> Option<Command> {
    let command = agent_command(agent, cfg);
    let dir = dir.to_string_lossy().into_owned();

    let mut template = cfg.terminal.iter();
    let program = template.next()?;
    let mut cmd = Command::new(program);
    for arg in template {
        match arg.as_str() {
            "%d" => cmd.arg(&dir),
            "%c" => cmd.arg(&command),
            "%s" => cmd.arg(login_shell()),
            other => cmd.arg(other),
        };
    }
    cmd.current_dir(&dir);
    Some(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_are_initials() {
        assert_eq!(by_id("claude").unwrap().badge(), "CC");
        assert_eq!(by_id("codex").unwrap().badge(), "C");
        assert_eq!(by_id("cursor").unwrap().badge(), "CA");
    }

    #[test]
    fn command_is_quoted_and_templated() {
        let mut cfg = Config::default();
        let agent = by_id("claude").unwrap();

        cfg.keep_shell_open = false;
        assert_eq!(agent_command(agent, &cfg), "claude");
        cfg.keep_shell_open = true;
        assert_eq!(agent_command(agent, &cfg), "claude; exec \"$SHELL\" -i");

        let cmd = launch_command(agent, Path::new("/tmp/a b"), &cfg).unwrap();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(cmd.get_program(), "cosmic-term");
        assert_eq!(args[0], "-w");
        assert_eq!(args[1], "/tmp/a b");
        assert_eq!(args[3], login_shell());
        assert_eq!(args[5], "claude; exec \"$SHELL\" -i");

        cfg.terminal.clear();
        assert!(launch_command(agent, Path::new("/tmp"), &cfg).is_none());
    }

    #[test]
    fn quoting_survives_apostrophes() {
        assert_eq!(shell_quote("plain-arg"), "plain-arg");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("a b"), "'a b'");
    }

    #[test]
    fn process_matching_uses_comm_or_argv0() {
        assert_eq!(
            matches_agent("claude", &["claude".into()]).map(|a| a.id),
            Some("claude")
        );
        assert_eq!(
            matches_agent("node", &["/home/u/.local/bin/opencode".into()]).map(|a| a.id),
            Some("opencode")
        );
        assert!(matches_agent("bash", &["bash".into()]).is_none());
    }
}
