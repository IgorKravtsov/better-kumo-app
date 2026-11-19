use std::collections::BTreeSet;
use std::time::Duration;

pub type Group = BTreeSet<String>;

const RESUME_LEAD_MS: i64 = 2_000;
pub const SEEN_LEAD_MS: i64 = 1_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub auto_start: bool,
    pub settle: Duration,
    pub alone_stop: Duration,
    pub quiet_stop: Duration,
    pub min_speech: Duration,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            auto_start: true,
            settle: Duration::from_secs(5),
            alone_stop: Duration::from_secs(5),
            quiet_stop: Duration::from_secs(180),
            min_speech: Duration::from_secs(20),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Alone,
    Quiet,
    Ended,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Wait,
    Start { from_ms: i64 },
    Stop { keep: bool, reason: StopReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Settling {
        group: Group,
        since_ms: i64,
        speech_ms: i64,
    },
    Recording {
        started_ms: i64,
        last_company_ms: i64,
        last_speech_ms: i64,
        speech_ms: i64,
    },
    Manual {
        started_ms: i64,
    },
    Resting {
        group: Group,
    },
    Held {
        group: Group,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Moment<'a> {
    pub now_ms: i64,
    pub earshot: &'a Group,
    pub speech_ms: i64,
}

#[derive(Debug, Clone)]
pub struct Detector {
    rules: Rules,
    phase: Phase,
}

impl Detector {
    pub fn new(rules: Rules) -> Self {
        Self {
            rules,
            phase: Phase::Idle,
        }
    }

    pub fn rules(&self) -> Rules {
        self.rules
    }

    pub fn set_rules(&mut self, rules: Rules) {
        self.rules = rules;
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn settle_left_ms(&self, now_ms: i64) -> Option<i64> {
        match &self.phase {
            Phase::Settling { since_ms, .. } => {
                Some((ms(self.rules.settle) - (now_ms - since_ms)).max(0))
            }
            _ => None,
        }
    }

    pub fn tick(&mut self, moment: Moment<'_>) -> Decision {
        let Moment {
            now_ms,
            earshot,
            speech_ms: heard,
        } = moment;
        let (next, decision) = match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Manual { started_ms } => (Phase::Manual { started_ms }, Decision::Wait),
            Phase::Held { .. } if earshot.is_empty() => (Phase::Idle, Decision::Wait),
            Phase::Held { group } if &group == earshot => (Phase::Held { group }, Decision::Wait),
            Phase::Held { .. } | Phase::Idle if !self.rules.auto_start || earshot.is_empty() => {
                (Phase::Idle, Decision::Wait)
            }
            Phase::Held { .. } | Phase::Idle => {
                self.settling(earshot.clone(), now_ms, heard, now_ms)
            }
            Phase::Settling { .. } | Phase::Resting { .. } if !self.rules.auto_start => {
                (Phase::Idle, Decision::Wait)
            }
            Phase::Settling { .. } if earshot.is_empty() => (Phase::Idle, Decision::Wait),
            Phase::Settling { group, .. } if &group != earshot => {
                self.settling(earshot.clone(), now_ms, heard, now_ms)
            }
            Phase::Settling {
                group,
                since_ms,
                speech_ms,
            } => self.settling(group, since_ms, speech_ms + heard, now_ms),
            Phase::Recording {
                started_ms,
                mut last_company_ms,
                mut last_speech_ms,
                mut speech_ms,
            } => {
                if !earshot.is_empty() {
                    last_company_ms = now_ms;
                }
                if heard > 0 {
                    last_speech_ms = now_ms;
                    speech_ms += heard;
                }
                let keep = speech_ms >= ms(self.rules.min_speech);
                if now_ms - last_company_ms >= ms(self.rules.alone_stop) {
                    (
                        Phase::Idle,
                        Decision::Stop {
                            keep,
                            reason: StopReason::Alone,
                        },
                    )
                } else if now_ms - last_speech_ms >= ms(self.rules.quiet_stop) {
                    let next = if earshot.is_empty() {
                        Phase::Idle
                    } else {
                        Phase::Resting {
                            group: earshot.clone(),
                        }
                    };
                    (
                        next,
                        Decision::Stop {
                            keep,
                            reason: StopReason::Quiet,
                        },
                    )
                } else {
                    (
                        Phase::Recording {
                            started_ms,
                            last_company_ms,
                            last_speech_ms,
                            speech_ms,
                        },
                        Decision::Wait,
                    )
                }
            }
            Phase::Resting { .. } if earshot.is_empty() => (Phase::Idle, Decision::Wait),
            Phase::Resting { group } if &group != earshot => {
                self.settling(earshot.clone(), now_ms, heard, now_ms)
            }
            Phase::Resting { group } if heard == 0 => (Phase::Resting { group }, Decision::Wait),
            Phase::Resting { .. } => {
                let from_ms = now_ms - RESUME_LEAD_MS;
                (
                    recording(from_ms, now_ms, heard),
                    Decision::Start { from_ms },
                )
            }
        };
        self.phase = next;
        decision
    }

    pub fn recording(&self) -> bool {
        matches!(self.phase, Phase::Recording { .. } | Phase::Manual { .. })
    }

    pub fn start_now(&mut self, now_ms: i64) -> Decision {
        if self.recording() {
            return Decision::Wait;
        }
        self.phase = Phase::Manual { started_ms: now_ms };
        Decision::Start { from_ms: now_ms }
    }

    pub fn stop_now(&mut self, earshot: &Group) -> Decision {
        let keep = match &self.phase {
            Phase::Recording { speech_ms, .. } => *speech_ms >= ms(self.rules.min_speech),
            Phase::Manual { .. } => true,
            _ => return Decision::Wait,
        };
        self.phase = if earshot.is_empty() {
            Phase::Idle
        } else {
            Phase::Held {
                group: earshot.clone(),
            }
        };
        Decision::Stop {
            keep,
            reason: StopReason::Manual,
        }
    }

    pub fn end(&mut self) -> Decision {
        let decision = match &self.phase {
            Phase::Recording { speech_ms, .. } => Decision::Stop {
                keep: *speech_ms >= ms(self.rules.min_speech),
                reason: StopReason::Ended,
            },
            Phase::Manual { .. } => Decision::Stop {
                keep: true,
                reason: StopReason::Ended,
            },
            _ => Decision::Wait,
        };
        self.phase = Phase::Idle;
        decision
    }

    fn settling(
        &self,
        group: Group,
        since_ms: i64,
        speech_ms: i64,
        now_ms: i64,
    ) -> (Phase, Decision) {
        if now_ms - since_ms >= ms(self.rules.settle) {
            (
                recording(since_ms, now_ms, speech_ms),
                Decision::Start {
                    from_ms: since_ms - SEEN_LEAD_MS,
                },
            )
        } else {
            (
                Phase::Settling {
                    group,
                    since_ms,
                    speech_ms,
                },
                Decision::Wait,
            )
        }
    }
}

fn recording(started_ms: i64, now_ms: i64, speech_ms: i64) -> Phase {
    Phase::Recording {
        started_ms,
        last_company_ms: now_ms,
        last_speech_ms: now_ms,
        speech_ms,
    }
}

fn ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(ids: &[&str]) -> Group {
        ids.iter().map(|s| s.to_string()).collect()
    }

    struct Script {
        detector: Detector,
        now_ms: i64,
    }

    impl Script {
        fn new() -> Self {
            Self {
                detector: Detector::new(Rules {
                    auto_start: true,
                    settle: Duration::from_secs(5),
                    alone_stop: Duration::from_secs(5),
                    quiet_stop: Duration::from_secs(60),
                    min_speech: Duration::from_secs(10),
                }),
                now_ms: 1_000_000,
            }
        }

        fn second(&mut self, earshot: &[&str], speech_ms: i64) -> Decision {
            self.now_ms += 1_000;
            self.detector.tick(Moment {
                now_ms: self.now_ms,
                earshot: &group(earshot),
                speech_ms,
            })
        }

        fn seconds(&mut self, n: usize, earshot: &[&str], speech_ms: i64) -> Vec<Decision> {
            (0..n).map(|_| self.second(earshot, speech_ms)).collect()
        }
    }

    fn waits(decisions: &[Decision]) -> bool {
        decisions.iter().all(|d| *d == Decision::Wait)
    }

    #[test]
    fn starts_once_the_same_group_held_for_the_settle_time_and_records_from_when_it_formed() {
        let mut s = Script::new();
        assert_eq!(s.second(&[], 0), Decision::Wait);
        assert_eq!(s.second(&["anna"], 0), Decision::Wait);
        let formed = s.now_ms;
        assert!(waits(&s.seconds(4, &["anna"], 0)));
        assert_eq!(
            s.second(&["anna"], 0),
            Decision::Start {
                from_ms: formed - SEEN_LEAD_MS
            }
        );
    }

    #[test]
    fn a_change_in_the_group_restarts_the_countdown() {
        let mut s = Script::new();
        assert!(waits(&s.seconds(4, &["anna"], 0)));
        assert_eq!(s.second(&["anna", "bob"], 0), Decision::Wait);
        let formed = s.now_ms;
        assert!(waits(&s.seconds(4, &["anna", "bob"], 0)));
        assert_eq!(
            s.second(&["anna", "bob"], 0),
            Decision::Start {
                from_ms: formed - SEEN_LEAD_MS
            }
        );
    }

    #[test]
    fn someone_walking_past_never_starts_a_recording() {
        let mut s = Script::new();
        assert!(waits(&s.seconds(3, &["anna"], 500)));
        assert!(waits(&s.seconds(10, &[], 0)));
        assert_eq!(s.detector.phase(), &Phase::Idle);
    }

    #[test]
    fn the_settle_time_is_configurable() {
        let mut s = Script::new();
        s.detector.set_rules(Rules {
            settle: Duration::from_secs(2),
            ..s.detector.rules()
        });
        let formed = s.now_ms + 1_000;
        assert_eq!(s.second(&["anna"], 0), Decision::Wait);
        assert_eq!(s.second(&["anna"], 0), Decision::Wait);
        assert_eq!(
            s.second(&["anna"], 0),
            Decision::Start {
                from_ms: formed - SEEN_LEAD_MS
            }
        );
        assert_eq!(s.detector.settle_left_ms(s.now_ms), None);
    }

    #[test]
    fn reports_how_long_is_left_to_settle() {
        let mut s = Script::new();
        s.seconds(3, &["anna"], 0);
        assert_eq!(s.detector.settle_left_ms(s.now_ms), Some(3_000));
    }

    #[test]
    fn people_joining_or_leaving_during_a_recording_keep_it_going() {
        let mut s = Script::new();
        s.seconds(6, &["anna"], 0);
        assert!(matches!(s.detector.phase(), Phase::Recording { .. }));
        assert!(waits(&s.seconds(10, &["anna", "bob"], 1_000)));
        assert!(waits(&s.seconds(10, &["bob"], 1_000)));
        assert!(matches!(s.detector.phase(), Phase::Recording { .. }));
    }

    #[test]
    fn stops_when_alone_and_keeps_it_only_with_enough_speech() {
        let mut s = Script::new();
        s.seconds(6, &["anna"], 0);
        s.seconds(12, &["anna"], 1_000);
        assert!(waits(&s.seconds(4, &[], 0)));
        assert_eq!(
            s.second(&[], 0),
            Decision::Stop {
                keep: true,
                reason: StopReason::Alone
            }
        );
        assert_eq!(s.detector.phase(), &Phase::Idle);

        let mut s = Script::new();
        s.seconds(6, &["anna"], 0);
        s.seconds(3, &["anna"], 1_000);
        s.seconds(4, &[], 0);
        assert_eq!(
            s.second(&[], 0),
            Decision::Stop {
                keep: false,
                reason: StopReason::Alone
            }
        );
    }

    #[test]
    fn speech_while_settling_counts_towards_keeping_the_recording() {
        let mut s = Script::new();
        s.seconds(6, &["anna"], 1_000);
        s.seconds(4, &["anna"], 1_000);
        s.seconds(5, &[], 0);
        assert_eq!(
            s.detector.end(),
            Decision::Wait,
            "already stopped by being alone"
        );
        let mut s = Script::new();
        s.seconds(6, &["anna"], 1_000);
        s.seconds(4, &["anna"], 1_000);
        assert_eq!(
            s.detector.end(),
            Decision::Stop {
                keep: true,
                reason: StopReason::Ended
            }
        );
    }

    #[test]
    fn a_long_silence_stops_and_the_same_group_speaking_again_resumes_at_once() {
        let mut s = Script::new();
        s.seconds(6, &["anna"], 0);
        s.seconds(15, &["anna"], 1_000);
        let decisions = s.seconds(60, &["anna"], 0);
        assert_eq!(
            decisions.last(),
            Some(&Decision::Stop {
                keep: true,
                reason: StopReason::Quiet
            })
        );
        assert!(waits(&s.seconds(30, &["anna"], 0)));
        assert_eq!(
            s.second(&["anna"], 400),
            Decision::Start {
                from_ms: s.now_ms - 2_000
            }
        );
    }

    #[test]
    fn after_a_long_silence_a_different_group_settles_again() {
        let mut s = Script::new();
        s.seconds(6, &["anna"], 0);
        s.seconds(60, &["anna"], 0);
        assert!(matches!(s.detector.phase(), Phase::Resting { .. }));
        assert_eq!(s.second(&["anna", "bob"], 400), Decision::Wait);
        assert!(matches!(s.detector.phase(), Phase::Settling { .. }));
    }

    #[test]
    fn without_auto_start_nothing_starts_by_itself() {
        let mut s = Script::new();
        s.detector.set_rules(Rules {
            auto_start: false,
            ..s.detector.rules()
        });
        assert!(waits(&s.seconds(30, &["anna"], 1_000)));
        assert_eq!(s.detector.phase(), &Phase::Idle);
    }

    #[test]
    fn turning_auto_start_off_while_waiting_cancels_the_countdown() {
        let mut s = Script::new();
        s.seconds(3, &["anna"], 0);
        s.detector.set_rules(Rules {
            auto_start: false,
            ..s.detector.rules()
        });
        assert!(waits(&s.seconds(10, &["anna"], 0)));
        assert_eq!(s.detector.phase(), &Phase::Idle);
    }

    #[test]
    fn a_manual_recording_starts_at_once_and_runs_until_stopped() {
        let mut s = Script::new();
        s.detector.set_rules(Rules {
            auto_start: false,
            ..s.detector.rules()
        });
        assert_eq!(
            s.detector.start_now(s.now_ms),
            Decision::Start { from_ms: s.now_ms }
        );
        assert!(
            waits(&s.seconds(300, &[], 0)),
            "alone or silent, it goes on"
        );
        assert_eq!(
            s.detector.stop_now(&group(&[])),
            Decision::Stop {
                keep: true,
                reason: StopReason::Manual
            }
        );
        assert_eq!(s.detector.phase(), &Phase::Idle);
        assert_eq!(s.detector.stop_now(&group(&[])), Decision::Wait);
    }

    #[test]
    fn starting_by_hand_during_a_recording_changes_nothing() {
        let mut s = Script::new();
        s.seconds(6, &["anna"], 0);
        assert_eq!(s.detector.start_now(s.now_ms), Decision::Wait);
        assert!(matches!(s.detector.phase(), Phase::Recording { .. }));
    }

    #[test]
    fn after_a_stop_by_hand_the_same_people_do_not_start_it_again() {
        let mut s = Script::new();
        s.seconds(6, &["anna"], 1_000);
        assert_eq!(
            s.detector.stop_now(&group(&["anna"])),
            Decision::Stop {
                keep: false,
                reason: StopReason::Manual
            },
            "an automatic recording stopped by hand still needs enough speech"
        );
        assert!(waits(&s.seconds(60, &["anna"], 1_000)));
        assert_eq!(s.second(&["anna", "bob"], 0), Decision::Wait);
        assert!(matches!(s.detector.phase(), Phase::Settling { .. }));
    }

    #[test]
    fn ending_a_manual_recording_keeps_it() {
        let mut s = Script::new();
        s.detector.start_now(s.now_ms);
        assert_eq!(
            s.detector.end(),
            Decision::Stop {
                keep: true,
                reason: StopReason::Ended
            }
        );
    }

    #[test]
    fn ending_outside_a_recording_does_nothing() {
        let mut s = Script::new();
        s.seconds(2, &["anna"], 0);
        assert_eq!(s.detector.end(), Decision::Wait);
        assert_eq!(s.detector.phase(), &Phase::Idle);
    }
}
