use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageEvent {
    OfficeFrame(String),
    Reloaded,
}

pub type PageEvents = mpsc::UnboundedReceiver<PageEvent>;

pub trait DevTools: Send + Sync {
    fn attach(&self) -> BoxFuture<'_, anyhow::Result<PageEvents>>;
    fn detach(&self);
    fn evaluate<'a>(&'a self, expression: &'a str) -> BoxFuture<'a, anyhow::Result<Value>>;
    fn bind_office_socket(&self) -> BoxFuture<'_, anyhow::Result<bool>>;
}
