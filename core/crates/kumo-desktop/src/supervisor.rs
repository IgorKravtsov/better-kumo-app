use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::kumospace::DesktopAppState;

const LAUNCH_GRACE: Duration = Duration::from_secs(45);
const POLL: Duration = Duration::from_secs(5);
const BACKOFF: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

pub trait DesktopApp: Send + Sync + 'static {
    fn state(&self) -> impl Future<Output = DesktopAppState> + Send;
    fn launch(&self) -> anyhow::Result<()>;
    fn restart_with_port(&self) -> impl Future<Output = anyhow::Result<()>> + Send;
    fn devtools_ready(&self) -> impl Future<Output = bool> + Send;
    fn devtools_url(&self) -> String;
}

pub trait RelayHandle: Clone + Send + Sync + 'static {
    fn stop(&self) -> impl Future<Output = ()> + Send;
}

pub type OnReady<R> = Box<dyn FnOnce(R) + Send>;

pub trait RelayStarter: Send + Sync + 'static {
    type Handle: RelayHandle;
    fn configured_port(&self) -> anyhow::Result<Option<u16>>;
    fn set_up_from_desktop_app(
        &self,
        devtools_url: String,
    ) -> impl Future<Output = anyhow::Result<u16>> + Send;
    fn start(&self, on_ready: OnReady<Self::Handle>) -> JoinHandle<anyhow::Result<()>>;
}

#[derive(Clone)]
pub struct Status<R> {
    pub desktop_app: DesktopAppState,
    /// What the tray shows: `config_problem` while `relay.toml` can't be used, else `relay_problem`.
    pub problem: Option<String>,
    config_problem: Option<String>,
    relay_problem: Option<String>,
    pub relay: Option<R>,
    pub port: u16,
}

impl<R> Default for Status<R> {
    fn default() -> Self {
        Self {
            desktop_app: DesktopAppState::NotRunning,
            problem: None,
            config_problem: None,
            relay_problem: None,
            relay: None,
            port: 8787,
        }
    }
}

pub type Shared<R> = Arc<Mutex<Status<R>>>;

fn update<R>(status: &Shared<R>, f: impl FnOnce(&mut Status<R>)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
        s.problem = s.config_problem.clone().or_else(|| s.relay_problem.clone());
    }
}

struct Failed;

pub struct Supervisor<A, S: RelayStarter> {
    pub status: Shared<S::Handle>,
    pub shutdown: watch::Receiver<bool>,
    pub app: A,
    pub relays: S,
    pub keep_open: Arc<AtomicBool>,
}

impl<A: DesktopApp, S: RelayStarter> Supervisor<A, S> {
    pub async fn run(mut self) {
        let mut relay_task: Option<JoinHandle<anyhow::Result<()>>> = None;
        let mut last_launch: Option<Instant> = None;
        // Failed relay starts back off on their own, so the desktop app is still polled every POLL.
        let mut retry_at: Option<Instant> = None;
        loop {
            let state = self.app.state().await;
            update(&self.status, |s| s.desktop_app = state);
            if last_launch.is_none_or(|t| t.elapsed() > LAUNCH_GRACE)
                && self.bring_up_desktop_app(state).await
            {
                last_launch = Some(Instant::now());
            }

            if let Some(task) = relay_task.take_if(|t| t.is_finished())
                && self.record_exit(task).await
            {
                retry_at = Some(Instant::now() + BACKOFF);
            }

            if relay_task.is_none()
                && retry_at.is_none_or(|t| Instant::now() >= t)
                && !*self.shutdown.borrow()
            {
                match self.start_relay(state).await {
                    Ok(task) => {
                        relay_task = task;
                        retry_at = None;
                    }
                    Err(Failed) => retry_at = Some(Instant::now() + BACKOFF),
                }
            }

            let mut stopping = false;
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                changed = self.shutdown.changed() => stopping = changed.is_err(),
            }
            if stopping || *self.shutdown.borrow() {
                self.stop(relay_task).await;
                return;
            }
        }
    }

    async fn bring_up_desktop_app(&self, state: DesktopAppState) -> bool {
        match state {
            DesktopAppState::NotRunning if self.keep_open.load(Ordering::Relaxed) => {
                if let Err(e) = self.app.launch() {
                    eprintln!("kumospace: launch failed: {e:#}");
                }
                true
            }
            DesktopAppState::RunningWithoutPort { fresh: true } => {
                eprintln!("kumospace: started without the debug port, restarting it");
                if let Err(e) = self.app.restart_with_port().await {
                    eprintln!("kumospace: restart failed: {e:#}");
                }
                true
            }
            _ => false,
        }
    }

    async fn record_exit(&self, task: JoinHandle<anyhow::Result<()>>) -> bool {
        let problem = match task.await {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(format!("{e:#}")),
            Err(e) => Some(e.to_string()),
        };
        if let Some(problem) = &problem {
            eprintln!("relay stopped: {problem}");
        }
        let crashed = problem.is_some();
        update(&self.status, |s| {
            s.relay = None;
            s.relay_problem = problem;
        });
        crashed
    }

    /// `Ok(None)` while waiting for the desktop app; only `Err` starts the backoff.
    async fn start_relay(
        &self,
        state: DesktopAppState,
    ) -> Result<Option<JoinHandle<anyhow::Result<()>>>, Failed> {
        let configured = match self.relays.configured_port() {
            Ok(configured) => {
                update(&self.status, |s| s.config_problem = None);
                configured
            }
            Err(e) => {
                let problem = format!("{e:#}");
                eprintln!("config: {problem}");
                update(&self.status, |s| s.config_problem = Some(problem));
                return Err(Failed);
            }
        };
        match self.prepare(configured, state).await {
            Ok(Some(port)) => {
                update(&self.status, |s| {
                    s.port = port;
                    s.relay_problem = None;
                });
                let ready = self.status.clone();
                Ok(Some(self.relays.start(Box::new(move |relay| {
                    update(&ready, |s| s.relay = Some(relay))
                }))))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                let problem = format!("{e:#}");
                eprintln!("setup: {problem}");
                update(&self.status, |s| s.relay_problem = Some(problem));
                Err(Failed)
            }
        }
    }

    async fn prepare(
        &self,
        configured: Option<u16>,
        state: DesktopAppState,
    ) -> anyhow::Result<Option<u16>> {
        if configured.is_some() {
            return Ok(configured);
        }
        if state != DesktopAppState::Running || !self.app.devtools_ready().await {
            return Ok(None);
        }
        self.relays
            .set_up_from_desktop_app(self.app.devtools_url())
            .await
            .map(Some)
    }

    async fn stop(&self, relay_task: Option<JoinHandle<anyhow::Result<()>>>) {
        if let Some(mut task) = relay_task {
            let relay = self.status.lock().ok().and_then(|s| s.relay.clone());
            match relay {
                Some(relay) => relay.stop().await,
                None => task.abort(),
            }
            let _ = tokio::time::timeout(STOP_TIMEOUT, &mut task).await;
        }
        update(&self.status, |s| s.relay = None);
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use tokio::sync::oneshot;

    use super::*;

    #[derive(Default)]
    struct AppScript {
        state: Option<DesktopAppState>,
        devtools_ready: bool,
        launches: usize,
        restarts: usize,
    }

    #[derive(Clone, Default)]
    struct FakeApp(Arc<Mutex<AppScript>>);

    impl FakeApp {
        fn set(&self, state: DesktopAppState, devtools_ready: bool) {
            let mut script = self.0.lock().unwrap();
            script.state = Some(state);
            script.devtools_ready = devtools_ready;
        }

        fn launches(&self) -> usize {
            self.0.lock().unwrap().launches
        }

        fn restarts(&self) -> usize {
            self.0.lock().unwrap().restarts
        }
    }

    impl DesktopApp for FakeApp {
        async fn state(&self) -> DesktopAppState {
            self.0
                .lock()
                .unwrap()
                .state
                .unwrap_or(DesktopAppState::NotRunning)
        }

        fn launch(&self) -> anyhow::Result<()> {
            self.0.lock().unwrap().launches += 1;
            Ok(())
        }

        async fn restart_with_port(&self) -> anyhow::Result<()> {
            let mut script = self.0.lock().unwrap();
            script.restarts += 1;
            script.state = Some(DesktopAppState::Running);
            Ok(())
        }

        async fn devtools_ready(&self) -> bool {
            self.0.lock().unwrap().devtools_ready
        }

        fn devtools_url(&self) -> String {
            "http://127.0.0.1:9222".into()
        }
    }

    #[derive(Default)]
    struct RelayScript {
        configured: bool,
        broken_config: Option<String>,
        broken_setup: Option<String>,
        setups: usize,
        starts: usize,
        stops: usize,
        exit: Option<oneshot::Sender<anyhow::Result<()>>>,
    }

    #[derive(Clone, Default)]
    struct FakeRelays(Arc<Mutex<RelayScript>>);

    impl FakeRelays {
        fn configured() -> Self {
            let relays = Self::default();
            relays.0.lock().unwrap().configured = true;
            relays
        }

        fn crash(&self, problem: &str) {
            let exit = self.0.lock().unwrap().exit.take().unwrap();
            let _ = exit.send(Err(anyhow!(problem.to_string())));
        }

        fn starts(&self) -> usize {
            self.0.lock().unwrap().starts
        }

        fn setups(&self) -> usize {
            self.0.lock().unwrap().setups
        }

        fn stops(&self) -> usize {
            self.0.lock().unwrap().stops
        }
    }

    #[derive(Clone)]
    struct FakeRelay(Arc<Mutex<RelayScript>>);

    impl RelayHandle for FakeRelay {
        async fn stop(&self) {
            let mut script = self.0.lock().unwrap();
            script.stops += 1;
            if let Some(exit) = script.exit.take() {
                let _ = exit.send(Ok(()));
            }
        }
    }

    impl RelayStarter for FakeRelays {
        type Handle = FakeRelay;

        fn configured_port(&self) -> anyhow::Result<Option<u16>> {
            let script = self.0.lock().unwrap();
            if let Some(problem) = &script.broken_config {
                return Err(anyhow!(problem.clone()));
            }
            Ok(script.configured.then_some(8787))
        }

        async fn set_up_from_desktop_app(&self, _: String) -> anyhow::Result<u16> {
            let mut script = self.0.lock().unwrap();
            script.setups += 1;
            if let Some(problem) = &script.broken_setup {
                return Err(anyhow!(problem.clone()));
            }
            script.configured = true;
            Ok(8787)
        }

        fn start(&self, on_ready: OnReady<FakeRelay>) -> JoinHandle<anyhow::Result<()>> {
            let (exit, exited) = oneshot::channel();
            {
                let mut script = self.0.lock().unwrap();
                script.starts += 1;
                script.exit = Some(exit);
            }
            on_ready(FakeRelay(self.0.clone()));
            tokio::spawn(async move { exited.await.unwrap_or(Ok(())) })
        }
    }

    struct Running {
        status: Shared<FakeRelay>,
        shutdown: watch::Sender<bool>,
        task: JoinHandle<()>,
    }

    impl Running {
        fn relay_up(&self) -> bool {
            self.status.lock().unwrap().relay.is_some()
        }

        fn problem(&self) -> Option<String> {
            self.status.lock().unwrap().problem.clone()
        }
    }

    fn supervise(app: &FakeApp, relays: &FakeRelays) -> Running {
        supervise_with(app, relays, Arc::new(AtomicBool::new(true)))
    }

    fn supervise_with(app: &FakeApp, relays: &FakeRelays, keep_open: Arc<AtomicBool>) -> Running {
        let status: Shared<FakeRelay> = Arc::default();
        let (shutdown, receiver) = watch::channel(false);
        let task = tokio::spawn(
            Supervisor {
                status: status.clone(),
                shutdown: receiver,
                app: app.clone(),
                relays: relays.clone(),
                keep_open,
            }
            .run(),
        );
        Running {
            status,
            shutdown,
            task,
        }
    }

    async fn at(start: Instant, seconds: u64) {
        tokio::time::sleep_until(start + Duration::from_secs(seconds)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn shows_a_config_it_cannot_use_even_without_the_desktop_app() {
        let start = Instant::now();
        let app = FakeApp::default();
        let relays = FakeRelays::default();
        relays.0.lock().unwrap().broken_config = Some("relay.toml has an empty secret".into());
        let running = supervise_with(&app, &relays, Arc::new(AtomicBool::new(false)));
        at(start, 1).await;
        assert_eq!(
            running.problem().as_deref(),
            Some("relay.toml has an empty secret")
        );
        assert_eq!(relays.starts(), 0);

        relays.0.lock().unwrap().broken_config = None;
        at(start, 32).await;
        assert_eq!(running.problem(), None);
        assert_eq!(relays.starts(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_setup_failure_stays_shown_after_the_desktop_app_closes() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::Running, true);
        let relays = FakeRelays::default();
        relays.0.lock().unwrap().broken_setup = Some("no Kumospace session".into());
        let running = supervise_with(&app, &relays, Arc::new(AtomicBool::new(false)));
        at(start, 1).await;
        assert_eq!(running.problem().as_deref(), Some("no Kumospace session"));

        app.set(DesktopAppState::NotRunning, false);
        at(start, 32).await;
        assert_eq!(running.problem().as_deref(), Some("no Kumospace session"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_config_error_hides_a_setup_failure_only_while_it_lasts() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::Running, true);
        let relays = FakeRelays::default();
        relays.0.lock().unwrap().broken_setup = Some("no Kumospace session".into());
        let running = supervise_with(&app, &relays, Arc::new(AtomicBool::new(false)));
        at(start, 1).await;
        app.set(DesktopAppState::NotRunning, false);
        relays.0.lock().unwrap().broken_config = Some("relay.toml is not valid".into());
        at(start, 32).await;
        assert_eq!(
            running.problem().as_deref(),
            Some("relay.toml is not valid")
        );

        relays.0.lock().unwrap().broken_config = None;
        at(start, 62).await;
        assert_eq!(running.problem().as_deref(), Some("no Kumospace session"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_desktop_app_is_still_polled_while_the_relay_backs_off() {
        let start = Instant::now();
        let app = FakeApp::default();
        let relays = FakeRelays::default();
        relays.0.lock().unwrap().broken_config = Some("relay.toml is not valid".into());
        let running = supervise_with(&app, &relays, Arc::new(AtomicBool::new(false)));
        at(start, 1).await;
        app.set(DesktopAppState::Running, true);
        at(start, 6).await;
        assert_eq!(
            running.status.lock().unwrap().desktop_app,
            DesktopAppState::Running
        );
        assert_eq!(
            running.problem().as_deref(),
            Some("relay.toml is not valid")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn setup_is_retried_on_the_next_poll_after_waiting_for_the_desktop_app() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::Running, true);
        let relays = FakeRelays::default();
        relays.0.lock().unwrap().broken_setup = Some("no Kumospace session".into());
        let running = supervise_with(&app, &relays, Arc::new(AtomicBool::new(false)));
        at(start, 1).await;
        app.set(DesktopAppState::NotRunning, false);
        at(start, 32).await;
        assert_eq!(relays.0.lock().unwrap().setups, 1);
        assert_eq!(running.problem().as_deref(), Some("no Kumospace session"));

        relays.0.lock().unwrap().broken_setup = None;
        app.set(DesktopAppState::Running, true);
        at(start, 36).await;
        assert_eq!(relays.0.lock().unwrap().setups, 2);
        assert_eq!(relays.starts(), 1);
        assert_eq!(running.problem(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn launches_the_desktop_app_once_per_45_seconds() {
        let start = Instant::now();
        let app = FakeApp::default();
        let _running = supervise(&app, &FakeRelays::default());
        at(start, 1).await;
        assert_eq!(app.launches(), 1);
        at(start, 46).await;
        assert_eq!(app.launches(), 1);
        at(start, 51).await;
        assert_eq!(app.launches(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn leaves_a_closed_desktop_app_closed_when_asked_to() {
        let start = Instant::now();
        let app = FakeApp::default();
        let keep_open = Arc::new(AtomicBool::new(false));
        let _running = supervise_with(&app, &FakeRelays::default(), keep_open.clone());
        at(start, 60).await;
        assert_eq!(app.launches(), 0);

        keep_open.store(true, Ordering::Relaxed);
        at(start, 66).await;
        assert_eq!(app.launches(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn stops_relaunching_once_asked_to() {
        let start = Instant::now();
        let app = FakeApp::default();
        let keep_open = Arc::new(AtomicBool::new(true));
        let _running = supervise_with(&app, &FakeRelays::default(), keep_open.clone());
        at(start, 1).await;
        assert_eq!(app.launches(), 1);

        keep_open.store(false, Ordering::Relaxed);
        at(start, 120).await;
        assert_eq!(app.launches(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn adds_the_debug_port_to_a_fresh_desktop_app_even_when_not_keeping_it_open() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::RunningWithoutPort { fresh: true }, false);
        let keep_open = Arc::new(AtomicBool::new(false));
        let _running = supervise_with(&app, &FakeRelays::default(), keep_open);
        at(start, 1).await;
        assert_eq!(app.restarts(), 1);
        assert_eq!(app.launches(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn restarts_a_desktop_app_started_without_the_debug_port() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::RunningWithoutPort { fresh: true }, false);
        let _running = supervise(&app, &FakeRelays::default());
        at(start, 1).await;
        assert_eq!(app.restarts(), 1);
        assert_eq!(app.launches(), 0);
        at(start, 60).await;
        assert_eq!(app.restarts(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn leaves_an_older_desktop_app_without_the_debug_port_alone() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::RunningWithoutPort { fresh: false }, false);
        let _running = supervise(&app, &FakeRelays::default());
        at(start, 60).await;
        assert_eq!(app.restarts(), 0);
        assert_eq!(app.launches(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn relay_crash_is_recorded_and_retried_after_30_seconds() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::Running, true);
        let relays = FakeRelays::configured();
        let running = supervise(&app, &relays);
        at(start, 1).await;
        assert_eq!(relays.starts(), 1);
        assert!(running.relay_up());

        relays.crash("cannot listen on 0.0.0.0:8787");
        at(start, 6).await;
        assert_eq!(
            running.problem().as_deref(),
            Some("cannot listen on 0.0.0.0:8787")
        );
        assert!(!running.relay_up());
        at(start, 34).await;
        assert_eq!(relays.starts(), 1);
        at(start, 36).await;
        assert_eq!(relays.starts(), 2);
        assert!(running.relay_up());
        assert_eq!(running.problem(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn relay_is_set_up_once_the_desktop_app_comes_back() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::Running, false);
        let relays = FakeRelays::default();
        let running = supervise(&app, &relays);
        at(start, 1).await;
        assert!(!running.relay_up());

        app.set(DesktopAppState::NotRunning, false);
        at(start, 6).await;
        assert_eq!(app.launches(), 1);
        assert!(!running.relay_up());

        app.set(DesktopAppState::Running, true);
        at(start, 11).await;
        assert_eq!(relays.setups(), 1);
        assert_eq!(relays.starts(), 1);
        assert!(running.relay_up());
    }

    #[tokio::test(start_paused = true)]
    async fn relay_keeps_running_while_the_desktop_app_restarts() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::Running, true);
        let relays = FakeRelays::configured();
        let running = supervise(&app, &relays);
        at(start, 1).await;
        assert!(running.relay_up());

        app.set(DesktopAppState::NotRunning, false);
        at(start, 6).await;
        assert_eq!(app.launches(), 1);
        app.set(DesktopAppState::Running, true);
        at(start, 11).await;
        assert!(running.relay_up());
        assert_eq!(relays.starts(), 1);
        assert_eq!(relays.stops(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_stops_the_relay() {
        let start = Instant::now();
        let app = FakeApp::default();
        app.set(DesktopAppState::Running, true);
        let relays = FakeRelays::configured();
        let running = supervise(&app, &relays);
        at(start, 1).await;
        assert!(running.relay_up());

        running.shutdown.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), running.task)
            .await
            .expect("supervisor returns promptly")
            .unwrap();
        assert_eq!(relays.stops(), 1);
        assert!(running.status.lock().unwrap().relay.is_none());
    }
}
