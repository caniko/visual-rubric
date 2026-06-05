use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rand::Rng as _;
use tempfile::TempDir;

use crate::{
    AcpClient, DEFAULT_CODEX_ACP_MODEL, DEFAULT_CODEX_ACP_REASONING_EFFORT, DEFAULT_SYSTEM_PROMPT,
    PoolError, RateLimitEvent, RubricOptions, RubricVerdict, default_codex_acp_binary,
    default_options, encode_png, parse_verdict,
};

const DEFAULT_SUBMIT_TIMEOUT: Duration = Duration::from_secs(600);
const RECYCLE_SPAWN_ATTEMPTS: u32 = 2;

pub struct RubricPool {
    senders: Vec<mpsc::Sender<Job>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    next: AtomicUsize,
    config: PoolConfig,
    shared: Arc<SharedPoolState>,
}

#[derive(Clone, Debug)]
pub struct PoolConfig {
    pub workers: usize,
    pub max_prompts_per_worker: u32,
    pub max_retries: u32,
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
    pub default_options: RubricOptions,
    pub codex_acp_binary: PathBuf,
    pub extra_env: Vec<(OsString, OsString)>,
    pub submit_timeout: Duration,
    pub source_codex_home: Option<PathBuf>,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            workers: 4,
            max_prompts_per_worker: 50,
            max_retries: 4,
            backoff_base: Duration::from_secs(30),
            backoff_cap: Duration::from_secs(300),
            default_options: default_options(),
            codex_acp_binary: default_codex_acp_binary(),
            extra_env: Vec::new(),
            submit_timeout: DEFAULT_SUBMIT_TIMEOUT,
            source_codex_home: None,
        }
    }
}

struct Job {
    png_path: PathBuf,
    question: String,
    options: RubricOptions,
    reply: mpsc::Sender<Result<RubricVerdict, PoolError>>,
}

#[derive(Clone, Debug, Default)]
pub struct PoolStats {
    pub completed: u64,
    pub failures: u64,
    pub rate_limit_events: Vec<RateLimitEvent>,
    pub worker_recycles: u64,
}

#[derive(Default)]
struct SharedPoolState {
    completed: AtomicU64,
    failures: AtomicU64,
    worker_recycles: AtomicU64,
    rate_limit_events: Mutex<Vec<RateLimitEvent>>,
    fatal_quota: AtomicBool,
    alive_mask: AtomicU64,
}

impl RubricPool {
    pub fn new(config: PoolConfig) -> Result<Self, PoolError> {
        if config.workers == 0 {
            return Err(PoolError::Spawn(
                "workers must be greater than zero".to_string(),
            ));
        }
        if config.workers > u64::BITS as usize {
            return Err(PoolError::Spawn(format!(
                "workers={} exceeds alive bitmask capacity {}",
                config.workers,
                u64::BITS
            )));
        }

        let shared = Arc::new(SharedPoolState::default());
        shared
            .alive_mask
            .store(alive_mask(config.workers), Ordering::Release);

        let mut senders = Vec::with_capacity(config.workers);
        let mut handles = Vec::with_capacity(config.workers);
        for worker_id in 0..config.workers {
            let (job_tx, job_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let worker = Worker {
                id: worker_id,
                config: config.clone(),
                shared: Arc::clone(&shared),
                jobs: job_rx,
            };
            let handle = thread::spawn(move || worker.run(ready_tx));
            match ready_rx.recv() {
                Ok(Ok(())) => {
                    senders.push(job_tx);
                    handles.push(handle);
                }
                Ok(Err(error)) => {
                    shared
                        .alive_mask
                        .fetch_and(!worker_bit(worker_id), Ordering::AcqRel);
                    drop(senders);
                    join_handles(handles);
                    let _ = handle.join();
                    return Err(error);
                }
                Err(error) => {
                    shared
                        .alive_mask
                        .fetch_and(!worker_bit(worker_id), Ordering::AcqRel);
                    drop(senders);
                    join_handles(handles);
                    let _ = handle.join();
                    return Err(PoolError::WorkerCrashed {
                        worker_id,
                        message: format!("worker exited before startup result: {error}"),
                    });
                }
            }
        }

        Ok(Self {
            senders,
            handles: Mutex::new(handles),
            next: AtomicUsize::new(0),
            config,
            shared,
        })
    }

    pub fn submit(
        &self,
        png_path: &Path,
        question: &str,
        opts: RubricOptions,
    ) -> Result<RubricVerdict, PoolError> {
        if self.shared.fatal_quota.load(Ordering::Acquire) {
            return Err(PoolError::QuotaExceeded);
        }

        let worker_id = self.next_live_worker()?;
        let (reply_tx, reply_rx) = mpsc::channel();
        let job = Job {
            png_path: png_path.to_path_buf(),
            question: question.to_string(),
            options: merge_options(opts, &self.config.default_options),
            reply: reply_tx,
        };

        self.senders[worker_id]
            .send(job)
            .map_err(|_| PoolError::WorkerCrashed {
                worker_id,
                message: "worker channel closed".to_string(),
            })?;

        match reply_rx.recv_timeout(self.config.submit_timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(PoolError::Timeout {
                worker_id,
                timeout: self.config.submit_timeout,
            }),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(PoolError::WorkerCrashed {
                worker_id,
                message: "worker dropped reply channel".to_string(),
            }),
        }
    }

    pub fn shutdown(self) -> PoolStats {
        let Self {
            senders,
            handles,
            shared,
            ..
        } = self;
        drop(senders);
        if let Ok(handles) = handles.into_inner() {
            join_handles(handles);
        }
        shared.stats()
    }

    pub fn stats(&self) -> PoolStats {
        self.shared.stats()
    }

    fn next_live_worker(&self) -> Result<usize, PoolError> {
        let worker_count = self.senders.len();
        for _ in 0..worker_count {
            let idx = self.next.fetch_add(1, Ordering::AcqRel) % worker_count;
            let mask = self.shared.alive_mask.load(Ordering::Acquire);
            if mask & worker_bit(idx) != 0 {
                return Ok(idx);
            }
        }
        Err(PoolError::NoLiveWorkers)
    }
}

struct Worker {
    id: usize,
    config: PoolConfig,
    shared: Arc<SharedPoolState>,
    jobs: mpsc::Receiver<Job>,
}

struct WorkerRuntime {
    acp: AcpClient,
    _codex_home: TempDir,
    prompts: u32,
    model: String,
    effort: String,
}

impl Worker {
    fn run(self, ready: mpsc::Sender<Result<(), PoolError>>) {
        let mut runtime = match self.spawn_runtime(&self.config.default_options) {
            Ok(runtime) => {
                let _ = ready.send(Ok(()));
                runtime
            }
            Err(error) => {
                self.mark_dead();
                let _ = ready.send(Err(error));
                return;
            }
        };

        while let Ok(job) = self.jobs.recv() {
            let result = self.handle_job(&mut runtime, &job);
            let fatal_quota = matches!(result, Err(PoolError::QuotaExceeded));
            let _ = job.reply.send(result);
            if fatal_quota {
                self.shared.fatal_quota.store(true, Ordering::Release);
            }
            if self.shared.alive_mask.load(Ordering::Acquire) & worker_bit(self.id) == 0 {
                break;
            }
        }
    }

    fn handle_job(
        &self,
        runtime: &mut WorkerRuntime,
        job: &Job,
    ) -> Result<RubricVerdict, PoolError> {
        let mut last_error = None;
        for attempt in 0..=self.config.max_retries {
            if !runtime.matches_options(&job.options) {
                self.recycle_runtime(runtime, &job.options)?;
            }

            match self.evaluate_once(runtime, job) {
                Ok(verdict) => {
                    self.shared.completed.fetch_add(1, Ordering::AcqRel);
                    runtime.prompts += 1;
                    if runtime.prompts >= self.config.max_prompts_per_worker {
                        self.recycle_runtime(runtime, &job.options)?;
                    }
                    return Ok(verdict);
                }
                Err(PoolError::QuotaExceeded) => {
                    self.shared.failures.fetch_add(1, Ordering::AcqRel);
                    return Err(PoolError::QuotaExceeded);
                }
                Err(PoolError::RateLimited { retry_after }) => {
                    let delay =
                        backoff_delay(attempt, self.config.backoff_base, self.config.backoff_cap);
                    self.shared.push_rate_limit_event(RateLimitEvent {
                        worker_id: self.id,
                        attempt,
                        delay,
                        retry_after,
                    });
                    last_error = Some(PoolError::RateLimited { retry_after });
                    if attempt < self.config.max_retries {
                        thread::sleep(delay);
                    }
                }
                Err(error) => {
                    last_error = Some(error);
                    self.recycle_runtime(runtime, &job.options)?;
                }
            }
        }

        self.shared.failures.fetch_add(1, Ordering::AcqRel);
        Err(last_error.unwrap_or_else(|| PoolError::Rpc("retry loop exhausted".to_string())))
    }

    fn evaluate_once(
        &self,
        runtime: &mut WorkerRuntime,
        job: &Job,
    ) -> Result<RubricVerdict, PoolError> {
        let b64 = encode_png(&job.png_path)?;
        let system_prompt = job
            .options
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SYSTEM_PROMPT);
        let prompt = format!("{system_prompt}\n\nQuestion: {}", job.question);
        let text = runtime.acp.prompt_image(&prompt, &b64)?;
        parse_verdict(&text).map_err(|e| PoolError::ParseVerdict(format!("from {text:?}: {e}")))
    }

    fn recycle_runtime(
        &self,
        runtime: &mut WorkerRuntime,
        options: &RubricOptions,
    ) -> Result<(), PoolError> {
        self.shared.worker_recycles.fetch_add(1, Ordering::AcqRel);
        let mut last_error = None;
        for _ in 0..RECYCLE_SPAWN_ATTEMPTS {
            match self.spawn_runtime(options) {
                Ok(new_runtime) => {
                    *runtime = new_runtime;
                    return Ok(());
                }
                Err(error) => {
                    last_error = Some(error);
                }
            }
        }
        self.mark_dead();
        Err(last_error.unwrap_or_else(|| PoolError::Spawn("recycle failed".to_string())))
    }

    fn spawn_runtime(&self, options: &RubricOptions) -> Result<WorkerRuntime, PoolError> {
        let codex_home =
            TempDir::new().map_err(|e| PoolError::Spawn(format!("create CODEX_HOME: {e}")))?;
        seed_codex_home(codex_home.path(), self.config.source_codex_home.as_deref())?;
        let mut env = self.config.extra_env.clone();
        env.push((
            OsString::from("CODEX_HOME"),
            codex_home.path().as_os_str().to_os_string(),
        ));

        let model = options.model.as_deref().unwrap_or(DEFAULT_CODEX_ACP_MODEL);
        let effort = options
            .effort
            .as_deref()
            .unwrap_or(DEFAULT_CODEX_ACP_REASONING_EFFORT);
        let mut acp = AcpClient::spawn(&self.config.codex_acp_binary, model, effort, &env, None)?;
        acp.start_session(None)?;

        Ok(WorkerRuntime {
            acp,
            _codex_home: codex_home,
            prompts: 0,
            model: model.to_string(),
            effort: effort.to_string(),
        })
    }

    fn mark_dead(&self) {
        self.shared
            .alive_mask
            .fetch_and(!worker_bit(self.id), Ordering::AcqRel);
    }
}

impl WorkerRuntime {
    fn matches_options(&self, options: &RubricOptions) -> bool {
        options.model.as_deref() == Some(self.model.as_str())
            && options.effort.as_deref() == Some(self.effort.as_str())
    }
}

impl SharedPoolState {
    fn stats(&self) -> PoolStats {
        PoolStats {
            completed: self.completed.load(Ordering::Acquire),
            failures: self.failures.load(Ordering::Acquire),
            rate_limit_events: self
                .rate_limit_events
                .lock()
                .map(|events| events.clone())
                .unwrap_or_default(),
            worker_recycles: self.worker_recycles.load(Ordering::Acquire),
        }
    }

    fn push_rate_limit_event(&self, event: RateLimitEvent) {
        if let Ok(mut events) = self.rate_limit_events.lock() {
            events.push(event);
        }
    }
}

fn merge_options(mut opts: RubricOptions, defaults: &RubricOptions) -> RubricOptions {
    if opts.model.is_none() {
        opts.model.clone_from(&defaults.model);
    }
    if opts.effort.is_none() {
        opts.effort.clone_from(&defaults.effort);
    }
    if opts.system_prompt.is_none() {
        opts.system_prompt.clone_from(&defaults.system_prompt);
    }
    opts
}

fn backoff_delay(attempt: u32, base: Duration, cap: Duration) -> Duration {
    let multiplier = 1u32 << attempt.min(6);
    let capped = base.saturating_mul(multiplier).min(cap);
    let jitter_cap = capped.as_millis() as u64 / 4;
    let jitter_ms = rand::rng().random_range(0..=jitter_cap);
    capped + Duration::from_millis(jitter_ms)
}

fn alive_mask(workers: usize) -> u64 {
    if workers == u64::BITS as usize {
        u64::MAX
    } else {
        (1u64 << workers) - 1
    }
}

fn worker_bit(worker_id: usize) -> u64 {
    1u64 << worker_id
}

fn join_handles(handles: Vec<JoinHandle<()>>) {
    for handle in handles {
        let _ = handle.join();
    }
}

fn seed_codex_home(
    worker_home: &Path,
    configured_source_home: Option<&Path>,
) -> Result<(), PoolError> {
    let Some(source_home) = source_codex_home(configured_source_home) else {
        return Ok(());
    };
    for file_name in [
        "auth.json",
        "config.toml",
        "installation_id",
        "models_cache.json",
        "version.json",
    ] {
        let source = source_home.join(file_name);
        if source.exists() {
            let target = worker_home.join(file_name);
            std::fs::copy(&source, &target).map_err(|e| {
                PoolError::Spawn(format!(
                    "seed worker CODEX_HOME file {} from {}: {e}",
                    target.display(),
                    source.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn source_codex_home(configured_source_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured_source_home {
        return Some(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(path));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex"))
}
