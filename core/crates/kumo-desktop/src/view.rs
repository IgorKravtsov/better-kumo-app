//! The windows only draw this; every decision about the computer is made here.

use kumo_meetings::ModuleState;
use kumo_relay::mcp_setup::AgentSetup;
use kumo_relay::pair_page::PairCode;
use serde::Serialize;

use crate::kumospace::DesktopAppState;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DesktopView {
    pub version: &'static str,
    pub kumospace: KumospaceView,
    pub relay: RelayView,
    pub phone: PhoneView,
    pub meetings: MeetingsView,
    pub agents: AgentsView,
    pub settings: SettingsView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppView {
    NotInstalled,
    Closed,
    NoPort,
    Running,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KumospaceView {
    pub app: AppView,
    pub connected: bool,
    pub space: Option<String>,
    pub can_restart: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayState {
    Off,
    Starting,
    On,
    Problem,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RelayView {
    pub state: RelayState,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhoneView {
    pub codes: Vec<PairCode>,
    pub paired: bool,
    pub waiting: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeetingsView {
    pub available: bool,
    pub module: &'static str,
    pub percent: u8,
    pub error: Option<String>,
    pub folder: Option<String>,
    pub auto_start: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentsView {
    pub enabled: bool,
    pub list: Vec<AgentSetup>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsView {
    pub start_at_login: bool,
    pub keep_kumospace_open: bool,
    /// None: this install cannot update itself (APT, a build from source).
    pub auto_update: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RelayFacts {
    pub desktop_connected: bool,
    pub phone_seen: bool,
    pub phones: usize,
    pub waiting_for_phone: bool,
    pub space: Option<String>,
    pub meetings: Option<ModuleState>,
    pub meetings_folder: Option<String>,
    pub meetings_auto_start: bool,
}

pub struct Inputs {
    pub desktop_app: DesktopAppState,
    pub relay: Option<RelayFacts>,
    pub problem: Option<String>,
    pub codes: Vec<PairCode>,
    pub agents: AgentsView,
    pub settings: SettingsView,
}

impl DesktopView {
    pub fn build(inputs: Inputs) -> Self {
        let Inputs {
            desktop_app,
            relay,
            problem,
            codes,
            agents,
            settings,
        } = inputs;
        let app = match desktop_app {
            DesktopAppState::NotInstalled => AppView::NotInstalled,
            DesktopAppState::NotRunning => AppView::Closed,
            DesktopAppState::RunningWithoutPort { .. } => AppView::NoPort,
            DesktopAppState::Running => AppView::Running,
        };
        let state = match (&relay, &problem) {
            (Some(_), _) => RelayState::On,
            (None, Some(_)) => RelayState::Problem,
            (None, None) if app == AppView::Running => RelayState::Starting,
            (None, None) => RelayState::Off,
        };
        let facts = relay.unwrap_or_default();
        let (module, percent, error) = match &facts.meetings {
            None => ("missing", 0, None),
            Some(ModuleState::Missing) => ("missing", 0, None),
            Some(ModuleState::Unsupported) => ("unsupported", 0, None),
            Some(ModuleState::Installing { percent, .. }) => ("installing", *percent, None),
            Some(ModuleState::Ready { .. }) => ("ready", 100, None),
            Some(ModuleState::Failed { error }) => ("failed", 0, Some(error.clone())),
        };
        Self {
            version: env!("CARGO_PKG_VERSION"),
            kumospace: KumospaceView {
                app,
                connected: state == RelayState::On && facts.desktop_connected,
                space: facts.space.clone(),
                can_restart: app == AppView::NoPort,
            },
            relay: RelayView {
                state,
                problem: if state == RelayState::Problem {
                    problem
                } else {
                    None
                },
            },
            phone: PhoneView {
                codes,
                paired: facts.phone_seen || facts.phones > 0,
                waiting: facts.waiting_for_phone,
            },
            meetings: MeetingsView {
                available: facts.meetings.is_some(),
                module,
                percent,
                error,
                folder: facts.meetings_folder,
                auto_start: facts.meetings_auto_start,
            },
            agents,
            settings,
        }
    }
}

/// Caches the interface scan and the agents' config reads, which are too slow for every refresh.
pub struct Gatherer {
    config: Option<kumo_relay::Config>,
    codes: Vec<PairCode>,
    agents: Vec<AgentSetup>,
    refreshed: Option<std::time::Instant>,
}

const SLOW_REFRESH: std::time::Duration = std::time::Duration::from_secs(4);

impl Gatherer {
    pub fn new() -> Self {
        Self {
            config: None,
            codes: Vec::new(),
            agents: Vec::new(),
            refreshed: None,
        }
    }

    pub fn relay_config(&mut self, running: bool) -> Option<&kumo_relay::Config> {
        if !running {
            self.config = None;
        } else if self.config.is_none() {
            self.config = kumo_relay::Config::load(&kumo_relay::config::default_config_path()).ok();
        }
        self.config.as_ref()
    }

    pub fn view(
        &mut self,
        status: &crate::supervisor::Status<std::sync::Arc<kumo_relay::Relay>>,
        agents_enabled: bool,
        settings: SettingsView,
    ) -> DesktopView {
        let relay = status.relay.as_ref();
        let space = self.relay_config(relay.is_some()).map(|c| c.space.clone());
        if self.refreshed.is_none_or(|t| t.elapsed() >= SLOW_REFRESH) {
            self.refreshed = Some(std::time::Instant::now());
            self.agents = kumo_relay::mcp_setup::current().agents;
            self.codes = match (relay, self.config.as_ref()) {
                (Some(relay), Some(config)) => {
                    kumo_relay::pair_page::codes_for(config, &relay.pair_addresses(config.port()))
                }
                _ => Vec::new(),
            };
        }
        let facts = relay.map(|relay| {
            let state = relay.status();
            RelayFacts {
                desktop_connected: state.desktop_connected,
                phone_seen: relay.phone_seen(),
                phones: state.phones,
                waiting_for_phone: relay.waiting_for_phone(),
                space: space.clone(),
                meetings: relay.meetings().map(|m| m.module),
                meetings_folder: relay.meetings_folder().map(|f| f.display().to_string()),
                meetings_auto_start: relay.meetings_auto_start(),
            }
        });
        DesktopView::build(Inputs {
            desktop_app: status.desktop_app,
            relay: facts,
            problem: status.problem.clone(),
            codes: if relay.is_some() {
                self.codes.clone()
            } else {
                Vec::new()
            },
            agents: AgentsView {
                enabled: agents_enabled,
                list: self.agents.clone(),
            },
            settings,
        })
    }

    pub fn refresh_soon(&mut self) {
        self.refreshed = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(desktop_app: DesktopAppState, relay: Option<RelayFacts>) -> Inputs {
        Inputs {
            desktop_app,
            relay,
            problem: None,
            codes: Vec::new(),
            agents: AgentsView {
                enabled: true,
                list: Vec::new(),
            },
            settings: SettingsView {
                start_at_login: true,
                keep_kumospace_open: true,
                auto_update: None,
            },
        }
    }

    #[test]
    fn kumospace_is_connected_only_once_the_relay_reaches_its_page() {
        let starting = DesktopView::build(inputs(DesktopAppState::Running, None));
        assert_eq!(starting.relay.state, RelayState::Starting);
        assert!(!starting.kumospace.connected);

        let attaching = RelayFacts {
            space: Some("acme".into()),
            ..RelayFacts::default()
        };
        let up = DesktopView::build(inputs(DesktopAppState::Running, Some(attaching.clone())));
        assert_eq!(up.relay.state, RelayState::On);
        assert!(
            !up.kumospace.connected,
            "the Relay runs but has not found the page yet"
        );

        let attached = RelayFacts {
            desktop_connected: true,
            ..attaching
        };
        let done = DesktopView::build(inputs(DesktopAppState::Running, Some(attached)));
        assert!(done.kumospace.connected);
        assert_eq!(done.kumospace.space.as_deref(), Some("acme"));
    }

    #[test]
    fn a_restart_is_offered_only_without_the_debugging_port() {
        for (app, offered) in [
            (DesktopAppState::NotInstalled, false),
            (DesktopAppState::NotRunning, false),
            (DesktopAppState::RunningWithoutPort { fresh: false }, true),
            (DesktopAppState::RunningWithoutPort { fresh: true }, true),
            (DesktopAppState::Running, false),
        ] {
            assert_eq!(
                DesktopView::build(inputs(app, None)).kumospace.can_restart,
                offered,
                "{app:?}"
            );
        }
    }

    #[test]
    fn a_problem_shows_only_while_the_relay_is_down() {
        let mut down = inputs(DesktopAppState::Running, None);
        down.problem = Some("relay.toml has an empty secret".into());
        let view = DesktopView::build(down);
        assert_eq!(view.relay.state, RelayState::Problem);
        assert_eq!(
            view.relay.problem.as_deref(),
            Some("relay.toml has an empty secret")
        );

        let mut up = inputs(DesktopAppState::Running, Some(RelayFacts::default()));
        up.problem = Some("an old problem".into());
        assert_eq!(DesktopView::build(up).relay.problem, None);
    }

    #[test]
    fn a_phone_counts_once_it_reached_the_relay_or_registered_for_pushes() {
        let view = |facts| DesktopView::build(inputs(DesktopAppState::Running, Some(facts)));
        assert!(!view(RelayFacts::default()).phone.paired);
        assert!(
            view(RelayFacts {
                phone_seen: true,
                ..RelayFacts::default()
            })
            .phone
            .paired
        );
        assert!(
            view(RelayFacts {
                phones: 1,
                ..RelayFacts::default()
            })
            .phone
            .paired
        );
    }

    #[test]
    fn the_meetings_module_maps_to_what_the_pages_draw() {
        let view = |module| {
            DesktopView::build(inputs(
                DesktopAppState::Running,
                Some(RelayFacts {
                    meetings: module,
                    ..RelayFacts::default()
                }),
            ))
            .meetings
        };
        assert!(!view(None).available);
        let installing = view(Some(ModuleState::Installing {
            step: "model".into(),
            percent: 40,
        }));
        assert_eq!((installing.module, installing.percent), ("installing", 40));
        let failed = view(Some(ModuleState::Failed {
            error: "no space left".into(),
        }));
        assert_eq!(
            (failed.module, failed.error.as_deref()),
            ("failed", Some("no space left"))
        );
        assert_eq!(
            view(Some(ModuleState::Ready {
                version: "0.2.19".into()
            }))
            .module,
            "ready"
        );
    }

    #[test]
    fn it_serializes_in_the_shape_the_pages_read() {
        let json = serde_json::to_value(DesktopView::build(inputs(
            DesktopAppState::RunningWithoutPort { fresh: false },
            None,
        )))
        .unwrap();
        assert_eq!(json["kumospace"]["app"], "no_port");
        assert_eq!(json["relay"]["state"], "off");
        assert_eq!(json["settings"]["auto_update"], serde_json::Value::Null);
    }
}
