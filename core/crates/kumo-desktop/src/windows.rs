//! Better Kumo's own windows, the Setup guide and Settings: tao windows with the system WebView
//! (WKWebView, WebView2, WebKitGTK through wry). The guide is built into the app, so it works
//! before the Relay runs; Settings is the Relay's `/settings` page. Pages talk to the app only with
//! `actions::Action` messages, and the app sends them `view::DesktopView` as JSON.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use tao::dpi::LogicalSize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder};

use crate::actions;

const GUIDE: &str = include_str!("guide.html");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Guide,
    Settings,
}

/// Events the event loop gets from the pages.
#[derive(Debug)]
pub enum UserEvent {
    Message { window: WindowId, text: String },
}

struct Open {
    kind: Kind,
    window: Window,
    webview: WebView,
    /// The last state sent, so an unchanged one is not sent again.
    sent: Option<String>,
}

pub struct Windows {
    open: HashMap<WindowId, Open>,
    context: WebContext,
}

/// The guide with the shared design put in, as the WebView gets it.
pub fn guide_page() -> String {
    GUIDE.replace("/*UI_CSS*/", kumo_relay::UI_CSS)
}

fn settings_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/settings")
}

/// Where WebView keeps its data. On Windows the default is next to the program, which is not
/// writable under Program Files.
fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
        });
    base.join("kumowatch-relay").join("webview")
}

impl Windows {
    pub fn new() -> Self {
        Self {
            open: HashMap::new(),
            context: WebContext::new(Some(data_dir())),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    pub fn kind(&self, id: WindowId) -> Option<Kind> {
        self.open.get(&id).map(|w| w.kind)
    }

    /// Brings the window of `kind` forward, opening it first if needed. `section` picks a part of
    /// Settings.
    pub fn show(
        &mut self,
        kind: Kind,
        port: u16,
        section: Option<&str>,
        target: &EventLoopWindowTarget<UserEvent>,
        proxy: &EventLoopProxy<UserEvent>,
    ) -> anyhow::Result<()> {
        if let Some(open) = self.open.values().find(|w| w.kind == kind) {
            if let Some(section) = section {
                let _ = open
                    .webview
                    .evaluate_script(&format!("location.hash = {:?};", section));
            }
            open.window.set_visible(true);
            open.window.set_focus();
            return Ok(());
        }
        let first = self.open.is_empty();
        let (title, width, height) = match kind {
            Kind::Guide => ("Set up Better Kumo", 880.0, 620.0),
            Kind::Settings => ("Better Kumo Settings", 880.0, 640.0),
        };
        let builder = WindowBuilder::new()
            .with_title(title)
            .with_inner_size(LogicalSize::new(width, height))
            .with_min_inner_size(LogicalSize::new(720.0, 480.0));
        #[cfg(target_os = "macos")]
        let builder = {
            use tao::platform::macos::WindowBuilderExtMacOS;
            builder
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true)
        };
        let window = builder.build(target).context("cannot open a window")?;
        let webview = self.webview(kind, port, section, &window, proxy)?;
        if first {
            // A menu-bar app has no Dock icon; a window needs one to come forward and take focus.
            #[cfg(target_os = "macos")]
            {
                use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
                target.set_activation_policy_at_runtime(ActivationPolicy::Regular);
            }
        }
        window.set_focus();
        self.open.insert(
            window.id(),
            Open {
                kind,
                window,
                webview,
                sent: None,
            },
        );
        Ok(())
    }

    fn webview(
        &mut self,
        kind: Kind,
        port: u16,
        section: Option<&str>,
        window: &Window,
        proxy: &EventLoopProxy<UserEvent>,
    ) -> anyhow::Result<WebView> {
        let id = window.id();
        let proxy = proxy.clone();
        let origin = format!("http://127.0.0.1:{port}/");
        let builder = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_ipc_handler(move |request| {
                let _ = proxy.send_event(UserEvent::Message {
                    window: id,
                    text: request.body().clone(),
                });
            })
            // Only the app's own pages load in its windows.
            .with_navigation_handler(move |url| {
                url.starts_with(&origin) || url.starts_with("about:") || url.starts_with("data:")
            })
            .with_new_window_req_handler(|url, _| {
                if actions::may_open_in_browser(&url) {
                    let _ = open::that(&url);
                }
                NewWindowResponse::Deny
            });
        let builder = match kind {
            Kind::Guide => builder.with_html(guide_page()),
            Kind::Settings => {
                let anchor = section.map(|s| format!("#{s}")).unwrap_or_default();
                builder.with_url(format!("{}{anchor}", settings_url(port)))
            }
        };
        #[cfg(not(target_os = "linux"))]
        let webview = builder.build(window);
        #[cfg(target_os = "linux")]
        let webview = {
            use tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix;
            let vbox = window.default_vbox().context("the window has no GTK box")?;
            builder.build_gtk(vbox)
        };
        webview.context("cannot start the system WebView")
    }

    pub fn close(&mut self, id: WindowId, target: &EventLoopWindowTarget<UserEvent>) {
        self.open.remove(&id);
        if self.open.is_empty() {
            #[cfg(target_os = "macos")]
            {
                use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
                target.set_activation_policy_at_runtime(ActivationPolicy::Accessory);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = target;
        }
    }

    pub fn drag(&self, id: WindowId) {
        if let Some(open) = self.open.get(&id) {
            let _ = open.window.drag_window();
        }
    }

    pub fn zoom(&self, id: WindowId) {
        if let Some(open) = self.open.get(&id) {
            open.window.set_maximized(!open.window.is_maximized());
        }
    }

    /// The next `send` reaches this window even if the state did not change (it just loaded).
    pub fn forget_sent(&mut self, id: WindowId) {
        if let Some(open) = self.open.get_mut(&id) {
            open.sent = None;
        }
    }

    /// Sends the state to every window that has not got this exact one yet.
    pub fn send(&mut self, json: &str) {
        for open in self.open.values_mut() {
            if open.sent.as_deref() == Some(json) {
                continue;
            }
            let script = format!("window.betterKumo && window.betterKumo.update({json});");
            if open.webview.evaluate_script(&script).is_ok() {
                open.sent = Some(json.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_carries_the_shared_design() {
        let page = guide_page();
        assert!(!page.contains("/*UI_CSS*/"));
        assert!(page.contains("--accent"));
    }
}
