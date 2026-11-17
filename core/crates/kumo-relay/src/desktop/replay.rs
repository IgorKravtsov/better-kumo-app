use std::sync::Mutex;

use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc;

use super::dev_tools::{DevTools, PageEvent, PageEvents};

struct Answer {
    prefix: String,
    reply: Value,
    frames: Vec<String>,
}

#[derive(Default)]
pub struct Replay {
    answers: Mutex<Vec<Answer>>,
    evaluated: Mutex<Vec<String>>,
    page: Mutex<Option<mpsc::UnboundedSender<PageEvent>>>,
    queued: Mutex<Vec<PageEvent>>,
}

impl Replay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn answer(&self, prefix: &str, reply: Value) {
        self.answer_with_frames(prefix, reply, &[]);
    }

    pub fn answer_with_frames(&self, prefix: &str, reply: Value, frames: &[&str]) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.push(Answer {
                prefix: prefix.to_string(),
                reply,
                frames: frames.iter().map(|f| f.to_string()).collect(),
            });
        }
    }

    pub fn frame(&self, payload: &str) {
        self.emit(PageEvent::OfficeFrame(payload.to_string()));
    }

    pub fn recording(&self, lines: &str) {
        for line in lines.lines().map(str::trim).filter(|l| !l.is_empty()) {
            self.frame(line);
        }
    }

    pub fn reload(&self) {
        self.emit(PageEvent::Reloaded);
    }

    pub fn evaluated(&self) -> Vec<String> {
        self.evaluated.lock().map(|e| e.clone()).unwrap_or_default()
    }

    fn emit(&self, event: PageEvent) {
        let sent = self
            .page
            .lock()
            .ok()
            .and_then(|p| p.clone())
            .is_some_and(|tx| tx.send(event.clone()).is_ok());
        if !sent && let Ok(mut queued) = self.queued.lock() {
            queued.push(event);
        }
    }
}

impl DevTools for Replay {
    fn attach(&self) -> BoxFuture<'_, anyhow::Result<PageEvents>> {
        Box::pin(async move {
            let (tx, rx) = mpsc::unbounded_channel();
            for event in self
                .queued
                .lock()
                .map(|mut q| std::mem::take(&mut *q))
                .unwrap_or_default()
            {
                let _ = tx.send(event);
            }
            if let Ok(mut page) = self.page.lock() {
                *page = Some(tx);
            }
            Ok(rx)
        })
    }

    fn detach(&self) {
        if let Ok(mut page) = self.page.lock() {
            *page = None;
        }
    }

    fn evaluate<'a>(&'a self, expression: &'a str) -> BoxFuture<'a, anyhow::Result<Value>> {
        Box::pin(async move {
            if let Ok(mut evaluated) = self.evaluated.lock() {
                evaluated.push(expression.to_string());
            }
            let (reply, frames) = self
                .answers
                .lock()
                .ok()
                .and_then(|answers| {
                    answers
                        .iter()
                        .rev()
                        .find(|a| expression.starts_with(&a.prefix))
                        .map(|a| (a.reply.clone(), a.frames.clone()))
                })
                .unwrap_or_default();
            for frame in frames {
                self.frame(&frame);
            }
            Ok(reply)
        })
    }

    fn bind_office_socket(&self) -> BoxFuture<'_, anyhow::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
