use std::sync::{Arc, Mutex};

use tokio::runtime::Handle;

use crate::call::{CallPorts, CallSession, CallSource, CallView};

pub struct AppSession {
    ports: CallPorts,
    runtime: Handle,
    call: Mutex<Option<Arc<CallSession>>>,
}

impl AppSession {
    pub fn new(ports: CallPorts, runtime: Handle) -> Self {
        Self {
            ports,
            runtime,
            call: Mutex::new(None),
        }
    }

    pub fn join_office(&self) {
        self.start(CallSource::Office);
    }

    pub fn handoff(&self) {
        self.start(CallSource::Desktop);
    }

    pub fn leave(&self) {
        if let Some(call) = self.current() {
            call.leave();
        }
    }

    pub fn set_mic(&self, on: bool) {
        if let Some(call) = self.current() {
            call.set_mic(on);
        }
    }

    pub fn set_camera(&self, on: bool) {
        if let Some(call) = self.current() {
            call.set_camera(on);
        }
    }

    pub fn call(&self) -> CallView {
        self.current().map(|c| c.view()).unwrap_or_default()
    }

    pub fn office_changed(&self) {
        if let Some(call) = self.office_call() {
            call.office_changed();
        }
    }

    pub fn office_rejected(&self) {
        if let Some(call) = self.office_call() {
            call.abandon();
        }
    }

    fn office_call(&self) -> Option<Arc<CallSession>> {
        self.current().filter(|c| c.source() == CallSource::Office)
    }

    fn current(&self) -> Option<Arc<CallSession>> {
        self.call.lock().ok().and_then(|c| c.clone())
    }

    fn start(&self, source: CallSource) {
        let previous = self.call.lock().ok().and_then(|mut c| c.take());
        if let Some(previous) = previous {
            let same = previous.source() == source;
            if same && previous.is_live() {
                if let Ok(mut call) = self.call.lock() {
                    *call = Some(previous);
                }
                return;
            }
            if !same {
                previous.leave();
            }
        }
        let call = Arc::new(CallSession::start(
            source,
            self.ports.clone(),
            self.runtime.clone(),
        ));
        if let Ok(mut current) = self.call.lock() {
            *current = Some(call);
        }
    }
}
