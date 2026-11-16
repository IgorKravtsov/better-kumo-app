use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kumo_relay::{Config, Relay};
use tokio::sync::watch;

use crate::kumospace::{self, Kumospace};

#[derive(Clone)]
pub struct Status {
    pub kumospace: Kumospace,
    pub problem: Option<String>,
    pub relay: Option<Arc<Relay>>,
    pub port: u16,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            kumospace: Kumospace::NotRunning,
            problem: None,
            relay: None,
            port: 8787,
        }
    }
}

pub type Shared = Arc<Mutex<Status>>;

fn update(status: &Shared, f: impl FnOnce(&mut Status)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

pub async fn supervise(status: Shared, mut shutdown: watch::Receiver<bool>) {
    let config_path = kumo_relay::config::default_config_path();
    let mut relay_task: Option<tokio::task::JoinHandle<anyhow::Result<()>>> = None;
    let mut last_launch: Option<Instant> = None;
    loop {
        let state = tokio::task::spawn_blocking(kumospace::state)
            .await
            .unwrap_or(Kumospace::NotRunning);
        update(&status, |s| s.kumospace = state);
        let may_launch = last_launch.is_none_or(|t| t.elapsed() > Duration::from_secs(45));
        match state {
            Kumospace::NotRunning if may_launch => {
                last_launch = Some(Instant::now());
                if let Err(e) = kumospace::launch() {
                    eprintln!("kumospace: launch failed: {e:#}");
                }
            }
            Kumospace::RunningWithoutPort { fresh: true } if may_launch => {
                last_launch = Some(Instant::now());
                eprintln!("kumospace: started without the debug port, restarting it");
                let _ = tokio::task::spawn_blocking(kumospace::restart_with_port).await;
            }
            _ => {}
        }

        if relay_task.as_ref().is_some_and(|t| t.is_finished())
            && let Some(task) = relay_task.take()
        {
            let problem = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(format!("{e:#}")),
                Err(e) => Some(e.to_string()),
            };
            if let Some(problem) = &problem {
                eprintln!("relay stopped: {problem}");
            }
            update(&status, |s| {
                s.relay = None;
                s.problem = problem;
            });
        }

        if relay_task.is_none() && !*shutdown.borrow() {
            match prepare(&config_path, state).await {
                Ok(Some(config)) => {
                    update(&status, |s| {
                        s.port = config.port();
                        s.problem = None;
                    });
                    relay_task = Some(start_relay(
                        config_path.clone(),
                        status.clone(),
                        shutdown.clone(),
                    ));
                }
                Ok(None) => {}
                Err(e) => {
                    let problem = format!("{e:#}");
                    eprintln!("setup: {problem}");
                    update(&status, |s| s.problem = Some(problem));
                }
            }
        }

        let failing = status.lock().map(|s| s.problem.is_some()).unwrap_or(false);
        let pause = if failing { 30 } else { 5 };
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(pause)) => {}
            _ = shutdown.changed() => {}
        }
        if *shutdown.borrow() {
            if let Some(task) = relay_task.take() {
                let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
            }
            return;
        }
    }
}

async fn prepare(path: &PathBuf, state: Kumospace) -> anyhow::Result<Option<Config>> {
    if let Ok(config) = Config::load(path)
        && kumo_relay::is_signed_in(&config)
    {
        return Ok(Some(config));
    }
    if state != Kumospace::Running || !kumospace::cdp_ready().await {
        return Ok(None);
    }
    kumo_relay::setup_from_desktop(path, &kumospace::cdp_url())
        .await
        .map(Some)
}

fn start_relay(
    path: PathBuf,
    status: Shared,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        let ready = status.clone();
        kumo_relay::run(
            &path,
            async move {
                let _ = shutdown.wait_for(|stop| *stop).await;
            },
            move |relay| update(&ready, |s| s.relay = Some(relay)),
        )
        .await
    })
}
