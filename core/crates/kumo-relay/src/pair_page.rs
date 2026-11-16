use qrcode::QrCode;
use qrcode::render::svg;

use crate::Config;
use crate::network::pair_addresses;

pub fn render(config: &Config) -> String {
    let addresses = pair_addresses(config.port());
    let mut cards = String::new();
    for address in &addresses {
        let link = crate::pair_link(config, &address.url);
        let qr = QrCode::new(link.as_bytes())
            .map(|code| {
                code.render::<svg::Color>()
                    .min_dimensions(240, 240)
                    .quiet_zone(true)
                    .build()
            })
            .unwrap_or_default();
        cards.push_str(&format!(
            r#"<section class="card"><h2>{label}</h2><p class="url">{url}</p><div class="qr">{qr}</div><a class="link" href="{link}">Open link on this device</a></section>"#,
            label = address.label,
            url = address.url,
        ));
    }
    if addresses.is_empty() {
        cards.push_str(
            r#"<section class="card"><h2>No network address</h2><p>Connect this computer to Wi-Fi or Tailscale and reload the page.</p></section>"#,
        );
    }
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Pair phone</title>
<style>
:root{{color-scheme:light dark;--bg:#f5f5f7;--card:#fff;--text:#1d1d1f;--muted:#6e6e73}}
@media (prefers-color-scheme:dark){{:root{{--bg:#1c1c1e;--card:#2c2c2e;--text:#f5f5f7;--muted:#98989d}}}}
body{{margin:0;font:15px -apple-system,system-ui,sans-serif;background:var(--bg);color:var(--text)}}
main{{max-width:760px;margin:0 auto;padding:32px 16px}}
h1{{font-size:24px;margin:0 0 6px}} p{{color:var(--muted);margin:0 0 20px}}
.grid{{display:flex;flex-wrap:wrap;gap:16px}}
.card{{background:var(--card);border-radius:16px;padding:20px;flex:1 1 300px;text-align:center}}
.card h2{{font-size:17px;margin:0 0 4px}} .url{{font-family:ui-monospace,monospace;font-size:13px}}
.qr svg{{width:240px;height:240px;background:#fff;border-radius:12px}}
.link{{display:inline-block;margin-top:10px;font-size:13px}}
</style></head><body><main>
<h1>Pair your phone with Better Kumo</h1>
<p>Open the camera on your iPhone or Android phone and point it at a code. Use <b>Tailscale</b> to control the desktop from anywhere, or <b>Same Wi-Fi</b> when the phone is on this network. The code contains a secret: do not share it.</p>
<div class="grid">{cards}</div>
</main></body></html>"#
    )
}
