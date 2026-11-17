use std::sync::{Arc, Mutex, Weak};

use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::machine::{CallMachine, Effect, Input};
use super::ports::{CallPorts, MediaSink};
use super::types::{CallSource, CallView};

pub struct CallSession {
    driver: Arc<Driver>,
    pump: JoinHandle<()>,
}

struct Driver {
    source: CallSource,
    machine: Mutex<CallMachine>,
    view: Mutex<CallView>,
    ports: CallPorts,
    inputs: mpsc::UnboundedSender<Input>,
    runtime: Handle,
}

impl CallSession {
    pub fn start(source: CallSource, ports: CallPorts, runtime: Handle) -> Self {
        let (inputs, rx) = mpsc::unbounded_channel();
        let driver = Arc::new(Driver {
            source,
            machine: Mutex::new(CallMachine::new(source)),
            view: Mutex::new(CallView::idle()),
            ports,
            inputs,
            runtime: runtime.clone(),
        });
        let pump = runtime.spawn(pump(Arc::downgrade(&driver), rx));
        driver.start();
        if source == CallSource::Office {
            driver.office_changed();
        }
        Self { driver, pump }
    }

    pub fn source(&self) -> CallSource {
        self.driver.source
    }

    pub fn is_live(&self) -> bool {
        self.driver.with_machine(|m| m.is_live())
    }

    pub fn view(&self) -> CallView {
        self.driver
            .view
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    pub fn leave(&self) {
        self.driver.feed(Input::Leave);
    }

    pub fn abandon(&self) {
        self.driver.feed(Input::Abandon);
    }

    pub fn set_mic(&self, on: bool) {
        self.driver.feed(Input::SetMic(on));
    }

    pub fn set_camera(&self, on: bool) {
        self.driver.feed(Input::SetCamera(on));
    }

    pub fn office_changed(&self) {
        self.driver.office_changed();
    }
}

impl Drop for CallSession {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

impl Driver {
    fn with_machine<T: Default>(&self, f: impl FnOnce(&CallMachine) -> T) -> T {
        self.machine.lock().map(|m| f(&m)).unwrap_or_default()
    }

    fn start(&self) {
        self.apply(|m| m.start());
    }

    fn office_changed(&self) {
        let credentials = self.ports.office.credentials();
        let zone = self.ports.office.zone();
        self.feed(Input::Zone(zone));
        self.feed(Input::Credentials(credentials));
    }

    fn feed(&self, input: Input) {
        self.apply(|m| m.step(input));
    }

    fn apply(&self, step: impl FnOnce(&mut CallMachine) -> Vec<Effect>) {
        let changed = {
            let Ok(mut machine) = self.machine.lock() else {
                return;
            };
            for effect in step(&mut machine) {
                self.run(effect);
            }
            let view = machine.view();
            let Ok(mut current) = self.view.lock() else {
                return;
            };
            let changed = *current != view;
            *current = view;
            changed
        };
        if changed {
            self.ports.changes.call_changed();
        }
    }

    fn run(&self, effect: Effect) {
        let ports = &self.ports;
        match effect {
            Effect::Join(request) => {
                let sink = MediaSink::new(request.attempt, self.inputs.clone());
                ports.media.join(request, sink);
            }
            Effect::Leave { attempt } => ports.media.leave(attempt),
            Effect::SetInputs {
                attempt,
                mic,
                camera,
            } => ports.media.set_inputs(attempt, mic, camera),
            Effect::Subscribe {
                attempt,
                participant_ids,
            } => ports.media.subscribe(attempt, participant_ids),
            Effect::RefreshToken => ports.office.refresh_token(),
            Effect::LeaveOffice => ports.office.leave(),
            Effect::Handoff => {
                let handoff = ports.relay.handoff();
                self.reply(async move { Some(Input::Granted(handoff.await)) });
            }
            Effect::Renew { lease_id } => {
                let renew = ports.relay.renew(lease_id.clone());
                self.reply(async move {
                    Some(Input::Renewed {
                        lease_id,
                        outcome: renew.await,
                    })
                });
            }
            Effect::Reclaim { lease_id } => {
                let reclaim = ports.relay.reclaim(lease_id);
                self.reply(async move {
                    reclaim.await;
                    None
                });
            }
            Effect::FetchDesktop => {
                let zone = ports.relay.zone();
                self.reply(async move { Some(Input::DesktopPolled(zone.await)) });
            }
            Effect::Schedule { after, timer } => {
                self.reply(async move {
                    tokio::time::sleep(after).await;
                    Some(Input::Timer(timer))
                });
            }
        }
    }

    fn reply(&self, work: impl Future<Output = Option<Input>> + Send + 'static) {
        let inputs = self.inputs.clone();
        self.runtime.spawn(async move {
            if let Some(input) = work.await {
                let _ = inputs.send(input);
            }
        });
    }
}

async fn pump(driver: Weak<Driver>, mut inputs: mpsc::UnboundedReceiver<Input>) {
    while let Some(input) = inputs.recv().await {
        let Some(driver) = driver.upgrade() else {
            return;
        };
        driver.feed(input);
    }
}
