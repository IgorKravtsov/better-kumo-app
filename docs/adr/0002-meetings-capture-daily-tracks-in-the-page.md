# Meetings capture each person's Daily track inside the Desktop app

A Meeting is recorded from the Desktop app's own Daily tracks, one per person, through an `AudioWorklet` injected over
DevTools, and transcribed in Rust with whisper.cpp. Kumospace already subscribes the Desktop app to exactly the people
the user can hear, so the Earshot and the audio come from the same place, each line of the transcript has a real
name, and there is no echo to remove. It needs no OS permission and works the same on macOS, Windows and Linux.

Whisper ships as a separate download, the Meetings module, not inside Better Kumo: most people never record
meetings, whisper.cpp is a large C++ build per platform, and a separate process keeps a crash or a long transcription
away from the Relay.

## Considered Options

- **Run Huginn (a separate C# app) next to Better Kumo**: rejected, it is a second app with its own Kumospace watcher,
  needs .NET and the Screen & System Audio Recording permission, has no Linux build, and mixes everyone into one
  "Them" track whose speakers have to be guessed from audio levels.
- **System audio + microphone capture in Rust**: rejected, three OS-specific capture stacks, one mixed far-side track,
  and echo removal for anyone on speakers.
- **Whisper linked into Better Kumo**: rejected, everyone would download and build it for a feature few use.
- **The module as a dynamic library loaded into the Relay**: rejected, Rust has no stable ABI, and a crash in
  whisper.cpp would take the Relay down.
- **`MediaRecorder` in the page (Opus/WebM)**: rejected, the Relay would need an Opus/WebM decoder; an `AudioContext`
  at 16 kHz hands out the PCM Whisper wants directly.
