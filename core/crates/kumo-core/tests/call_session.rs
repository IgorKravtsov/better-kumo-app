use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use kumo_core::app_session::AppSession;
use kumo_core::call::{
    AudioLevel, CallChanges, CallFailure, CallPhase, CallPorts, CallSource, CallTile, CallView,
    CallZone, Credentials, Grant, HandoffFailure, Mate, MediaJoin, MediaParticipant, MediaPort,
    MediaSink, OfficeCallSource, RelayCalls, RenewOutcome,
};
use kumo_core::desktop::Presence;

type Log = Arc<Mutex<Vec<String>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum MediaCommand {
    Join(MediaJoin),
    Leave(u64),
    SetInputs {
        attempt: u64,
        mic: bool,
        camera: bool,
    },
    Subscribe(u64, Vec<String>),
}

struct FakeMedia {
    log: Log,
    commands: Mutex<Vec<MediaCommand>>,
    sinks: Mutex<HashMap<u64, MediaSink>>,
}

impl FakeMedia {
    fn record(&self, command: MediaCommand) {
        self.log.lock().unwrap().push(format!("media {command:?}"));
        self.commands.lock().unwrap().push(command);
    }

    fn commands(&self) -> Vec<MediaCommand> {
        self.commands.lock().unwrap().clone()
    }

    fn last(&self) -> Option<MediaCommand> {
        self.commands().last().cloned()
    }

    fn sink(&self, attempt: u64) -> MediaSink {
        self.sinks.lock().unwrap()[&attempt].clone()
    }

    fn joins(&self) -> Vec<MediaJoin> {
        self.commands()
            .into_iter()
            .filter_map(|c| match c {
                MediaCommand::Join(j) => Some(j),
                _ => None,
            })
            .collect()
    }
}

impl MediaPort for FakeMedia {
    fn join(&self, request: MediaJoin, sink: MediaSink) {
        self.sinks.lock().unwrap().insert(request.attempt, sink);
        self.record(MediaCommand::Join(request));
    }

    fn leave(&self, attempt: u64) {
        self.record(MediaCommand::Leave(attempt));
    }

    fn set_inputs(&self, attempt: u64, mic: bool, camera: bool) {
        self.record(MediaCommand::SetInputs {
            attempt,
            mic,
            camera,
        });
    }

    fn subscribe(&self, attempt: u64, participant_ids: Vec<String>) {
        self.record(MediaCommand::Subscribe(attempt, participant_ids));
    }
}

struct FakeOffice {
    log: Log,
    credentials: Mutex<Option<Credentials>>,
    zone: Mutex<CallZone>,
    refreshes: AtomicUsize,
}

impl FakeOffice {
    fn set_credentials(&self, url: &str, token: &str) {
        *self.credentials.lock().unwrap() = Some(credentials(url, token));
    }

    fn refreshes(&self) -> usize {
        self.refreshes.load(Ordering::SeqCst)
    }
}

impl OfficeCallSource for FakeOffice {
    fn credentials(&self) -> Option<Credentials> {
        self.credentials.lock().unwrap().clone()
    }

    fn zone(&self) -> CallZone {
        self.zone.lock().unwrap().clone()
    }

    fn refresh_token(&self) {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.log.lock().unwrap().push("office refresh".into());
    }

    fn leave(&self) {
        self.log.lock().unwrap().push("office leave".into());
    }
}

#[derive(Default)]
struct Leases {
    issued: u32,
    current: Option<String>,
}

struct FakeRelay {
    log: Log,
    leases: Mutex<Leases>,
    unreachable: AtomicBool,
    not_in_call: AtomicBool,
    renewals: AtomicUsize,
    reclaims: Mutex<Vec<String>>,
    zone: Mutex<Option<CallZone>>,
}

impl FakeRelay {
    fn second_phone_hands_off(&self) {
        self.leases.lock().unwrap().current = Some("other-phone".into());
    }

    fn renewals(&self) -> usize {
        self.renewals.load(Ordering::SeqCst)
    }

    fn reclaims(&self) -> Vec<String> {
        self.reclaims.lock().unwrap().clone()
    }
}

impl RelayCalls for FakeRelay {
    fn handoff(&self) -> BoxFuture<'static, Result<Grant, HandoffFailure>> {
        self.log.lock().unwrap().push("relay handoff".into());
        let result = if self.unreachable.load(Ordering::SeqCst) {
            Err(HandoffFailure::Unreachable)
        } else if self.not_in_call.load(Ordering::SeqCst) {
            Err(HandoffFailure::NotInCall)
        } else {
            let mut leases = self.leases.lock().unwrap();
            leases.issued += 1;
            let lease_id = format!("lease-{}", leases.issued);
            leases.current = Some(lease_id.clone());
            Ok(Grant {
                lease_id,
                credentials: credentials(
                    "https://kumo.daily.co/desk",
                    &format!("desk-token-{}", leases.issued),
                ),
                room_name: "Blue Room".into(),
                peers: vec!["amy".into()],
            })
        };
        Box::pin(async move { result })
    }

    fn renew(&self, lease_id: String) -> BoxFuture<'static, RenewOutcome> {
        self.renewals.fetch_add(1, Ordering::SeqCst);
        let outcome = if self.unreachable.load(Ordering::SeqCst) {
            RenewOutcome::Unreachable
        } else if self.leases.lock().unwrap().current.as_ref() == Some(&lease_id) {
            RenewOutcome::Renewed
        } else {
            RenewOutcome::Stale
        };
        Box::pin(async move { outcome })
    }

    fn reclaim(&self, lease_id: String) -> BoxFuture<'static, ()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("relay reclaim {lease_id}"));
        self.reclaims.lock().unwrap().push(lease_id);
        Box::pin(async {})
    }

    fn zone(&self) -> BoxFuture<'static, Option<CallZone>> {
        let zone = self.zone.lock().unwrap().clone();
        Box::pin(async move { zone })
    }
}

#[derive(Default)]
struct Changes(AtomicUsize);

impl CallChanges for Changes {
    fn call_changed(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Harness {
    app: AppSession,
    log: Log,
    media: Arc<FakeMedia>,
    office: Arc<FakeOffice>,
    relay: Arc<FakeRelay>,
    changes: Arc<Changes>,
}

impl Harness {
    fn new() -> Self {
        let log: Log = Arc::default();
        let media = Arc::new(FakeMedia {
            log: log.clone(),
            commands: Mutex::default(),
            sinks: Mutex::default(),
        });
        let office = Arc::new(FakeOffice {
            log: log.clone(),
            credentials: Mutex::new(Some(credentials("https://kumo.daily.co/blue", "t1"))),
            zone: Mutex::new(office_zone()),
            refreshes: AtomicUsize::new(0),
        });
        let relay = Arc::new(FakeRelay {
            log: log.clone(),
            leases: Mutex::default(),
            unreachable: AtomicBool::new(false),
            not_in_call: AtomicBool::new(false),
            renewals: AtomicUsize::new(0),
            reclaims: Mutex::default(),
            zone: Mutex::new(Some(desktop_zone())),
        });
        let changes = Arc::new(Changes::default());
        let app = AppSession::new(
            CallPorts {
                media: media.clone(),
                office: office.clone(),
                relay: relay.clone(),
                changes: changes.clone(),
            },
            tokio::runtime::Handle::current(),
        );
        Self {
            app,
            log,
            media,
            office,
            relay,
            changes,
        }
    }

    fn view(&self) -> CallView {
        self.app.call()
    }

    fn phase(&self) -> CallPhase {
        self.view().phase
    }

    fn changes(&self) -> usize {
        self.changes.0.load(Ordering::SeqCst)
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    async fn joined_office(&self) {
        self.app.join_office();
        settle().await;
        let sink = self.media.sink(1);
        sink.joined();
        sink.participants(vec![
            participant("p0", "me", true),
            participant("p1", "amy", false),
            participant("p9", "stranger", false),
        ]);
        settle().await;
        assert_eq!(self.phase(), CallPhase::Joined);
    }

    async fn joined_desktop(&self) {
        self.app.handoff();
        settle().await;
        let sink = self.media.sink(1);
        sink.joined();
        sink.participants(vec![
            participant("p0", "me", true),
            participant("p1", "amy", false),
        ]);
        settle().await;
        assert_eq!(self.phase(), CallPhase::Joined);
    }

    async fn fail_join(&self, attempt: u64) {
        self.media
            .sink(attempt)
            .join_failed("exp-token".into(), "meeting token expired".into());
        settle().await;
    }
}

fn credentials(url: &str, token: &str) -> Credentials {
    Credentials {
        url: url.into(),
        token: token.into(),
    }
}

fn mate(key: &str, name: &str) -> Mate {
    Mate {
        key: key.into(),
        name: name.into(),
        avatar: format!("https://img/{key}.png"),
        presence: Presence::Available,
    }
}

fn office_zone() -> CallZone {
    CallZone {
        room_name: "Blue Room".into(),
        me: Some(mate("me", "Me")),
        mates: vec![mate("amy", "Amy"), mate("zed", "Zed")],
    }
}

fn desktop_zone() -> CallZone {
    CallZone {
        room_name: "Blue Room".into(),
        me: Some(mate("me", "Me")),
        mates: vec![mate("amy", "Amy"), mate("bob", "Bob")],
    }
}

fn participant(id: &str, user_id: &str, is_local: bool) -> MediaParticipant {
    MediaParticipant {
        id: id.into(),
        user_id: Some(user_id.into()),
        user_name: Some(format!("daily-{user_id}")),
        is_local,
        mic_on: true,
        has_camera: false,
        has_screen: false,
    }
}

fn level(participant_id: &str, level: f32) -> AudioLevel {
    AudioLevel {
        participant_id: participant_id.into(),
        level,
    }
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

async fn wait(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    settle().await;
}

fn tile<'a>(view: &'a CallView, key: &str) -> &'a CallTile {
    view.tiles.iter().find(|t| t.key == key).unwrap()
}

#[tokio::test(start_paused = true)]
async fn office_call_joins_daily_and_subscribes_to_zone_mates_only() {
    let h = Harness::new();
    h.app.join_office();
    settle().await;
    assert_eq!(
        h.media.commands(),
        vec![MediaCommand::Join(MediaJoin {
            attempt: 1,
            url: "https://kumo.daily.co/blue".into(),
            token: "t1".into(),
            mic: false,
            camera: false,
        })]
    );
    let view = h.view();
    assert_eq!(view.phase, CallPhase::Joining);
    assert_eq!(view.source, Some(CallSource::Office));
    assert_eq!(view.room_name, "Blue Room");

    let sink = h.media.sink(1);
    sink.joined();
    sink.participants(vec![
        participant("p0", "me", true),
        participant("p1", "amy", false),
        participant("p9", "stranger", false),
    ]);
    settle().await;
    assert_eq!(h.phase(), CallPhase::Joined);
    assert_eq!(
        h.media.last(),
        Some(MediaCommand::Subscribe(1, vec!["p1".into()]))
    );
    let keys: Vec<String> = h.view().tiles.iter().map(|t| t.key.clone()).collect();
    assert_eq!(keys, ["me", "amy", "zed"]);
}

#[tokio::test(start_paused = true)]
async fn zone_mates_missing_from_daily_are_tiles_not_in_call() {
    let h = Harness::new();
    h.joined_office().await;
    let view = h.view();
    let me = tile(&view, "me");
    assert!(me.is_local && me.in_call);
    assert_eq!(me.name, "Me");
    assert_eq!(me.participant_id.as_deref(), Some("p0"));
    let amy = tile(&view, "amy");
    assert!(amy.in_call);
    assert_eq!(amy.avatar, "https://img/amy.png");
    let zed = tile(&view, "zed");
    assert!(!zed.in_call);
    assert_eq!(zed.participant_id, None);
    assert_eq!(zed.presence, Presence::Available);
    assert!(view.tiles.iter().all(|t| t.key != "stranger"));
}

#[tokio::test(start_paused = true)]
async fn token_errors_back_off_one_two_three_seconds_then_fail() {
    let h = Harness::new();
    h.app.join_office();
    settle().await;
    for (attempt, backoff) in [(1u64, 1000u64), (2, 2000), (3, 3000)] {
        h.fail_join(attempt).await;
        assert_eq!(h.phase(), CallPhase::Waiting);
        assert_eq!(h.media.last(), Some(MediaCommand::Leave(attempt)));
        let refreshes = h.office.refreshes();
        wait(backoff - 1).await;
        assert_eq!(h.office.refreshes(), refreshes);
        wait(1).await;
        assert_eq!(h.office.refreshes(), refreshes + 1);
        h.office
            .set_credentials("https://kumo.daily.co/blue", &format!("t{}", attempt + 1));
        h.app.office_changed();
        settle().await;
        assert_eq!(h.media.joins().len() as u64, attempt + 1);
    }
    h.fail_join(4).await;
    assert_eq!(
        h.phase(),
        CallPhase::Failed {
            reason: CallFailure::TokenRetriesExhausted
        }
    );
    wait(10_000).await;
    assert_eq!(h.office.refreshes(), 3);
    assert_eq!(h.media.joins().len(), 4);
}

#[tokio::test(start_paused = true)]
async fn one_token_error_refreshes_and_joins_with_the_fresh_token() {
    let h = Harness::new();
    h.app.join_office();
    settle().await;
    h.fail_join(1).await;
    h.app.office_changed();
    settle().await;
    assert_eq!(h.media.joins().len(), 1);
    wait(1000).await;
    assert_eq!(h.office.refreshes(), 1);
    h.office.set_credentials("https://kumo.daily.co/blue", "t2");
    h.app.office_changed();
    settle().await;
    let joins = h.media.joins();
    assert_eq!(joins.len(), 2);
    assert_eq!(joins[1].token, "t2");
    h.media.sink(2).joined();
    settle().await;
    assert_eq!(h.phase(), CallPhase::Joined);
}

#[tokio::test(start_paused = true)]
async fn handoff_renews_the_lease_and_a_second_phone_takes_over_without_reclaim() {
    let h = Harness::new();
    h.joined_desktop().await;
    let view = h.view();
    assert_eq!(view.source, Some(CallSource::Desktop));
    assert_eq!(h.media.joins()[0].url, "https://kumo.daily.co/desk");
    wait(9_999).await;
    assert_eq!(h.relay.renewals(), 0);
    wait(1).await;
    assert_eq!(h.relay.renewals(), 1);
    wait(10_000).await;
    assert_eq!(h.relay.renewals(), 2);
    assert_eq!(h.phase(), CallPhase::Joined);

    h.relay.second_phone_hands_off();
    wait(10_000).await;
    assert_eq!(
        h.phase(),
        CallPhase::Failed {
            reason: CallFailure::TakenOver
        }
    );
    assert_eq!(h.media.last(), Some(MediaCommand::Leave(1)));
    assert!(h.relay.reclaims().is_empty());
    wait(30_000).await;
    assert_eq!(h.relay.renewals(), 3);
    h.app.leave();
    settle().await;
    assert!(h.relay.reclaims().is_empty());
    assert_eq!(h.phase(), CallPhase::Idle);
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_relay_for_one_tick_keeps_the_call() {
    let h = Harness::new();
    h.joined_desktop().await;
    h.relay.unreachable.store(true, Ordering::SeqCst);
    wait(10_000).await;
    assert_eq!(h.relay.renewals(), 1);
    assert_eq!(h.phase(), CallPhase::Joined);
    h.relay.unreachable.store(false, Ordering::SeqCst);
    wait(10_000).await;
    assert_eq!(h.relay.renewals(), 2);
    assert_eq!(h.phase(), CallPhase::Joined);
}

#[tokio::test(start_paused = true)]
async fn leaving_a_desktop_call_reclaims_it_once() {
    let h = Harness::new();
    h.joined_desktop().await;
    h.app.leave();
    settle().await;
    assert_eq!(h.relay.reclaims(), ["lease-1"]);
    assert_eq!(h.media.last(), Some(MediaCommand::Leave(1)));
    assert_eq!(h.view(), CallView::idle());
    h.app.leave();
    settle().await;
    assert_eq!(h.relay.reclaims(), ["lease-1"]);
    wait(30_000).await;
    assert_eq!(h.relay.renewals(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_desktop_without_a_call_fails_the_handoff() {
    let h = Harness::new();
    h.relay.not_in_call.store(true, Ordering::SeqCst);
    h.app.handoff();
    settle().await;
    assert_eq!(
        h.phase(),
        CallPhase::Failed {
            reason: CallFailure::DesktopNotInCall
        }
    );
    assert!(h.media.commands().is_empty());
}

#[tokio::test(start_paused = true)]
async fn desktop_zone_mates_come_from_polling_the_relay() {
    let h = Harness::new();
    h.joined_desktop().await;
    let keys: Vec<String> = h.view().tiles.iter().map(|t| t.key.clone()).collect();
    assert_eq!(keys, ["me", "amy", "bob"]);
    assert!(!tile(&h.view(), "bob").in_call);
    *h.relay.zone.lock().unwrap() = Some(CallZone {
        room_name: "Green Room".into(),
        me: Some(mate("me", "Me")),
        mates: vec![mate("amy", "Amy")],
    });
    wait(3_000).await;
    let view = h.view();
    assert_eq!(view.room_name, "Green Room");
    assert_eq!(view.tiles.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn events_from_a_stale_attempt_are_ignored() {
    let h = Harness::new();
    h.app.join_office();
    settle().await;
    h.office
        .set_credentials("https://kumo.daily.co/green", "t9");
    h.app.office_changed();
    settle().await;
    assert_eq!(
        h.media.commands()[1..].to_vec(),
        vec![
            MediaCommand::Leave(1),
            MediaCommand::Join(MediaJoin {
                attempt: 2,
                url: "https://kumo.daily.co/green".into(),
                token: "t9".into(),
                mic: false,
                camera: false,
            }),
        ]
    );
    let stale = h.media.sink(1);
    stale.joined();
    stale.participants(vec![participant("p1", "amy", false)]);
    stale.left();
    settle().await;
    assert_eq!(h.phase(), CallPhase::Joining);
    assert_eq!(h.media.commands().len(), 3);
    h.media.sink(2).joined();
    settle().await;
    assert_eq!(h.phase(), CallPhase::Joined);
}

#[tokio::test(start_paused = true)]
async fn speaking_needs_the_threshold_and_local_speech_needs_the_mic() {
    let h = Harness::new();
    h.joined_office().await;
    let sink = h.media.sink(1);
    sink.audio_levels(vec![level("p1", 0.02), level("p0", 0.5)]);
    settle().await;
    let view = h.view();
    assert!(view.tiles.iter().all(|t| !t.speaking));
    assert_eq!(view.active_speaker, None);

    sink.audio_levels(vec![level("p1", 0.05)]);
    settle().await;
    let view = h.view();
    assert!(tile(&view, "amy").speaking);
    assert!(!tile(&view, "me").speaking);
    assert_eq!(view.active_speaker.as_deref(), Some("amy"));

    h.app.set_mic(true);
    settle().await;
    assert_eq!(
        h.media.last(),
        Some(MediaCommand::SetInputs {
            attempt: 1,
            mic: true,
            camera: false
        })
    );
    let view = h.view();
    assert!(view.mic_on);
    assert!(tile(&view, "me").speaking);

    sink.audio_levels(vec![level("p1", 0.0)]);
    settle().await;
    let view = h.view();
    assert!(!tile(&view, "amy").speaking);
    assert_eq!(view.active_speaker.as_deref(), Some("amy"));
}

#[tokio::test(start_paused = true)]
async fn audio_levels_that_change_nothing_do_not_emit() {
    let h = Harness::new();
    h.joined_office().await;
    let sink = h.media.sink(1);
    let before = h.changes();
    sink.audio_levels(vec![level("p1", 0.01), level("p0", 0.3)]);
    settle().await;
    assert_eq!(h.changes(), before);
    sink.audio_levels(vec![level("p1", 0.05)]);
    settle().await;
    assert_eq!(h.changes(), before + 1);
    sink.audio_levels(vec![level("p1", 0.07)]);
    sink.audio_levels(vec![level("p1", 0.04)]);
    settle().await;
    assert_eq!(h.changes(), before + 1);
}

#[tokio::test(start_paused = true)]
async fn switching_from_office_to_desktop_leaves_the_office_first() {
    let h = Harness::new();
    h.joined_office().await;
    h.app.handoff();
    settle().await;
    let log = h.log();
    let leave = log.iter().position(|l| l == "office leave").unwrap();
    let media_leave = log.iter().position(|l| l == "media Leave(1)").unwrap();
    let handoff = log.iter().position(|l| l == "relay handoff").unwrap();
    assert!(media_leave < handoff);
    assert!(leave < handoff);
    assert_eq!(h.view().source, Some(CallSource::Desktop));
    assert_eq!(h.phase(), CallPhase::Joining);
}

#[tokio::test(start_paused = true)]
async fn switching_from_desktop_to_office_reclaims_first() {
    let h = Harness::new();
    h.joined_desktop().await;
    h.app.join_office();
    settle().await;
    assert_eq!(h.relay.reclaims(), ["lease-1"]);
    assert_eq!(h.view().source, Some(CallSource::Office));
    assert_eq!(h.media.joins().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn an_office_join_that_is_refused_ends_the_waiting_call() {
    let h = Harness::new();
    *h.office.credentials.lock().unwrap() = None;
    h.app.join_office();
    settle().await;
    assert_eq!(h.phase(), CallPhase::Waiting);
    h.app.office_rejected();
    settle().await;
    assert_eq!(h.phase(), CallPhase::Idle);
    assert!(!h.log().contains(&"office leave".to_string()));
}

#[tokio::test(start_paused = true)]
async fn failed_stays_until_the_next_join() {
    let h = Harness::new();
    h.relay.not_in_call.store(true, Ordering::SeqCst);
    h.app.handoff();
    settle().await;
    wait(60_000).await;
    assert!(matches!(h.phase(), CallPhase::Failed { .. }));
    h.relay.not_in_call.store(false, Ordering::SeqCst);
    h.app.handoff();
    settle().await;
    assert_eq!(h.phase(), CallPhase::Joining);
}

#[tokio::test(start_paused = true)]
async fn an_unexpected_left_while_joined_retries_with_a_fresh_token() {
    let h = Harness::new();
    h.joined_office().await;
    h.media.sink(1).left();
    settle().await;
    assert_eq!(h.phase(), CallPhase::Waiting);
    assert_eq!(h.media.last(), Some(MediaCommand::Leave(1)));
    wait(1000).await;
    assert_eq!(h.office.refreshes(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_desktop_retry_hands_off_again_and_renews_the_new_lease() {
    let h = Harness::new();
    h.joined_desktop().await;
    h.media.sink(1).error("Meeting token is invalid".into());
    settle().await;
    assert_eq!(h.phase(), CallPhase::Waiting);
    wait(1000).await;
    let joins = h.media.joins();
    assert_eq!(joins.len(), 2);
    assert_eq!(joins[1].token, "desk-token-2");
    h.media.sink(2).joined();
    settle().await;
    assert_eq!(h.phase(), CallPhase::Joined);
    wait(10_000).await;
    assert_eq!(h.phase(), CallPhase::Joined);
    h.app.leave();
    settle().await;
    assert_eq!(h.relay.reclaims(), ["lease-2"]);
}
