//! The guide is built in because onboarding must work before the Relay starts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

#[derive(Debug)]
pub enum UserEvent {
    Message { window: WindowId, text: String },
}

struct Open {
    kind: Kind,
    window: Window,
    webview: WebView,
    loaded: Arc<AtomicBool>,
    sent: Option<String>,
}

pub struct Windows {
    open: HashMap<WindowId, Open>,
    context: WebContext,
}

pub fn guide_page() -> String {
    GUIDE.replace("/*UI_CSS*/", kumo_relay::UI_CSS)
}

fn settings_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/settings")
}

/// Compared parsed: a WebView reports port 80 without `:80`.
fn is_page(page: &str, url: &str) -> bool {
    match (url::Url::parse(page), url::Url::parse(url)) {
        (Ok(page), Ok(url)) => {
            url.scheme() == page.scheme()
                && url.host() == page.host()
                && url.port_or_known_default() == page.port_or_known_default()
                && url.path() == page.path()
        }
        _ => false,
    }
}

/// Only the app's own pages run in its windows, since they can post actions. Settings is the one
/// Relay page; the guide is loaded from a string, whose initial URL differs per WebView (about:,
/// data:), and may load nothing after it reported `ready`.
fn may_navigate(kind: Kind, settings_page: &str, loaded: bool, url: &str) -> bool {
    match kind {
        Kind::Settings => is_page(settings_page, url),
        Kind::Guide => !loaded && (url.starts_with("about:") || url.starts_with("data:")),
    }
}

/// WebView2's default data folder is next to the program, which is not writable under Program Files.
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
        let loaded = Arc::new(AtomicBool::new(false));
        let webview = self.webview(kind, port, section, &window, proxy, loaded.clone())?;
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
                loaded,
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
        loaded: Arc<AtomicBool>,
    ) -> anyhow::Result<WebView> {
        let id = window.id();
        let proxy = proxy.clone();
        let page = settings_url(port);
        let ipc_page = page.clone();
        let builder = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_ipc_handler(move |request| {
                if kind == Kind::Settings && !is_page(&ipc_page, &request.uri().to_string()) {
                    return;
                }
                let _ = proxy.send_event(UserEvent::Message {
                    window: id,
                    text: request.body().clone(),
                });
            })
            .with_navigation_handler(move |url| {
                may_navigate(kind, &page, loaded.load(Ordering::SeqCst), &url)
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

    pub fn has(&self, kind: Kind) -> bool {
        self.open.values().any(|w| w.kind == kind)
    }

    pub fn close_kind(&mut self, kind: Kind, target: &EventLoopWindowTarget<UserEvent>) {
        let ids: Vec<WindowId> = self
            .open
            .iter()
            .filter(|(_, w)| w.kind == kind)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.close(id, target);
        }
    }

    pub fn resend_all(&mut self) {
        for open in self.open.values_mut() {
            open.sent = None;
        }
    }

    pub fn loaded(&mut self, id: WindowId) {
        if let Some(open) = self.open.get_mut(&id) {
            open.loaded.store(true, Ordering::SeqCst);
            open.sent = None;
        }
    }

    pub fn send(&mut self, guide: &str, settings: &str) {
        for open in self.open.values_mut() {
            let json = match open.kind {
                Kind::Guide => guide,
                Kind::Settings => settings,
            };
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
    fn windows_load_only_their_own_page() {
        let page = settings_url(8787);
        for url in [
            "http://127.0.0.1:8787/settings",
            "http://127.0.0.1:8787/settings#phones",
        ] {
            assert!(may_navigate(Kind::Settings, &page, true, url), "{url}");
        }
        assert!(is_page(
            &settings_url(80),
            "http://127.0.0.1/settings#phones"
        ));
        assert!(!is_page(
            &settings_url(80),
            "http://127.0.0.1:8080/settings"
        ));
        for url in [
            "http://127.0.0.1:8787/settingsx",
            "http://127.0.0.1:8787/pair",
            "http://127.0.0.1:8787/v1/status",
            "https://evil.example/settings",
            "data:text/html,<script>ipc.postMessage('x')</script>",
            "about:blank",
        ] {
            assert!(!may_navigate(Kind::Settings, &page, false, url), "{url}");
        }
        assert!(may_navigate(Kind::Guide, &page, false, "about:blank"));
        assert!(may_navigate(
            Kind::Guide,
            &page,
            false,
            "data:text/html,guide"
        ));
        for url in [
            "about:blank",
            "data:text/html,<script>ipc.postMessage('x')</script>",
            "http://127.0.0.1:8787/settings",
            "https://evil.example/",
        ] {
            assert!(!may_navigate(Kind::Guide, &page, true, url), "{url}");
        }
    }

    #[test]
    fn the_guide_carries_the_shared_design() {
        let page = guide_page();
        assert!(!page.contains("/*UI_CSS*/"));
        assert!(page.contains("--accent"));
    }
}
