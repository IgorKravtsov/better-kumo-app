use std::sync::Arc;

use futures_util::future::BoxFuture;
use kumo_meetings::{MeetingPage, Meetings, MeetingsDeps, Module, ProcessTranscriber};
use serde_json::Value;

use crate::Config;
use crate::desktop::Desktop;

struct DesktopPage(Arc<Desktop>);

impl MeetingPage for DesktopPage {
    fn evaluate<'a>(&'a self, expression: &'a str) -> BoxFuture<'a, anyhow::Result<Value>> {
        Box::pin(self.0.evaluate(expression))
    }

    fn announce<'a>(&'a self, text: &'a str) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(self.0.send_nearby(text))
    }
}

pub fn for_desktop(config: &Config, desktop: &Arc<Desktop>) -> Arc<Meetings> {
    Meetings::new(MeetingsDeps {
        page: Arc::new(DesktopPage(desktop.clone())),
        config: config.meetings.clone(),
        models_dir: config.models_dir(),
        module: Module::new(
            config.data_dir().join("meetings-module"),
            config.meetings.transcriber.clone(),
        ),
        transcriber: Arc::new(ProcessTranscriber),
    })
}
