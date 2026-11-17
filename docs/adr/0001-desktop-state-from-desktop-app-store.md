# Desktop state comes from the Desktop app's store, not the Office mirror

Desktop state is read from the Desktop app's own store through the injected page script; the Relay's Office mirror (kumo-core's office model fed with sniffed socket frames) tracks only events: call credentials, Nudges and Knocks. The Relay attaches after the Desktop app has already received the full Office, so a mirror built from frames starts empty and never sees the initial census, entities or zones, and media state never appears on the wire at all.

## Considered Options

- **Mirror as the single source, seeded by rebuilding wire frames in the page script**: rejected, it adds a JS-to-wire translator that drifts with every Kumospace release.
- **Mirror as the single source, seeded by forcing the Desktop app to re-join**: rejected, it visibly disrupts the user's call.
