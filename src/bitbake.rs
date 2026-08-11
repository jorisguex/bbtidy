//! The bounded subprocess boundary shared by all BitBake-backed operations.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const TERMINATION_GRACE: Duration = Duration::from_millis(250);
const EXCERPT_BYTES: usize = 4096;

/// Conservative defaults for BitBake-backed commands. These are intentionally
/// separate from source-file safety limits because BitBake environments and
/// diagnostics have different failure modes.
pub const DEFAULT_BITBAKE_COMMAND_TIMEOUT_SECONDS: u64 = 1_800;
pub const DEFAULT_BITBAKE_TOTAL_TIMEOUT_SECONDS: u64 = 7_200;
pub const DEFAULT_BITBAKE_MAX_STDOUT_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_BITBAKE_MAX_STDERR_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_BITBAKE_MAX_COMMANDS: usize = 20_000;
pub const DEFAULT_BITBAKE_MAX_RECIPE_QUERIES: usize = 10_000;

/// Limits applied to every BitBake-backed command in one operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BitBakeExecutionLimits {
    pub command_timeout: Duration,
    pub total_timeout: Duration,
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
    pub max_commands: usize,
    pub max_recipe_queries: usize,
}

impl Default for BitBakeExecutionLimits {
    fn default() -> Self {
        Self {
            command_timeout: Duration::from_secs(DEFAULT_BITBAKE_COMMAND_TIMEOUT_SECONDS),
            total_timeout: Duration::from_secs(DEFAULT_BITBAKE_TOTAL_TIMEOUT_SECONDS),
            max_stdout_bytes: DEFAULT_BITBAKE_MAX_STDOUT_BYTES,
            max_stderr_bytes: DEFAULT_BITBAKE_MAX_STDERR_BYTES,
            max_commands: DEFAULT_BITBAKE_MAX_COMMANDS,
            max_recipe_queries: DEFAULT_BITBAKE_MAX_RECIPE_QUERIES,
        }
    }
}

impl BitBakeExecutionLimits {
    pub fn validate(self) -> Result<Self, String> {
        if self.command_timeout.is_zero() {
            return Err("bitbake.command_timeout_seconds must be greater than zero".to_owned());
        }
        if self.total_timeout.is_zero() {
            return Err("bitbake.total_timeout_seconds must be greater than zero".to_owned());
        }
        if self.total_timeout < self.command_timeout {
            return Err(
                "bitbake.total_timeout_seconds must be at least command_timeout_seconds".to_owned(),
            );
        }
        if self.max_stdout_bytes == 0 {
            return Err("bitbake.max_stdout_bytes must be greater than zero".to_owned());
        }
        if self.max_stderr_bytes == 0 {
            return Err("bitbake.max_stderr_bytes must be greater than zero".to_owned());
        }
        if self.max_commands == 0 {
            return Err("bitbake.max_commands must be greater than zero".to_owned());
        }
        if self.max_recipe_queries == 0 {
            return Err("bitbake.max_recipe_queries must be greater than zero".to_owned());
        }
        if self.command_timeout.as_millis() > u64::MAX as u128
            || self.total_timeout.as_millis() > u64::MAX as u128
        {
            return Err("bitbake timeout is too large".to_owned());
        }
        if usize::try_from(self.max_stdout_bytes).is_err()
            || usize::try_from(self.max_stderr_bytes).is_err()
        {
            return Err("bitbake output limit is too large for this platform".to_owned());
        }
        Ok(self)
    }
}

/// A cancellation token owned by the CLI or an embedding application.
#[derive(Clone, Debug, Default)]
pub struct BitBakeCancellationToken {
    flag: Arc<AtomicBool>,
    external: Option<&'static AtomicBool>,
}

impl BitBakeCancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn with_external_flag(flag: &'static AtomicBool) -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            external: Some(flag),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
            || self
                .external
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }
}

/// A phase is included in every invocation and operational error.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BitBakePhase {
    Version,
    Parse,
    GlobalEnvironment,
    RecipeEnvironment,
    TargetEnvironment,
    DependencyGraph,
    DryRun,
    RecipeInventory,
}

impl fmt::Display for BitBakePhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Version => "version",
            Self::Parse => "parse",
            Self::GlobalEnvironment => "global environment",
            Self::RecipeEnvironment => "recipe environment",
            Self::TargetEnvironment => "target environment",
            Self::DependencyGraph => "dependency graph",
            Self::DryRun => "dry run",
            Self::RecipeInventory => "recipe inventory",
        })
    }
}

/// A single bounded invocation description.
#[derive(Clone, Debug)]
pub struct BitBakeInvocation {
    pub executable: PathBuf,
    pub current_dir: PathBuf,
    pub arguments: Vec<String>,
    pub phase: BitBakePhase,
    pub target: Option<String>,
    pub recipe: Option<String>,
    pub environment: Vec<(String, String)>,
    pub cacheable: bool,
}

impl BitBakeInvocation {
    pub fn new(
        executable: impl Into<PathBuf>,
        current_dir: impl Into<PathBuf>,
        phase: BitBakePhase,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            executable: executable.into(),
            current_dir: current_dir.into(),
            arguments: arguments.into_iter().map(Into::into).collect(),
            phase,
            target: None,
            recipe: None,
            environment: Vec::new(),
            cacheable: matches!(phase, BitBakePhase::Version | BitBakePhase::Parse),
        }
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    pub fn recipe(mut self, recipe: impl Into<String>) -> Self {
        self.recipe = Some(recipe.into());
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.push((name.into(), value.into()));
        self
    }

    pub fn uncached(mut self) -> Self {
        self.cacheable = false;
        self
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct InvocationKey {
    executable: PathBuf,
    current_dir: PathBuf,
    phase: BitBakePhase,
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
}

impl From<&BitBakeInvocation> for InvocationKey {
    fn from(invocation: &BitBakeInvocation) -> Self {
        Self {
            executable: fs::canonicalize(&invocation.executable)
                .unwrap_or_else(|_| invocation.executable.clone()),
            current_dir: fs::canonicalize(&invocation.current_dir)
                .unwrap_or_else(|_| invocation.current_dir.clone()),
            phase: invocation.phase,
            arguments: invocation.arguments.clone(),
            environment: invocation.environment.clone(),
        }
    }
}

/// The exact bounded output of a successful invocation.
#[derive(Clone, Debug)]
pub struct BitBakeOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

impl BitBakeOutput {
    pub fn stdout_bytes(&self) -> u64 {
        self.stdout.len() as u64
    }

    pub fn stderr_bytes(&self) -> u64 {
        self.stderr.len() as u64
    }
}

/// Deterministic, machine-readable execution counters.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct BitBakeExecutionStats {
    pub limits: BitBakeExecutionLimits,
    pub total_commands: usize,
    pub commands_by_phase: BTreeMap<BitBakePhase, usize>,
    pub elapsed_ms_by_phase: BTreeMap<BitBakePhase, u128>,
    pub stdout_bytes_by_phase: BTreeMap<BitBakePhase, u64>,
    pub stderr_bytes_by_phase: BTreeMap<BitBakePhase, u64>,
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
    pub total_stdout_bytes: u64,
    pub total_stderr_bytes: u64,
    pub recipe_queries_scheduled: usize,
    pub recipe_queries_completed: usize,
    pub cache_hits: usize,
    pub strategy: Option<String>,
}

impl BitBakeExecutionStats {
    pub fn record_cache_hit(&mut self) {
        self.cache_hits = self.cache_hits.saturating_add(1);
    }

    pub fn set_strategy(&mut self, strategy: impl Into<String>) {
        self.strategy = Some(strategy.into());
    }
}

/// Typed operational failure at the BitBake boundary.
#[derive(Debug)]
pub enum BitBakeError {
    Spawn {
        invocation: BitBakeInvocation,
        source: io::Error,
    },
    Wait {
        invocation: BitBakeInvocation,
        source: io::Error,
    },
    Read {
        phase: BitBakePhase,
        stream: &'static str,
        target: Option<String>,
        recipe: Option<String>,
        source: io::Error,
    },
    Cancelled {
        invocation: BitBakeInvocation,
        elapsed: Duration,
    },
    Timeout {
        invocation: BitBakeInvocation,
        elapsed: Duration,
        limit: Duration,
    },
    TotalDeadline {
        invocation: BitBakeInvocation,
        elapsed: Duration,
        limit: Duration,
    },
    StdoutLimit {
        invocation: BitBakeInvocation,
        elapsed: Duration,
        limit: u64,
        captured: u64,
    },
    StderrLimit {
        invocation: BitBakeInvocation,
        elapsed: Duration,
        limit: u64,
        captured: u64,
    },
    CommandBudget {
        phase: BitBakePhase,
        limit: usize,
    },
    RecipeQueryBudget {
        phase: BitBakePhase,
        recipe: Option<String>,
        limit: usize,
    },
    NonZero {
        invocation: BitBakeInvocation,
        output: BitBakeOutput,
        stdout_excerpt: String,
        stderr_excerpt: String,
    },
}

impl BitBakeError {
    pub fn phase(&self) -> BitBakePhase {
        match self {
            Self::Spawn { invocation, .. }
            | Self::Wait { invocation, .. }
            | Self::Cancelled { invocation, .. }
            | Self::Timeout { invocation, .. }
            | Self::TotalDeadline { invocation, .. }
            | Self::StdoutLimit { invocation, .. }
            | Self::StderrLimit { invocation, .. }
            | Self::NonZero { invocation, .. } => invocation.phase,
            Self::Read { phase, .. }
            | Self::CommandBudget { phase, .. }
            | Self::RecipeQueryBudget { phase, .. } => *phase,
        }
    }
}

impl fmt::Display for BitBakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { invocation, source } => write_context(
                formatter,
                invocation,
                &format_args!("could not spawn BitBake: {source}"),
            ),
            Self::Wait { invocation, source } => write_context(
                formatter,
                invocation,
                &format_args!("could not wait for BitBake: {source}"),
            ),
            Self::Read {
                phase,
                stream,
                target,
                recipe,
                source,
            } => write!(
                formatter,
                "BitBake {phase} {stream} read failed{}{}: {source}",
                target
                    .as_deref()
                    .map(|value| format!(" for target {value}"))
                    .unwrap_or_default(),
                recipe
                    .as_deref()
                    .map(|value| format!(" for recipe {value}"))
                    .unwrap_or_default(),
            ),
            Self::Cancelled {
                invocation,
                elapsed,
            } => write_context(
                formatter,
                invocation,
                &format_args!("cancelled after {:.3}s", elapsed.as_secs_f64()),
            ),
            Self::Timeout {
                invocation,
                elapsed,
                limit,
            } => write_context(
                formatter,
                invocation,
                &format_args!(
                    "command timeout after {:.3}s (limit {:.3}s)",
                    elapsed.as_secs_f64(),
                    limit.as_secs_f64()
                ),
            ),
            Self::TotalDeadline {
                invocation,
                elapsed,
                limit,
            } => write_context(
                formatter,
                invocation,
                &format_args!(
                    "total BitBake deadline after {:.3}s (limit {:.3}s)",
                    elapsed.as_secs_f64(),
                    limit.as_secs_f64()
                ),
            ),
            Self::StdoutLimit {
                invocation,
                elapsed,
                limit,
                captured,
            } => write_context(
                formatter,
                invocation,
                &format_args!(
                    "stdout limit exceeded after {:.3}s (captured {captured} bytes, limit {limit})",
                    elapsed.as_secs_f64()
                ),
            ),
            Self::StderrLimit {
                invocation,
                elapsed,
                limit,
                captured,
            } => write_context(
                formatter,
                invocation,
                &format_args!(
                    "stderr limit exceeded after {:.3}s (captured {captured} bytes, limit {limit})",
                    elapsed.as_secs_f64()
                ),
            ),
            Self::CommandBudget { phase, limit } => {
                write!(
                    formatter,
                    "BitBake {phase} command budget exhausted (limit {limit})"
                )
            }
            Self::RecipeQueryBudget {
                phase,
                recipe,
                limit,
            } => write!(
                formatter,
                "BitBake {phase} recipe-query budget exhausted{} (limit {limit})",
                recipe
                    .as_deref()
                    .map(|value| format!(" at {value}"))
                    .unwrap_or_default(),
            ),
            Self::NonZero {
                invocation,
                output,
                stdout_excerpt,
                stderr_excerpt,
            } => write_context(
                formatter,
                invocation,
                &format_args!(
                    "exited with {} after {:.3}s; stdout{}; stderr{}",
                    output.status,
                    output.elapsed.as_secs_f64(),
                    excerpt_suffix(stdout_excerpt),
                    excerpt_suffix(stderr_excerpt),
                ),
            ),
        }
    }
}

impl std::error::Error for BitBakeError {}

/// A sequential runner with operation-wide accounting and cache scope.
pub struct BitBakeRunner {
    limits: BitBakeExecutionLimits,
    started: Instant,
    cancellation: BitBakeCancellationToken,
    cache: BTreeMap<InvocationKey, BitBakeOutput>,
    stats: BitBakeExecutionStats,
}

impl BitBakeRunner {
    pub fn new(limits: BitBakeExecutionLimits) -> Result<Self, String> {
        Self::with_cancellation(limits, BitBakeCancellationToken::new())
    }

    pub fn with_cancellation(
        limits: BitBakeExecutionLimits,
        cancellation: BitBakeCancellationToken,
    ) -> Result<Self, String> {
        let limits = limits.validate()?;
        Ok(Self {
            limits,
            started: Instant::now(),
            cancellation,
            cache: BTreeMap::new(),
            stats: BitBakeExecutionStats {
                limits,
                ..BitBakeExecutionStats::default()
            },
        })
    }

    pub fn limits(&self) -> BitBakeExecutionLimits {
        self.limits
    }

    pub fn cancellation(&self) -> &BitBakeCancellationToken {
        &self.cancellation
    }

    pub fn stats(&self) -> &BitBakeExecutionStats {
        &self.stats
    }

    pub fn stats_mut(&mut self) -> &mut BitBakeExecutionStats {
        &mut self.stats
    }

    #[allow(clippy::result_large_err)]
    pub fn record_recipe_queries(&mut self, count: usize) -> Result<(), BitBakeError> {
        if count
            > self
                .limits
                .max_recipe_queries
                .saturating_sub(self.stats.recipe_queries_scheduled)
        {
            return Err(BitBakeError::RecipeQueryBudget {
                phase: BitBakePhase::RecipeEnvironment,
                recipe: None,
                limit: self.limits.max_recipe_queries,
            });
        }
        self.stats.recipe_queries_scheduled += count;
        self.stats.recipe_queries_completed += count;
        Ok(())
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    #[allow(clippy::result_large_err)]
    pub fn run(&mut self, invocation: BitBakeInvocation) -> Result<BitBakeOutput, BitBakeError> {
        if self.cancellation.is_cancelled() {
            return Err(BitBakeError::Cancelled {
                invocation,
                elapsed: self.elapsed(),
            });
        }
        if self.elapsed() >= self.limits.total_timeout {
            return Err(BitBakeError::TotalDeadline {
                invocation,
                elapsed: self.elapsed(),
                limit: self.limits.total_timeout,
            });
        }
        if self.stats.total_commands >= self.limits.max_commands {
            return Err(BitBakeError::CommandBudget {
                phase: invocation.phase,
                limit: self.limits.max_commands,
            });
        }
        if invocation.recipe.is_some() {
            if self.stats.recipe_queries_scheduled >= self.limits.max_recipe_queries {
                return Err(BitBakeError::RecipeQueryBudget {
                    phase: invocation.phase,
                    recipe: invocation.recipe.clone(),
                    limit: self.limits.max_recipe_queries,
                });
            }
            self.stats.recipe_queries_scheduled += 1;
        }

        let key = InvocationKey::from(&invocation);
        if invocation.cacheable {
            if let Some(output) = self.cache.get(&key) {
                self.stats.record_cache_hit();
                return Ok(output.clone());
            }
        }

        self.stats.total_commands += 1;
        *self
            .stats
            .commands_by_phase
            .entry(invocation.phase)
            .or_default() += 1;
        let command_started = Instant::now();
        let mut command = Command::new(&invocation.executable);
        command
            .current_dir(&invocation.current_dir)
            .args(&invocation.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in &invocation.environment {
            command.env(name, value);
        }
        configure_process_group(&mut command);
        let mut child = command.spawn().map_err(|source| BitBakeError::Spawn {
            invocation: invocation.clone(),
            source,
        })?;
        let stdout = child.stdout.take().expect("piped stdout requested");
        let stderr = child.stderr.take().expect("piped stderr requested");
        let stdout_flag = Arc::new(AtomicBool::new(false));
        let stderr_flag = Arc::new(AtomicBool::new(false));
        let stdout_done = Arc::new(AtomicBool::new(false));
        let stderr_done = Arc::new(AtomicBool::new(false));
        let stdout_reader = spawn_reader(
            stdout,
            self.limits.max_stdout_bytes,
            Arc::clone(&stdout_flag),
            Arc::clone(&stdout_done),
        );
        let stderr_reader = spawn_reader(
            stderr,
            self.limits.max_stderr_bytes,
            Arc::clone(&stderr_flag),
            Arc::clone(&stderr_done),
        );

        let status = loop {
            let status = child.try_wait().map_err(|source| BitBakeError::Wait {
                invocation: invocation.clone(),
                source,
            })?;
            let elapsed = command_started.elapsed();
            if self.cancellation.is_cancelled() {
                terminate_child(&mut child);
                break Err(BitBakeError::Cancelled {
                    invocation: invocation.clone(),
                    elapsed: self.elapsed(),
                });
            }
            if stdout_flag.load(Ordering::SeqCst) {
                terminate_child(&mut child);
                break Err(BitBakeError::StdoutLimit {
                    invocation: invocation.clone(),
                    elapsed,
                    limit: self.limits.max_stdout_bytes,
                    captured: self.limits.max_stdout_bytes,
                });
            }
            if stderr_flag.load(Ordering::SeqCst) {
                terminate_child(&mut child);
                break Err(BitBakeError::StderrLimit {
                    invocation: invocation.clone(),
                    elapsed,
                    limit: self.limits.max_stderr_bytes,
                    captured: self.limits.max_stderr_bytes,
                });
            }
            if let Some(status) = status {
                if stdout_done.load(Ordering::SeqCst) && stderr_done.load(Ordering::SeqCst) {
                    break Ok(status);
                }
            }
            if elapsed >= self.limits.command_timeout {
                terminate_child(&mut child);
                break Err(BitBakeError::Timeout {
                    invocation: invocation.clone(),
                    elapsed,
                    limit: self.limits.command_timeout,
                });
            }
            let total_elapsed = self.elapsed();
            if total_elapsed >= self.limits.total_timeout {
                terminate_child(&mut child);
                break Err(BitBakeError::TotalDeadline {
                    invocation: invocation.clone(),
                    elapsed: total_elapsed,
                    limit: self.limits.total_timeout,
                });
            }
            thread::sleep(POLL_INTERVAL);
        }?;

        let stdout = stdout_reader
            .join()
            .map_err(|_| BitBakeError::Read {
                phase: invocation.phase,
                stream: "stdout",
                target: invocation.target.clone(),
                recipe: invocation.recipe.clone(),
                source: io::Error::other("stdout reader thread panicked"),
            })?
            .map_err(|source| BitBakeError::Read {
                phase: invocation.phase,
                stream: "stdout",
                target: invocation.target.clone(),
                recipe: invocation.recipe.clone(),
                source,
            })?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| BitBakeError::Read {
                phase: invocation.phase,
                stream: "stderr",
                target: invocation.target.clone(),
                recipe: invocation.recipe.clone(),
                source: io::Error::other("stderr reader thread panicked"),
            })?
            .map_err(|source| BitBakeError::Read {
                phase: invocation.phase,
                stream: "stderr",
                target: invocation.target.clone(),
                recipe: invocation.recipe.clone(),
                source,
            })?;
        let elapsed = command_started.elapsed();
        *self
            .stats
            .elapsed_ms_by_phase
            .entry(invocation.phase)
            .or_default() += elapsed.as_millis();
        let stdout_bytes = stdout.len() as u64;
        let stderr_bytes = stderr.len() as u64;
        *self
            .stats
            .stdout_bytes_by_phase
            .entry(invocation.phase)
            .or_default() += stdout_bytes;
        *self
            .stats
            .stderr_bytes_by_phase
            .entry(invocation.phase)
            .or_default() += stderr_bytes;
        self.stats.total_stdout_bytes = self.stats.total_stdout_bytes.saturating_add(stdout_bytes);
        self.stats.total_stderr_bytes = self.stats.total_stderr_bytes.saturating_add(stderr_bytes);
        self.stats.max_stdout_bytes = self.stats.max_stdout_bytes.max(stdout_bytes);
        self.stats.max_stderr_bytes = self.stats.max_stderr_bytes.max(stderr_bytes);
        if invocation.recipe.is_some() {
            self.stats.recipe_queries_completed += 1;
        }
        let output = BitBakeOutput {
            status,
            stdout,
            stderr,
            elapsed,
        };
        if !output.status.success() {
            return Err(BitBakeError::NonZero {
                invocation,
                stdout_excerpt: excerpt(&output.stdout),
                stderr_excerpt: excerpt(&output.stderr),
                output,
            });
        }
        if invocation.cacheable {
            self.cache.insert(key, output.clone());
        }
        Ok(output)
    }
}

fn write_context(
    formatter: &mut fmt::Formatter<'_>,
    invocation: &BitBakeInvocation,
    detail: &fmt::Arguments<'_>,
) -> fmt::Result {
    write!(formatter, "BitBake {}", invocation.phase)?;
    if let Some(target) = invocation.target.as_deref() {
        write!(formatter, " target {target}")?;
    }
    if let Some(recipe) = invocation.recipe.as_deref() {
        write!(formatter, " recipe {recipe}")?;
    }
    write!(
        formatter,
        " ({}): {detail}",
        invocation.executable.display()
    )
}

fn excerpt(bytes: &[u8]) -> String {
    let truncated = bytes.len() > EXCERPT_BYTES;
    let bytes = &bytes[..bytes.len().min(EXCERPT_BYTES)];
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if truncated {
        text.push_str(" …[excerpt truncated]");
    }
    text.trim().to_owned()
}

fn excerpt_suffix(text: &str) -> String {
    if text.is_empty() {
        " empty".to_owned()
    } else {
        format!(" {text:?}")
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    limit: u64,
    overflow: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<Vec<u8>, io::Error>> {
    thread::spawn(move || {
        let result = (|| {
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    return Ok(bytes);
                }
                let remaining = limit.saturating_sub(bytes.len() as u64);
                if count as u64 > remaining {
                    bytes.extend_from_slice(&buffer[..remaining as usize]);
                    overflow.store(true, Ordering::SeqCst);
                    return Ok(bytes);
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
        })();
        done.store(true, Ordering::SeqCst);
        result
    })
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        // The process group is created by configure_process_group. Ignore a
        // race with a child that has already exited, then fall back to the
        // direct-child API and always reap below.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + TERMINATION_GRACE;
        while Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(POLL_INTERVAL);
        }
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    static NEXT_FAKE: AtomicU64 = AtomicU64::new(0);

    fn fake(source: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "bbtidy-runner-{}-{}",
            std::process::id(),
            NEXT_FAKE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::create_dir_all(&directory);
        let path = directory.join("fake");
        fs::write(&path, format!("#!/bin/sh\n{source}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn make_runner(stdout: u64, stderr: u64) -> BitBakeRunner {
        BitBakeRunner::new(BitBakeExecutionLimits {
            command_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(5),
            max_stdout_bytes: stdout,
            max_stderr_bytes: stderr,
            max_commands: 4,
            max_recipe_queries: 2,
        })
        .unwrap()
    }

    fn invocation(path: &Path) -> BitBakeInvocation {
        BitBakeInvocation::new(path, ".", BitBakePhase::Parse, ["--ok"])
    }

    #[test]
    fn captures_stdout_and_stderr_exactly_at_the_limit() {
        let path = fake("printf 'abc'; printf 'de' >&2");
        let mut runner = make_runner(3, 2);
        let output = runner.run(invocation(&path)).unwrap();
        assert_eq!(output.stdout, b"abc");
        assert_eq!(output.stderr, b"de");
    }

    #[test]
    fn drains_large_stdout_and_stderr_concurrently() {
        let path = fake(
            "python3 -c 'import sys; sys.stdout.buffer.write(b\"x\" * 65536); sys.stderr.buffer.write(b\"y\" * 65536)'",
        );
        let mut runner = BitBakeRunner::new(BitBakeExecutionLimits {
            command_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(5),
            max_stdout_bytes: 65536,
            max_stderr_bytes: 65536,
            max_commands: 2,
            max_recipe_queries: 1,
        })
        .unwrap();
        let output = runner.run(invocation(&path)).unwrap();
        assert_eq!(output.stdout.len(), 65536);
        assert_eq!(output.stderr.len(), 65536);
    }

    #[test]
    fn rejects_output_overflow_without_waiting_for_eof() {
        let path = fake("printf 'abcd'");
        let mut runner = make_runner(3, 2);
        let error = runner.run(invocation(&path)).unwrap_err();
        assert!(matches!(error, BitBakeError::StdoutLimit { limit: 3, .. }));
    }

    #[test]
    fn rejects_stderr_overflow_and_preserves_invalid_utf8_bytes() {
        let path = fake("printf '\\377'; printf 'abcd' >&2");
        let mut runner = make_runner(2, 3);
        let error = runner.run(invocation(&path)).unwrap_err();
        assert!(matches!(error, BitBakeError::StderrLimit { limit: 3, .. }));

        let path = fake("printf '\\377'");
        let output = make_runner(2, 2).run(invocation(&path)).unwrap();
        assert_eq!(output.stdout, vec![0xff]);
    }

    #[test]
    fn reports_nonzero_status_with_bounded_excerpts() {
        let path = fake("printf 'failure' >&2; exit 7");
        let mut runner = make_runner(32, 32);
        let error = runner.run(invocation(&path)).unwrap_err();
        match error {
            BitBakeError::NonZero {
                output,
                stderr_excerpt,
                ..
            } => {
                assert_eq!(output.status.code(), Some(7));
                assert_eq!(stderr_excerpt, "failure");
            }
            other => panic!("unexpected runner error: {other}"),
        }
    }

    #[test]
    fn caches_only_identical_version_or_parse_invocations() {
        let path = fake(":");
        let mut runner = make_runner(10, 10);
        runner
            .run(invocation(&path).uncached())
            .expect("uncached parse invocation");
        let cached = BitBakeInvocation::new(&path, ".", BitBakePhase::Version, ["--version"]);
        runner.run(cached.clone()).unwrap();
        runner.run(cached).unwrap();
        assert_eq!(runner.stats().total_commands, 2);
        assert_eq!(runner.stats().cache_hits, 1);
    }

    #[test]
    fn rejects_command_timeout() {
        let path = fake("sleep 2");
        let mut runner = BitBakeRunner::new(BitBakeExecutionLimits {
            command_timeout: Duration::from_millis(30),
            total_timeout: Duration::from_secs(2),
            max_stdout_bytes: 10,
            max_stderr_bytes: 10,
            max_commands: 2,
            max_recipe_queries: 1,
        })
        .unwrap();
        let error = runner.run(invocation(&path).uncached()).unwrap_err();
        assert!(matches!(error, BitBakeError::Timeout { .. }));
    }

    #[test]
    fn enforces_command_budget() {
        let path = fake(":");
        let mut runner = make_runner(10, 10);
        for _ in 0..4 {
            runner.run(invocation(&path).uncached()).unwrap();
        }
        assert!(matches!(
            runner.run(invocation(&path).uncached()),
            Err(BitBakeError::CommandBudget { limit: 4, .. })
        ));
    }

    #[test]
    fn cancellation_reaps_the_child_process_group() {
        let path = fake("sleep 30");
        let cancellation = BitBakeCancellationToken::new();
        let mut runner = BitBakeRunner::with_cancellation(
            BitBakeExecutionLimits {
                command_timeout: Duration::from_secs(10),
                total_timeout: Duration::from_secs(20),
                max_stdout_bytes: 10,
                max_stderr_bytes: 10,
                max_commands: 2,
                max_recipe_queries: 1,
            },
            cancellation.clone(),
        )
        .unwrap();
        let path_clone = path.clone();
        let handle = std::thread::spawn(move || {
            runner.run(BitBakeInvocation::new(
                path_clone,
                ".",
                BitBakePhase::Parse,
                ["--sleep"],
            ))
        });
        std::thread::sleep(Duration::from_millis(30));
        cancellation.cancel();
        assert!(matches!(
            handle.join().unwrap(),
            Err(BitBakeError::Cancelled { .. })
        ));
    }
}
