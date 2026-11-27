use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::Context;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;
use tokio::task::JoinHandle;

use crate::audio::Chunk;
use crate::config::MeetingsConfig;
use crate::detector::{Decision, Detector, Group, Moment};
use crate::list::{Jobs, MeetingEntry, list};
use crate::model::{Model, download};
use crate::module::{Job, Module, ModuleState, Transcribe, asset_name};
use crate::page::{self, MeetingPage, PageTick};
use crate::recording::{MEETING_FILE, RecordedMeeting, Recording};
use crate::transcript::TRANSCRIPT_FILE;

pub const ANNOUNCEMENT: &str =
    "🔴 Better Kumo is recording this conversation to transcribe it for my notes.";
const PREROLL_SLACK_MS: i64 = 5_000;
const TIDY_EVERY: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Activity {
    #[default]
    Off,
    NeedsModule,
    Unavailable,
    NotInCall,
    Listening,
    Waiting {
        people: Vec<String>,
        seconds_left: u64,
    },
    Recording {
        since_ms: i64,
        people: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct MeetingsStatus {
    pub activity: Activity,
    pub module: ModuleState,
    pub transcribing: usize,
    pub model_download: Option<u8>,
    pub last_transcript: Option<PathBuf>,
    pub problem: Option<String>,
}

impl MeetingsStatus {
    pub fn summary(&self, now_ms: i64) -> String {
        let base = match (&self.activity, &self.problem) {
            (Activity::Recording { since_ms, people }, _) => {
                let secs = ((now_ms - since_ms).max(0) / 1_000) as u64;
                format!("recording {}:{:02}{}", secs / 60, secs % 60, with(people))
            }
            (Activity::Off, _) => "start a recording by hand".into(),
            (Activity::NeedsModule, _) => match &self.module {
                ModuleState::Installing { percent, .. } => format!("installing {percent}%"),
                _ => "install the module in Settings".into(),
            },
            (_, Some(problem)) => {
                let short: String = problem.chars().take(50).collect();
                format!("problem: {short}")
            }
            (Activity::Unavailable, _) => "waiting for Kumospace".into(),
            (Activity::NotInCall, _) => "nobody nearby".into(),
            (Activity::Listening, _) => "listening".into(),
            (
                Activity::Waiting {
                    people,
                    seconds_left,
                },
                _,
            ) => format!("starting in {seconds_left} s{}", with(people)),
        };
        let mut line = format!("Meetings: {base}");
        if let Some(percent) = self.model_download {
            line.push_str(&format!(" · speech model {percent}%"));
        }
        if self.transcribing > 0 {
            line.push_str(&format!(" · transcribing {}", self.transcribing));
        }
        line
    }
}

fn with(people: &[String]) -> String {
    let named: Vec<&str> = people
        .iter()
        .map(String::as_str)
        .filter(|n| !n.is_empty())
        .collect();
    match named.len() {
        0 => String::new(),
        1..=3 => format!(" · {}", named.join(", ")),
        n => format!(" · {} +{}", named[..2].join(", "), n - 2),
    }
}

pub struct MeetingsDeps {
    pub page: Arc<dyn MeetingPage>,
    pub config: MeetingsConfig,
    pub models_dir: PathBuf,
    pub module: Module,
    pub transcriber: Arc<dyn Transcribe>,
}

pub struct Meetings {
    page: Arc<dyn MeetingPage>,
    models_dir: PathBuf,
    module: Module,
    transcriber: Arc<dyn Transcribe>,
    installing: tokio::sync::Mutex<()>,
    runtime: tokio::runtime::Handle,
    inner: Mutex<Inner>,
    jobs: Arc<tokio::sync::Mutex<()>>,
    /// One tidy pass at a time: saves and hourly ticks while one runs add nothing.
    tidying: std::sync::atomic::AtomicBool,
    stopped: std::sync::atomic::AtomicBool,
    model_lock: tokio::sync::Mutex<()>,
    task: Mutex<Option<JoinHandle<()>>>,
}

struct Inner {
    config: MeetingsConfig,
    detector: Detector,
    recording: Option<Recording>,
    preroll: VecDeque<Chunk>,
    capturing: bool,
    model_requested: bool,
    queued: HashSet<PathBuf>,
    transcribing: Option<PathBuf>,
    failed: HashMap<PathBuf, String>,
    earshot: Group,
    people: Vec<String>,
    zone: String,
    status: MeetingsStatus,
}

enum Event {
    Announce,
    PrepareModel,
    Transcribe(RecordedMeeting),
}

impl Meetings {
    pub fn new(deps: MeetingsDeps) -> Arc<Self> {
        Arc::new(Self {
            page: deps.page,
            models_dir: deps.models_dir,
            installing: tokio::sync::Mutex::new(()),
            runtime: tokio::runtime::Handle::current(),
            inner: Mutex::new(Inner {
                detector: Detector::new(deps.config.rules()),
                config: deps.config,
                recording: None,
                preroll: VecDeque::new(),
                capturing: false,
                model_requested: false,
                queued: HashSet::new(),
                transcribing: None,
                failed: HashMap::new(),
                earshot: Group::new(),
                people: Vec::new(),
                zone: String::new(),
                status: MeetingsStatus {
                    module: initial_module_state(&deps.module),
                    ..MeetingsStatus::default()
                },
            }),
            module: deps.module,
            transcriber: deps.transcriber,
            jobs: Arc::new(tokio::sync::Mutex::new(())),
            tidying: Default::default(),
            stopped: Default::default(),
            model_lock: tokio::sync::Mutex::new(()),
            task: Mutex::new(None),
        })
    }

    pub fn status(&self) -> MeetingsStatus {
        self.inner().status.clone()
    }

    pub fn meetings(&self) -> anyhow::Result<Vec<MeetingEntry>> {
        let (folder, jobs) = {
            let inner = self.inner();
            (
                inner.config.folder(),
                Jobs {
                    recording: inner.recording.as_ref().map(|r| r.dir().to_path_buf()),
                    transcribing: inner.transcribing.clone(),
                    queued: inner.queued.clone(),
                    failed: inner.failed.clone(),
                },
            )
        };
        list(&folder, &jobs)
    }

    pub fn transcribe_pending(self: &Arc<Self>) -> anyhow::Result<()> {
        if !self.module_ready() {
            anyhow::bail!("install the meetings module first");
        }
        self.recover();
        Ok(())
    }

    pub fn audio_usage(&self) -> crate::storage::AudioUsage {
        crate::storage::audio_usage(&self.folder())
    }

    /// The folder is the one shown when the person confirmed, even if it changes while a
    /// transcription holds `jobs`. The guard moves into the blocking work, so a dropped request
    /// cannot let a tidy pass start while files are still being deleted.
    pub async fn delete_audio(&self) -> anyhow::Result<usize> {
        let folder = self.folder();
        let job = self.jobs.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _job = job;
            crate::storage::delete_audio(&folder)
        })
        .await?
    }

    /// Takes `jobs` for one meeting at a time, so a transcription or a delete waits for at most
    /// one compression, and a transcription finishing meanwhile cannot settle the same meeting
    /// twice. Settings are read again for each meeting.
    pub async fn tidy(self: Arc<Self>) {
        use std::sync::atomic::Ordering;
        if self.tidying.swap(true, Ordering::AcqRel) {
            return;
        }
        let folder = self.folder();
        let scan = folder.clone();
        let dirs = tokio::task::spawn_blocking(move || crate::storage::settled_meetings(&scan))
            .await
            .unwrap_or_default();
        for dir in dirs {
            let job = self.jobs.clone().lock_owned().await;
            if self.stopped.load(Ordering::Acquire) {
                break;
            }
            let (me, folder) = (self.clone(), folder.clone());
            let more = tokio::task::spawn_blocking(move || {
                let _job = job;
                let now_ms = Timestamp::now().as_millisecond();
                crate::storage::tidy_one(&folder, &dir, &me.config(), now_ms)
            })
            .await
            .unwrap_or(false);
            if !more {
                break;
            }
        }
        self.tidying.store(false, Ordering::Release);
    }

    pub fn tidy_soon(self: &Arc<Self>) {
        let me = self.clone();
        self.runtime.spawn(me.tidy());
    }

    pub fn config(&self) -> MeetingsConfig {
        self.inner().config.clone()
    }

    pub fn set_config(&self, config: MeetingsConfig) {
        let mut inner = self.inner();
        inner.detector.set_rules(config.rules());
        if config.model != inner.config.model {
            inner.model_requested = false;
        }
        inner.config = config;
    }

    pub fn folder(&self) -> PathBuf {
        self.inner().config.folder()
    }

    pub fn module_ready(&self) -> bool {
        self.inner().status.module.is_ready()
    }

    pub fn install_module(self: &Arc<Self>) {
        let me = self.clone();
        self.runtime.spawn(async move {
            let Ok(_installing) = me.installing.try_lock() else {
                return;
            };
            let result = me.install().await;
            let installed = result.is_ok();
            let state = match result {
                Ok(version) => ModuleState::Ready { version },
                Err(e) => {
                    eprintln!("meetings: installing the module failed: {e:#}");
                    ModuleState::Failed {
                        error: format!("{e:#}"),
                    }
                }
            };
            me.inner().status.module = state;
            if installed {
                me.recover();
            }
        });
    }

    async fn install(self: &Arc<Self>) -> anyhow::Result<String> {
        self.set_installing("Waiting for the current transcription", 0);
        let _jobs = self.jobs.lock().await;
        self.set_installing("Downloading the transcriber", 0);
        self.module
            .install(|percent| self.set_installing("Downloading the transcriber", percent))
            .await?;
        let version = self.module.installed_version().await?;
        self.set_installing("Downloading the speech model", 0);
        let model = Model::named(&self.config().model)?;
        let _guard = self.model_lock.lock().await;
        download(&model, &self.models_dir, |percent| {
            self.set_installing("Downloading the speech model", percent)
        })
        .await?;
        Ok(version)
    }

    pub async fn remove_module(&self) -> anyhow::Result<()> {
        let _installing = self
            .installing
            .try_lock()
            .ok()
            .context("the meetings module is being installed")?;
        let _jobs = self
            .jobs
            .try_lock()
            .ok()
            .filter(|_| self.inner().status.transcribing == 0)
            .context("a meeting is being transcribed, remove the module when it is done")?;
        let _model = self.model_lock.lock().await;
        self.module.remove().await?;
        if self.models_dir.exists() {
            tokio::fs::remove_dir_all(&self.models_dir).await?;
        }
        self.inner().status.module = initial_module_state(&self.module);
        Ok(())
    }

    fn set_installing(&self, step: &str, percent: u8) {
        self.inner().status.module = ModuleState::Installing {
            step: step.into(),
            percent,
        };
    }

    async fn check_module(&self) {
        if self.module.binary().is_none() {
            return;
        }
        let state = match self.module.installed_version().await {
            Ok(version) => ModuleState::Ready { version },
            Err(e) => ModuleState::Failed {
                error: format!("{e:#}"),
            },
        };
        self.inner().status.module = state;
    }

    pub fn spawn(self: &Arc<Self>) {
        self.stopped
            .store(false, std::sync::atomic::Ordering::Release);
        self.recover();
        let me = self.clone();
        let task = self.runtime.spawn(async move {
            me.check_module().await;
            let mut every = tokio::time::interval(Duration::from_secs(1));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut tidied: Option<tokio::time::Instant> = None;
            loop {
                every.tick().await;
                me.step(Timestamp::now().as_millisecond()).await;
                if tidied.is_none_or(|t| t.elapsed() >= TIDY_EVERY) {
                    tidied = Some(tokio::time::Instant::now());
                    let me = me.clone();
                    me.runtime.clone().spawn(me.tidy());
                }
            }
        });
        if let Ok(mut slot) = self.task.lock() {
            *slot = Some(task);
        }
    }

    pub fn stop(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut slot) = self.task.lock()
            && let Some(task) = slot.take()
        {
            task.abort();
        }
        let mut inner = self.inner();
        for event in inner.end() {
            if let Event::Transcribe(recorded) = event {
                inner.release(&recorded.dir);
            }
        }
    }

    pub async fn step(self: &Arc<Self>, now_ms: i64) {
        let (auto_start, capturing, ready, recording) = {
            let inner = self.inner();
            (
                inner.config.auto_start,
                inner.capturing,
                inner.status.module.is_ready(),
                inner.detector.recording(),
            )
        };
        if !ready || !(auto_start || recording) {
            if capturing {
                let _ = page::read(self.page.as_ref(), false).await;
            }
            let events = {
                let mut inner = self.inner();
                inner.capturing = false;
                inner.preroll.clear();
                inner.status.activity = if !ready {
                    Activity::NeedsModule
                } else {
                    Activity::Off
                };
                inner.end()
            };
            self.handle(events);
            return;
        }
        let tick = page::read(self.page.as_ref(), true).await;
        let events = self.inner().apply(tick, now_ms);
        self.handle(events);
    }

    pub fn recording(&self) -> bool {
        self.inner().detector.recording()
    }

    pub fn start_now(self: &Arc<Self>) -> anyhow::Result<()> {
        let events = {
            let mut inner = self.inner();
            if !inner.status.module.is_ready() {
                anyhow::bail!("install the meetings module first");
            }
            let now_ms = Timestamp::now().as_millisecond();
            let Decision::Start { from_ms } = inner.detector.start_now(now_ms) else {
                return Ok(());
            };
            let zone = inner.zone.clone();
            if let Err(e) = inner.begin(from_ms, &zone) {
                inner.detector.end();
                return Err(e);
            }
            inner.status.activity = Activity::Recording {
                since_ms: from_ms,
                people: inner.people.clone(),
            };
            vec![Event::Announce]
        };
        self.handle(events);
        Ok(())
    }

    pub fn stop_now(self: &Arc<Self>) {
        let events: Vec<Event> = {
            let mut inner = self.inner();
            let earshot = inner.earshot.clone();
            match inner.detector.stop_now(&earshot) {
                Decision::Stop { keep, .. } => {
                    inner.status.activity = Activity::Listening;
                    inner.finish(keep).into_iter().collect()
                }
                _ => Vec::new(),
            }
        };
        self.handle(events);
    }

    pub async fn model(self: &Arc<Self>) -> anyhow::Result<PathBuf> {
        let _guard = self.model_lock.lock().await;
        let model = Model::named(&self.config().model)?;
        let path = model.path_in(&self.models_dir);
        if path.exists() {
            return Ok(path);
        }
        eprintln!(
            "meetings: downloading the speech model {}",
            model.file_name()
        );
        let result = download(&model, &self.models_dir, |percent| {
            self.inner().status.model_download = Some(percent);
        })
        .await;
        self.inner().status.model_download = None;
        result
    }

    fn handle(self: &Arc<Self>, events: Vec<Event>) {
        for event in events {
            match event {
                Event::Announce => {
                    if !self.config().announce {
                        continue;
                    }
                    let page = self.page.clone();
                    self.runtime.spawn(async move {
                        if let Err(e) = page.announce(ANNOUNCEMENT).await {
                            eprintln!("meetings: could not post to Nearby Chat: {e:#}");
                        }
                    });
                }
                Event::PrepareModel => {
                    let me = self.clone();
                    self.runtime.spawn(async move {
                        if let Err(e) = me.model().await {
                            me.inner().status.problem = Some(format!("speech model: {e:#}"));
                        }
                    });
                }
                Event::Transcribe(recorded) => self.run(recorded),
            }
        }
    }

    fn run(self: &Arc<Self>, recorded: RecordedMeeting) {
        let me = self.clone();
        self.runtime.spawn(async move {
            let dir = recorded.dir.clone();
            let mut reservation = Reservation {
                meetings: me.clone(),
                dir: Some(dir.clone()),
            };
            let result = me.transcribe(recorded).await;
            reservation.dir = None;
            let mut inner = me.inner();
            inner.release(&dir);
            match result {
                Ok(path) => {
                    eprintln!("meetings: transcript saved to {}", path.display());
                    inner.failed.remove(&dir);
                    inner.status.last_transcript = Some(path);
                    inner.status.problem = None;
                }
                Err(e) => {
                    eprintln!("meetings: transcribing {} failed: {e:#}", dir.display());
                    inner.failed.insert(dir, format!("{e:#}"));
                    inner.status.problem = Some(format!("transcription failed: {e:#}"));
                }
            }
        });
    }

    async fn transcribe(self: &Arc<Self>, recorded: RecordedMeeting) -> anyhow::Result<PathBuf> {
        let _job = self.jobs.lock().await;
        self.inner().transcribing = Some(recorded.dir.clone());
        let binary = self
            .module
            .binary()
            .context("install the meetings module in Settings")?;
        let model = self.model().await?;
        let config = self.config();
        self.transcriber
            .transcribe(Job {
                binary: &binary,
                model: &model,
                language: &config.language,
                meeting: &recorded.dir,
            })
            .await?;
        let path = recorded.dir.join(TRANSCRIPT_FILE);
        if !path.exists() {
            anyhow::bail!("the transcriber wrote no transcript");
        }
        // The settings now, not when the transcription began: Keep audio may have been turned on.
        let current = self.config();
        let now_ms = Timestamp::now().as_millisecond();
        tokio::task::spawn_blocking(move || crate::storage::settle(&recorded, &current, now_ms))
            .await?;
        Ok(path)
    }

    fn recover(self: &Arc<Self>) {
        let candidates = untranscribed(&self.folder());
        let (empty, pending) = self.inner().claim(candidates);
        for dir in empty {
            let _ = std::fs::remove_dir_all(dir);
        }
        for recorded in pending {
            eprintln!(
                "meetings: transcribing {}, it has no transcript yet",
                recorded.dir.display()
            );
            self.run(recorded);
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Inner {
    fn release(&mut self, dir: &Path) {
        if self.transcribing.as_deref() == Some(dir) {
            self.transcribing = None;
        }
        if self.queued.remove(dir) {
            self.status.transcribing = self.status.transcribing.saturating_sub(1);
        }
    }

    fn reserve(&mut self, dir: &Path) -> bool {
        if !self.queued.insert(dir.to_path_buf()) {
            return false;
        }
        self.failed.remove(dir);
        self.status.transcribing += 1;
        true
    }

    fn claim(&mut self, candidates: Vec<RecordedMeeting>) -> (Vec<PathBuf>, Vec<RecordedMeeting>) {
        let live = self.recording.as_ref().map(|r| r.dir().to_path_buf());
        let mut empty = Vec::new();
        let mut pending = Vec::new();
        for recorded in candidates {
            let dir = recorded.dir.clone();
            if Some(&dir) == live.as_ref() || self.queued.contains(&dir) || !dir.exists() {
                continue;
            }
            let still_empty = recorded.meeting.tracks.is_empty()
                && RecordedMeeting::load(&dir).is_ok_and(|now| now.meeting.tracks.is_empty());
            if still_empty {
                empty.push(dir);
            } else if self.reserve(&dir) {
                pending.push(recorded);
            }
        }
        (empty, pending)
    }

    fn apply(&mut self, tick: anyhow::Result<PageTick>, now_ms: i64) -> Vec<Event> {
        let mut events = Vec::new();
        if !self.model_requested {
            self.model_requested = true;
            events.push(Event::PrepareModel);
        }
        self.capturing = true;
        let (tick, reachable) = match tick {
            Ok(tick) => (tick, true),
            Err(_) => (PageTick::default(), false),
        };
        if let Some(error) = &tick.error {
            self.status.problem = Some(format!("Kumospace audio: {error}"));
        }
        let speech_ms = speech_of(&tick.chunks);
        self.earshot = tick.earshot();
        self.people = tick.people.iter().map(|p| p.name.clone()).collect();
        self.zone = tick.zone.clone();
        match self.recording.as_mut() {
            Some(recording) => {
                for chunk in &tick.chunks {
                    if let Err(e) = recording.push(chunk) {
                        self.status.problem = Some(format!("recording: {e:#}"));
                    }
                }
                recording.note_zone(&tick.zone);
                if let Err(e) = recording.checkpoint() {
                    self.status.problem = Some(format!("recording: {e:#}"));
                }
            }
            None => {
                self.preroll.extend(tick.chunks.iter().cloned());
                let keep_from =
                    now_ms - self.detector.rules().settle.as_millis() as i64 - PREROLL_SLACK_MS;
                while self.preroll.front().is_some_and(|c| c.end_ms() < keep_from) {
                    self.preroll.pop_front();
                }
            }
        }
        let decision = self.detector.tick(Moment {
            now_ms,
            earshot: &tick.earshot(),
            speech_ms,
        });
        match decision {
            Decision::Start { from_ms } => match self.begin(from_ms, &tick.zone) {
                Ok(()) => events.push(Event::Announce),
                Err(e) => self.status.problem = Some(format!("cannot start recording: {e:#}")),
            },
            Decision::Stop { keep, reason } => {
                eprintln!("meetings: recording stopped ({reason:?}), keep={keep}");
                events.extend(self.finish(keep));
            }
            Decision::Wait => {}
        }
        self.status.activity = self.activity(&tick, reachable, now_ms);
        events
    }

    fn begin(&mut self, from_ms: i64, zone: &str) -> anyhow::Result<()> {
        let folder = self.config.folder();
        let dir = unique_dir(&folder, &folder_name(from_ms, zone));
        let mut recording = Recording::create(dir, from_ms)?;
        recording.note_zone(zone);
        for chunk in self.preroll.drain(..) {
            recording.push(&chunk)?;
        }
        recording.checkpoint()?;
        eprintln!("meetings: recording into {}", recording.dir().display());
        self.recording = Some(recording);
        Ok(())
    }

    fn finish(&mut self, keep: bool) -> Option<Event> {
        let recording = self.recording.take()?;
        let dir = recording.dir().to_path_buf();
        match recording.finish() {
            Ok(recorded) if keep && !recorded.meeting.tracks.is_empty() => {
                self.reserve(&recorded.dir);
                Some(Event::Transcribe(recorded))
            }
            Ok(_) => {
                let _ = std::fs::remove_dir_all(&dir);
                None
            }
            Err(e) => {
                self.status.problem = Some(format!("recording: {e:#}"));
                None
            }
        }
    }

    fn end(&mut self) -> Vec<Event> {
        match self.detector.end() {
            Decision::Stop { keep, .. } => self.finish(keep).into_iter().collect(),
            _ => self.finish(false).into_iter().collect(),
        }
    }

    fn activity(&self, tick: &PageTick, reachable: bool, now_ms: i64) -> Activity {
        let people: Vec<String> = tick.people.iter().map(|p| p.name.clone()).collect();
        if let Some(recording) = &self.recording {
            return Activity::Recording {
                since_ms: recording.started_ms(),
                people,
            };
        }
        if !reachable {
            return Activity::Unavailable;
        }
        if !tick.in_call {
            return Activity::NotInCall;
        }
        match self.detector.settle_left_ms(now_ms) {
            Some(left) => Activity::Waiting {
                people,
                seconds_left: (left as u64).div_ceil(1_000),
            },
            None => Activity::Listening,
        }
    }
}

struct Reservation {
    meetings: Arc<Meetings>,
    dir: Option<PathBuf>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            self.meetings.inner().release(dir);
        }
    }
}

fn untranscribed(folder: &Path) -> Vec<RecordedMeeting> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| dir.join(MEETING_FILE).exists() && !dir.join(TRANSCRIPT_FILE).exists())
        .filter_map(|dir| RecordedMeeting::load(&dir).ok())
        .collect()
}

fn initial_module_state(module: &Module) -> ModuleState {
    if module.binary().is_some() {
        ModuleState::Ready {
            version: module.version.clone(),
        }
    } else if module.override_path.is_none() && asset_name(&module.version).is_none() {
        ModuleState::Unsupported
    } else {
        ModuleState::Missing
    }
}

fn speech_of(chunks: &[Chunk]) -> i64 {
    let mut by_speaker: BTreeMap<&str, i64> = BTreeMap::new();
    for chunk in chunks {
        *by_speaker.entry(chunk.speaker.id.as_str()).or_default() += chunk.speech_ms();
    }
    by_speaker.values().copied().max().unwrap_or(0)
}

fn folder_name(started_ms: i64, zone: &str) -> String {
    let when = Timestamp::from_millisecond(started_ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(TimeZone::system())
        .strftime("%Y-%m-%d %H-%M")
        .to_string();
    let place: String = zone
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .take(60)
        .collect();
    let place = place.split_whitespace().collect::<Vec<_>>().join(" ");
    if place.is_empty() {
        when
    } else {
        format!("{when} {place}")
    }
}

fn unique_dir(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| folder.join(format!("{name} ({n})")))
        .find(|dir| !dir.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::tests::tone_i16;
    use crate::list::MeetingState;
    use crate::recording::AUDIO_DIR;
    use crate::transcript::{Segment, SpeechToText};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use futures_util::future::BoxFuture;
    use serde_json::{Value, json};

    #[derive(Default)]
    struct FakePage {
        ticks: Mutex<VecDeque<Value>>,
        captures: Mutex<Vec<bool>>,
        announced: Mutex<Vec<String>>,
    }

    impl MeetingPage for FakePage {
        fn evaluate<'a>(&'a self, expression: &'a str) -> BoxFuture<'a, anyhow::Result<Value>> {
            Box::pin(async move {
                self.captures
                    .lock()
                    .unwrap()
                    .push(expression.contains("tick(true)"));
                Ok(self
                    .ticks
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| json!({ "inCall": false, "me": null })))
            })
        }

        fn announce<'a>(&'a self, text: &'a str) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move {
                self.announced.lock().unwrap().push(text.to_string());
                Ok(())
            })
        }
    }

    struct Echo;

    struct InProcess;

    #[derive(Default)]
    struct FailsFirst {
        failed: std::sync::atomic::AtomicBool,
    }

    impl Transcribe for FailsFirst {
        fn transcribe<'a>(&'a self, job: Job<'a>) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move {
                if !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    anyhow::bail!("whisper crashed");
                }
                InProcess.transcribe(job).await
            })
        }
    }

    impl Transcribe for InProcess {
        fn transcribe<'a>(&'a self, job: Job<'a>) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move {
                let recorded = RecordedMeeting::load(job.meeting)?;
                let transcript = crate::transcript::transcribe(&recorded, &mut Echo)?;
                std::fs::write(
                    job.meeting.join(TRANSCRIPT_FILE),
                    crate::transcript::render(&transcript, &TimeZone::UTC),
                )?;
                Ok(())
            })
        }
    }

    impl SpeechToText for Echo {
        fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Segment>> {
            Ok(vec![Segment {
                start_ms: 0,
                end_ms: crate::audio::samples_to_ms(samples.len()),
                text: "hello".into(),
            }])
        }
    }

    struct Rig {
        page: Arc<FakePage>,
        meetings: Arc<Meetings>,
        folder: PathBuf,
        now_ms: i64,
        _dir: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Self {
            Self::with(Arc::new(InProcess))
        }

        fn with(engine: Arc<dyn Transcribe>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let models = dir.path().join("models");
            std::fs::create_dir_all(&models).unwrap();
            std::fs::write(models.join("ggml-test.bin"), b"").unwrap();
            let folder = dir.path().join("Meetings");
            let transcriber = dir.path().join("better-kumo-transcriber");
            std::fs::write(&transcriber, b"").unwrap();
            let page = Arc::new(FakePage::default());
            let meetings = Meetings::new(MeetingsDeps {
                page: page.clone(),
                config: MeetingsConfig {
                    auto_start: true,
                    settle_seconds: 2,
                    alone_stop_seconds: 2,
                    min_speech_seconds: 3,
                    model: "test".into(),
                    folder: Some(folder.clone()),
                    ..MeetingsConfig::default()
                },
                models_dir: models,
                module: Module::new(dir.path().join("module"), Some(transcriber)),
                transcriber: engine,
            });
            Self {
                page,
                meetings,
                folder,
                now_ms: 1_700_000_000_000,
                _dir: dir,
            }
        }

        async fn second(&mut self, people: &[(&str, &str)], talking: bool) {
            self.now_ms += 1_000;
            let chunks: Vec<Value> = people
                .iter()
                .map(|(id, name)| {
                    let level = if talking { 0.3 } else { 0.0 };
                    let pcm: Vec<u8> = tone_i16(1_000, level)
                        .into_iter()
                        .flat_map(|s| s.to_le_bytes())
                        .collect();
                    json!({ "id": id, "name": name, "me": false, "at": self.now_ms - 1_000, "pcm": STANDARD.encode(pcm) })
                })
                .collect();
            let people: Vec<Value> = people
                .iter()
                .map(|(id, name)| json!({ "id": id, "name": name }))
                .collect();
            let in_call = !people.is_empty();
            self.page.ticks.lock().unwrap().push_back(json!({
                "inCall": in_call, "me": { "id": "u-me", "name": "Igor" }, "people": people,
                "zone": "Kitchen", "capturing": 1, "error": null, "chunks": chunks
            }));
            self.meetings.step(self.now_ms).await;
        }

        fn meeting_dirs(&self) -> Vec<PathBuf> {
            std::fs::read_dir(&self.folder)
                .map(|entries| entries.flatten().map(|e| e.path()).collect())
                .unwrap_or_default()
        }

        async fn settled(&self, state: &MeetingState) -> Vec<MeetingEntry> {
            for _ in 0..200 {
                let list = self.meetings.meetings().unwrap();
                if list.first().is_some_and(|m| &m.state == state) {
                    return list;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("never {state:?}: {:?}", self.meetings.meetings());
        }

        async fn transcribed(&self) -> PathBuf {
            for _ in 0..200 {
                let status = self.meetings.status();
                if status.transcribing == 0
                    && let Some(path) = status.last_transcript
                {
                    return path;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("no transcript: {:?}", self.meetings.status());
        }
    }

    const ANNA: (&str, &str) = ("u-anna", "Anna");

    #[tokio::test(flavor = "multi_thread")]
    async fn records_a_settled_conversation_and_transcribes_it() {
        let mut rig = Rig::new();
        rig.second(&[ANNA], true).await;
        assert!(matches!(
            rig.meetings.status().activity,
            Activity::Waiting {
                seconds_left: 2,
                ..
            }
        ));
        rig.second(&[ANNA], true).await;
        rig.second(&[ANNA], true).await;
        assert!(matches!(
            rig.meetings.status().activity,
            Activity::Recording { .. }
        ));
        for _ in 0..4 {
            rig.second(&[ANNA], true).await;
        }
        rig.second(&[], false).await;
        rig.second(&[], false).await;
        assert_eq!(rig.meetings.status().activity, Activity::NotInCall);

        let transcript = rig.transcribed().await;
        let text = std::fs::read_to_string(&transcript).unwrap();
        assert!(text.contains("· Kitchen"), "{text}");
        assert!(
            text.contains("**[00:00:00] Anna:** hello"),
            "the first words, from before the settle time, are kept: {text}"
        );
        assert!(
            !transcript.parent().unwrap().join(AUDIO_DIR).exists(),
            "audio is deleted after transcription"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            rig.page.announced.lock().unwrap().as_slice(),
            [ANNOUNCEMENT]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lists_each_meeting_through_recording_failure_and_retry() {
        let mut rig = Rig::with(Arc::new(FailsFirst::default()));
        for _ in 0..3 {
            rig.second(&[ANNA], true).await;
        }
        let list = rig.meetings.meetings().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, MeetingState::Recording);
        assert_eq!(list[0].people, vec!["Anna".to_string()]);

        for _ in 0..4 {
            rig.second(&[ANNA], true).await;
        }
        let meetings = rig.meetings.clone();
        let jobs = meetings.jobs.lock().await;
        rig.second(&[], false).await;
        rig.second(&[], false).await;
        assert_eq!(
            rig.meetings.meetings().unwrap()[0].state,
            MeetingState::Queued
        );
        drop(jobs);

        let failed = MeetingState::Failed {
            error: "whisper crashed".into(),
        };
        rig.settled(&failed).await;
        assert_eq!(rig.meetings.status().transcribing, 0);

        rig.meetings.transcribe_pending().unwrap();
        let list = rig.settled(&MeetingState::Done).await;
        assert!(list[0].transcript.as_ref().is_some_and(|p| p.exists()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_recording_started_by_hand_runs_without_auto_start_until_stopped() {
        let mut rig = Rig::new();
        rig.meetings.set_config(MeetingsConfig {
            auto_start: false,
            ..rig.meetings.config()
        });
        rig.second(&[ANNA], true).await;
        assert!(
            rig.page.captures.lock().unwrap().is_empty(),
            "nothing is captured until asked"
        );
        rig.meetings.start_now().unwrap();
        assert!(rig.meetings.recording());
        rig.now_ms = Timestamp::now().as_millisecond();
        for _ in 0..3 {
            rig.second(&[ANNA], true).await;
        }
        for _ in 0..10 {
            rig.second(&[], false).await;
        }
        assert!(rig.meetings.recording(), "being alone does not stop it");
        rig.meetings.stop_now();
        let text = std::fs::read_to_string(rig.transcribed().await).unwrap();
        assert!(text.contains("Anna:** hello"), "{text}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            rig.page.announced.lock().unwrap().as_slice(),
            [ANNOUNCEMENT]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_recording_stopped_by_hand_before_any_audio_leaves_nothing() {
        let rig = Rig::new();
        rig.meetings.start_now().unwrap();
        assert_eq!(rig.meeting_dirs().len(), 1);
        rig.meetings.stop_now();
        assert!(rig.meeting_dirs().is_empty());
        assert_eq!(rig.meetings.status().transcribing, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn someone_walking_past_leaves_nothing_behind() {
        let mut rig = Rig::new();
        rig.second(&[ANNA], true).await;
        rig.second(&[], false).await;
        rig.second(&[ANNA], true).await;
        rig.second(&[], false).await;
        assert!(rig.meeting_dirs().is_empty());
        assert!(rig.page.announced.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_conversation_with_too_little_speech_is_deleted() {
        let mut rig = Rig::new();
        for _ in 0..3 {
            rig.second(&[ANNA], false).await;
        }
        assert_eq!(rig.meeting_dirs().len(), 1);
        rig.second(&[ANNA], false).await;
        rig.second(&[], false).await;
        rig.second(&[], false).await;
        assert!(rig.meeting_dirs().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn turning_auto_start_off_lets_the_recording_finish_and_stop_ends_it() {
        let mut rig = Rig::new();
        for _ in 0..6 {
            rig.second(&[ANNA], true).await;
        }
        rig.meetings.set_config(MeetingsConfig {
            auto_start: false,
            ..rig.meetings.config()
        });
        rig.second(&[ANNA], true).await;
        assert!(matches!(
            rig.meetings.status().activity,
            Activity::Recording { .. }
        ));
        rig.meetings.stop_now();
        assert!(!rig.meetings.recording());
        rig.meetings.step(rig.now_ms + 1_000).await;
        assert_eq!(rig.meetings.status().activity, Activity::Off);
        assert_eq!(rig.page.captures.lock().unwrap().last(), Some(&false));
        rig.transcribed().await;
        rig.meetings.step(rig.now_ms + 2_000).await;
        assert_eq!(
            rig.page
                .captures
                .lock()
                .unwrap()
                .iter()
                .filter(|c| !**c)
                .count(),
            1
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_recording_left_untranscribed_is_picked_up_on_start() {
        let rig = Rig::new();
        let dir = rig.folder.join("2026-11-18 10-00 Kitchen");
        let mut recording = Recording::create(dir.clone(), 0).unwrap();
        recording
            .push(&Chunk {
                speaker: crate::audio::Speaker {
                    id: "u-anna".into(),
                    name: "Anna".into(),
                    me: false,
                },
                at_ms: 0,
                samples: tone_i16(2_000, 0.3),
            })
            .unwrap();
        recording.finish().unwrap();
        rig.meetings.recover();
        assert_eq!(rig.transcribed().await, dir.join(TRANSCRIPT_FILE));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn without_the_module_nothing_is_captured() {
        let dir = tempfile::tempdir().unwrap();
        let page = Arc::new(FakePage::default());
        let meetings = Meetings::new(MeetingsDeps {
            page: page.clone(),
            config: MeetingsConfig {
                auto_start: true,
                ..MeetingsConfig::default()
            },
            models_dir: dir.path().join("models"),
            module: Module::new(dir.path().join("module"), Some(dir.path().join("absent"))),
            transcriber: Arc::new(InProcess),
        });
        meetings.step(1_000).await;
        assert_eq!(meetings.status().activity, Activity::NeedsModule);
        assert_eq!(meetings.status().module, ModuleState::Missing);
        assert!(page.captures.lock().unwrap().is_empty());
        assert_eq!(
            meetings.status().summary(0),
            "Meetings: install the module in Settings"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_module_cannot_be_removed_while_a_meeting_is_transcribed() {
        let rig = Rig::new();
        let _job = rig.meetings.jobs.lock().await;
        rig.meetings.inner().status.transcribing = 1;
        let error = rig.meetings.remove_module().await.unwrap_err();
        assert!(
            format!("{error:#}").contains("being transcribed"),
            "{error:#}"
        );
        assert!(rig.meetings.module_ready());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_recording_cut_off_before_anyone_spoke_is_dropped_on_start() {
        let rig = Rig::new();
        let dir = rig.folder.join("2026-11-18 10-00");
        let recording = Recording::create(dir.clone(), 0).unwrap();
        std::mem::forget(recording);
        rig.meetings.recover();
        assert!(!dir.exists());
        assert_eq!(rig.meetings.status().transcribing, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recovery_leaves_the_live_recording_alone_and_queues_each_meeting_once() {
        let mut rig = Rig::new();
        for _ in 0..3 {
            rig.second(&[ANNA], true).await;
        }
        assert!(matches!(
            rig.meetings.status().activity,
            Activity::Recording { .. }
        ));
        let _job = rig.meetings.jobs.lock().await;
        rig.meetings.recover();
        assert_eq!(rig.meetings.status().transcribing, 0);
        assert_eq!(rig.meeting_dirs().len(), 1);

        let pending = rig.folder.join("2026-11-18 10-00 Kitchen");
        let mut recording = Recording::create(pending, 0).unwrap();
        recording
            .push(&Chunk {
                speaker: crate::audio::Speaker {
                    id: "u-anna".into(),
                    name: "Anna".into(),
                    me: false,
                },
                at_ms: 0,
                samples: tone_i16(1_000, 0.3),
            })
            .unwrap();
        recording.finish().unwrap();
        rig.meetings.recover();
        rig.meetings.recover();
        assert_eq!(rig.meetings.status().transcribing, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recovery_never_deletes_a_meeting_it_saw_empty_while_it_was_live() {
        let mut rig = Rig::new();
        let meetings = rig.meetings.clone();
        let _job = meetings.jobs.lock().await;
        for _ in 0..3 {
            rig.second(&[ANNA], true).await;
        }
        let live = rig.meeting_dirs().pop().unwrap();
        let stale = RecordedMeeting {
            dir: live.clone(),
            meeting: crate::recording::MeetingFile {
                started_ms: 0,
                ended_ms: 0,
                zones: vec![],
                tracks: vec![],
            },
        };
        for _ in 0..4 {
            rig.second(&[ANNA], true).await;
        }
        rig.second(&[], false).await;
        rig.second(&[], false).await;
        assert_eq!(rig.meetings.status().transcribing, 1);
        let (empty, pending) = rig.meetings.inner().claim(vec![stale.clone()]);
        assert!(
            empty.is_empty() && pending.is_empty(),
            "it is already queued"
        );

        rig.meetings.inner().release(&live);
        let (empty, pending) = rig.meetings.inner().claim(vec![stale]);
        assert!(empty.is_empty(), "meeting.json on disk has tracks now");
        assert_eq!(pending.len(), 1);
        assert!(live.exists());
    }

    struct Crashing;

    impl Transcribe for Crashing {
        fn transcribe<'a>(&'a self, _: Job<'a>) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { panic!("the transcriber crashed") })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_crashing_job_releases_its_meeting() {
        let rig = Rig::new();
        let meetings = Meetings::new(MeetingsDeps {
            page: rig.page.clone(),
            config: rig.meetings.config(),
            models_dir: rig._dir.path().join("models"),
            module: Module::new(
                rig._dir.path().join("module"),
                Some(rig._dir.path().join("better-kumo-transcriber")),
            ),
            transcriber: Arc::new(Crashing),
        });
        let dir = rig.folder.join("2026-11-18 10-00");
        let mut recording = Recording::create(dir.clone(), 0).unwrap();
        recording
            .push(&Chunk {
                speaker: crate::audio::Speaker {
                    id: "u-anna".into(),
                    name: "Anna".into(),
                    me: false,
                },
                at_ms: 0,
                samples: tone_i16(1_000, 0.3),
            })
            .unwrap();
        recording.finish().unwrap();
        meetings.recover();
        for _ in 0..200 {
            if meetings.status().transcribing == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(meetings.status().transcribing, 0);
        assert!(meetings.inner().queued.is_empty());
        assert!(dir.exists(), "the meeting stays for the next try");
    }

    #[test]
    fn the_status_line_says_what_is_happening() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let status = |activity| MeetingsStatus {
            activity,
            ..MeetingsStatus::default()
        };
        assert_eq!(
            status(Activity::Off).summary(0),
            "Meetings: start a recording by hand"
        );
        assert_eq!(
            status(Activity::Waiting {
                people: names(&["Anna"]),
                seconds_left: 3
            })
            .summary(0),
            "Meetings: starting in 3 s · Anna"
        );
        let recording = MeetingsStatus {
            activity: Activity::Recording {
                since_ms: 0,
                people: names(&["Anna", "Bob", "Cid", "Dee"]),
            },
            transcribing: 1,
            model_download: Some(42),
            ..MeetingsStatus::default()
        };
        assert_eq!(
            recording.summary(125_000),
            "Meetings: recording 2:05 · Anna, Bob +2 · speech model 42% · transcribing 1"
        );
        let broken = MeetingsStatus {
            activity: Activity::Listening,
            problem: Some("transcription failed".into()),
            ..MeetingsStatus::default()
        };
        assert_eq!(broken.summary(0), "Meetings: problem: transcription failed");
    }

    #[test]
    fn folder_names_are_safe_and_unique() {
        let dir = tempfile::tempdir().unwrap();
        let name = folder_name(0, "R&D: Room/1");
        assert!(name.ends_with(" R&D Room 1"), "{name}");
        std::fs::create_dir_all(dir.path().join("x")).unwrap();
        assert_eq!(unique_dir(dir.path(), "x"), dir.path().join("x (2)"));
    }
}
