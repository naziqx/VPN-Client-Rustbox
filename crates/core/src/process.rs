//! Running a core as a child process.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::config::CoreKind;

const MAX_LOG_LINES: usize = 2000;

/// Shared ring buffer with core output. Cheap to clone.
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<LogInner>>);

#[derive(Default)]
struct LogInner {
    lines: VecDeque<String>,
    /// Normalized form of the last line and how many times it repeated.
    last_key: String,
    repeats: usize,
    /// Incremented on every push; lets UIs redraw only on change.
    generation: u64,
}

impl LogBuffer {
    /// Appends a line. Consecutive lines that differ only in numbers (timestamps,
    /// connection ids) are collapsed into one with a repeat counter.
    pub fn push(&self, line: impl Into<String>) {
        let line = line.into();
        let key: String = line.chars().filter(|c| !c.is_ascii_digit()).collect();
        let mut inner = self.0.lock().unwrap();
        if !inner.lines.is_empty() && key == inner.last_key {
            inner.repeats += 1;
            let text = format!("{line}  (×{})", inner.repeats);
            *inner.lines.back_mut().unwrap() = text;
            inner.generation += 1;
            return;
        }
        inner.last_key = key;
        inner.repeats = 1;
        if inner.lines.len() >= MAX_LOG_LINES {
            inner.lines.pop_front();
        }
        inner.lines.push_back(line);
        inner.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.0.lock().unwrap().generation
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.0.lock().unwrap().lines.iter().cloned().collect()
    }

    pub fn tail(&self, n: usize) -> Vec<String> {
        let inner = self.0.lock().unwrap();
        inner
            .lines
            .iter()
            .skip(inner.lines.len().saturating_sub(n))
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        let mut inner = self.0.lock().unwrap();
        inner.lines.clear();
        inner.last_key.clear();
        inner.generation += 1;
    }
}

pub struct CoreProcess {
    child: Child,
    pub kind: CoreKind,
    pub config_path: PathBuf,
}

impl CoreProcess {
    pub fn spawn(
        kind: CoreKind,
        exe: &Path,
        config: &Path,
        logs: Option<LogBuffer>,
    ) -> anyhow::Result<Self> {
        Self::spawn_with_env(kind, exe, config, logs, &[])
    }

    pub fn spawn_with_env(
        kind: CoreKind,
        exe: &Path,
        config: &Path,
        logs: Option<LogBuffer>,
        env: &[(String, String)],
    ) -> anyhow::Result<Self> {
        let piped = logs.is_some();
        let mut child = core_command(exe)
            .args(kind.run_args(config))
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(config.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null())
            .stdout(if piped { Stdio::piped() } else { Stdio::null() })
            .stderr(if piped { Stdio::piped() } else { Stdio::null() })
            .spawn()
            .with_context(|| format!("failed to start {}", exe.display()))?;
        if let Some(logs) = logs {
            if let Some(out) = child.stdout.take() {
                pump(out, logs.clone());
            }
            if let Some(err) = child.stderr.take() {
                pump(err, logs);
            }
        }
        Ok(Self {
            child,
            kind,
            config_path: config.to_path_buf(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// `None` while running, `Some(status)` once exited.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    pub fn is_running(&mut self) -> bool {
        self.try_wait().is_none()
    }

    /// Graceful stop (SIGTERM, so the core can remove TUN routes), then kill.
    pub fn stop(&mut self) {
        if !self.is_running() {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !self.is_running() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for CoreProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pump(stream: impl Read + Send + 'static, logs: LogBuffer) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            logs.push(strip_ansi(&line));
        }
    });
}

/// Removes ANSI color escape sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Validates a config with the core's own checker.
/// `Command` for a core executable. On Windows the core gets no console
/// window (otherwise every start and every URL-test process flashes one).
fn core_command(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn check_config(
    kind: CoreKind,
    exe: &Path,
    config: &Path,
    env: &[(String, String)],
) -> anyhow::Result<()> {
    let out = core_command(exe)
        .args(kind.check_args(config))
        .envs(env.iter().map(|(k, v)| (k, v)))
        .output()
        .with_context(|| format!("failed to run {}", exe.display()))?;
    if !out.status.success() {
        let mut msg = String::from_utf8_lossy(&out.stderr).to_string();
        msg.push_str(&String::from_utf8_lossy(&out.stdout));
        bail!("{} rejected the config: {}", kind, strip_ansi(msg.trim()));
    }
    Ok(())
}

/// First line of `<core> version`.
pub fn core_version(kind: CoreKind, exe: &Path) -> Option<String> {
    let out = core_command(exe).args(kind.version_args()).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
}
