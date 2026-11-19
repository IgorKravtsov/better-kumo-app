# Kumowatch

A faster companion to Kumospace: native phone apps, plus a relay on the office computer (macOS, Windows or Linux) that lets phones see and control the Kumospace desktop app.

## Language

### Kumospace

**Office**:
A Kumospace space as a live place: its floors, zones, the people in them and their positions.
_Avoid_: room (when the whole space is meant), map

**Floor**:
One navigable area of the Office with its own layout and zones.
_Avoid_: room, level

**Zone**:
A bounded region on a Floor; people in the same Zone share a call.
_Avoid_: area, room

**Zone mates**:
The other people currently in the same Zone as you.
_Avoid_: peers, participants

**Availability**:
A person's self-chosen presence in the Office: available, away or focusing.
_Avoid_: status (too broad), presence

**Nudge**:
A ping from one person to another in the Office, meant to get their attention.
_Avoid_: poke, ping

**Knock**:
A request from someone outside a closed Zone to be let in.
_Avoid_: door knock, entry request

**Broadcast**:
Someone presenting their camera or screen to the whole Office.
_Avoid_: stream, presentation

**Public Chat**:
The Office-wide session chat, carried over the Office connection rather than Sendbird.
_Avoid_: space chat, global chat

**Nearby Chat**:
The session chat with the people around you or in your Zone, also carried over the Office connection.
_Avoid_: room chat, local chat

**Attachment**:
A photo picked in the composer and held there until Send; it goes out with the typed text as one Outbox batch.
_Avoid_: upload, media

**Outbox batch**:
The Sendbird messages one Send produces: each Attachment as its own file message, in picked order, then the text. They are queued, fail and retry together.
_Avoid_: multi-file message (that is Sendbird's MFM, which Kumowatch does not send)

**Floor entry**:
Someone else joining the Floor the user is on (the Desktop app's "<name> joined the Floor" toast).
_Avoid_: arrival, join

**Alert**:
What a Kumowatch user is told about something that happened (a message, a Nudge, a Knock, a Broadcast, a Public or Nearby Chat message, a Floor entry), independent of how it is delivered.
_Avoid_: notification (reserved for the OS-level delivery), push

**Notification kinds**:
The per-kind switches (Messages, Mentions, Nudges, Knocks, Broadcasts, Public Chat, Nearby Chat, Floor entries) the Relay applies under Mirroring before delivering an Alert. Floor entries are off unless turned on; every other kind is on.
_Avoid_: filters, channels

**My status**:
Which of four statuses decides the user's Notification kinds right now: Available, Away or Focusing (their own Availability), or Unknown when it can't be known (the Desktop app or the Office connection is down, or they are idle or offline). The Relay reads it from the Desktop app; This phone reads it from its own Office connection.
_Avoid_: presence (that's anyone's), availability (only the three chosen ones)

**Notification profile**:
One set of Notification kinds per My status. An Alert is delivered only if the kinds for the current My status allow it; an older single set of kinds counts as the same set for all four.
_Avoid_: settings, rules

**Delivery**:
How an Android phone receives Alerts: through ntfy (the Relay publishes to the user's topic) or on This phone (the app keeps its own Office connection and shows Android notifications). One Notification profile applies to both; iOS uses ntfy only.
_Avoid_: mode, transport, channel

### The Mac side

**Desktop app**:
The official Kumospace app running on the user's Mac.
_Avoid_: Electron app, client

**Relay**:
The Kumowatch process on the office computer that watches the Desktop app and serves phones over the local network.
_Avoid_: server, bridge, daemon

**Desktop bridge**:
The part of the Relay that observes and drives the Desktop app.
_Avoid_: CDP client, desktop service

**Mirroring**:
The Relay's main switch for sending Alerts to phones; the iOS widget flips it, and the Android widget does when Delivery is ntfy.
_Avoid_: enabled, notifications on

**Office mirror**:
The Relay's own model of the Office as the Desktop app sees it.
_Avoid_: desktop state cache, snapshot

**Desktop state**:
What a phone is shown about the Desktop app: the current Floor, the people on it, the user's own position, Availability and media.
_Avoid_: desk, desk state

**Earshot**:
The people whose audio the Desktop app is receiving right now (Daily subscribes it to everyone the user can hear: their Zone, or their audio range on the open floor).
_Avoid_: audio range (that is the Office setting), nearby, group

**Meeting**:
A conversation the Relay records and transcribes. The user starts and stops one by hand at any time, or, with automatic start on, it starts once the same Earshot has held for the settle time and reaches back to when that Earshot formed; then people joining or leaving don't end it, being alone or a long silence does.
_Avoid_: call (that is the Daily session), recording (that is the audio of a Meeting)

**Meetings module**:
The optional download that turns recorded Meetings into transcripts: the `better-kumo-transcriber` program (Whisper) and its speech model. Installed from the Settings page; Better Kumo itself never contains Whisper.
_Avoid_: plugin, add-on, transcription engine

**Settle time**:
How long the Earshot must stay exactly the same before a Meeting starts (`settle_seconds`, 5 s by default), so someone walking past is never recorded.
_Avoid_: delay, debounce

### Calls

**Call session**:
One Kumowatch user's participation in a Zone's call, from joining to leaving, wherever the media flows.
_Avoid_: call model, call manager

**Call source**:
Where a Call session gets its Office and call credentials from: the phone's own Office connection, or the Desktop app through the Relay.
_Avoid_: mode, call type

**Handoff**:
Moving the user's active call from the Desktop app to a phone.
_Avoid_: transfer, takeover

**Reclaim**:
Returning a handed-off call to the Desktop app.
_Avoid_: give back, release

**Lease**:
A phone's ongoing claim on a handed-off call, renewed every few seconds; the newest Handoff takes it from any other phone, and when it lapses the Relay Reclaims.
_Avoid_: heartbeat, keepalive
