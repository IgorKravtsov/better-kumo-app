use qrcode::QrCode;
use qrcode::render::svg;

use crate::Config;
use crate::network::{PairAddress, pair_addresses};

pub fn render(config: &Config) -> String {
    render_for(config, &pair_addresses(config.port()))
}

pub fn render_for(config: &Config, addresses: &[PairAddress]) -> String {
    let mut cards = String::new();
    for address in addresses {
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
            r#"<section class="card"><h2>{label}</h2><p class="url">{url}</p><div class="qr">{qr}</div><div class="actions"><button type="button" class="copy" data-link="{link}">Copy link</button><a class="link" href="{link}">Open link on this device</a></div></section>"#,
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
.actions{{display:flex;flex-wrap:wrap;gap:12px;justify-content:center;align-items:center;margin-top:10px}}
.link{{font-size:13px}}
.copy{{font:inherit;font-size:13px;padding:6px 14px;border-radius:8px;border:1px solid var(--muted);background:transparent;color:var(--text);cursor:pointer}}
</style></head><body><main>
<h1>Pair your phone with Better Kumo</h1>
<p>Open the camera on your iPhone or Android phone and point it at a code. Use <b>Tailscale</b> to control the desktop from anywhere, or <b>Same Wi-Fi</b> when the phone is on this network. To connect another app or integration, use <b>Copy link</b> and paste the link there. The code and the link contain a secret: do not share them.</p>
<div class="grid">{cards}</div>
</main>
<script>
for (const button of document.querySelectorAll(".copy")) {{
  button.addEventListener("click", async () => {{
    const link = button.dataset.link;
    try {{
      await navigator.clipboard.writeText(link);
    }} catch (e) {{
      const area = document.createElement("textarea");
      area.value = link;
      document.body.appendChild(area);
      area.select();
      document.execCommand("copy");
      area.remove();
    }}
    button.textContent = "Copied";
    setTimeout(() => {{ button.textContent = "Copy link"; }}, 1500);
  }});
}}
</script></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_has_a_copy_button_for_its_own_link() {
        let config = Config::template("pair-secret", "office");
        let addresses = [
            PairAddress {
                label: "Tailscale",
                url: "http://100.64.0.1:8787".into(),
            },
            PairAddress {
                label: "Same Wi-Fi",
                url: "http://192.168.1.20:8787".into(),
            },
        ];
        let page = render_for(&config, &addresses);
        for address in &addresses {
            let link = crate::pair_link(&config, &address.url);
            assert!(page.contains(&format!(
                r#"<button type="button" class="copy" data-link="{link}">Copy link</button>"#
            )));
        }
    }
}
