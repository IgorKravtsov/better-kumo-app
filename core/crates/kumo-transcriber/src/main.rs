mod whisper;

use std::path::PathBuf;

use anyhow::{Context, bail};
use jiff::tz::TimeZone;
use kumo_meetings::module::PROTOCOL;
use kumo_meetings::recording::RecordedMeeting;
use kumo_meetings::transcript::{TRANSCRIPT_FILE, render, transcribe};

struct Args {
    model: PathBuf,
    language: String,
    meeting: PathBuf,
}

fn parse(mut args: impl Iterator<Item = String>) -> anyhow::Result<Option<Args>> {
    let usage =
        "usage: better-kumo-transcriber --model <ggml model> [--language auto] <meeting folder>";
    let mut model = None;
    let mut language = "auto".to_string();
    let mut meeting = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" => {
                println!("{} {PROTOCOL}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--model" => model = Some(PathBuf::from(args.next().context(usage)?)),
            "--language" => language = args.next().context(usage)?,
            _ if arg.starts_with("--") => bail!("unknown option {arg}\n{usage}"),
            _ => meeting = Some(PathBuf::from(arg)),
        }
    }
    Ok(Some(Args {
        model: model.context(usage)?,
        language,
        meeting: meeting.context(usage)?,
    }))
}

fn main() -> anyhow::Result<()> {
    let Some(args) = parse(std::env::args().skip(1))? else {
        return Ok(());
    };
    let recorded = RecordedMeeting::load(&args.meeting)?;
    let started = std::time::Instant::now();
    let mut engine = whisper::Whisper::load(&args.model, &args.language)?;
    let transcript = transcribe(&recorded, &mut engine)?;
    let path = args.meeting.join(TRANSCRIPT_FILE);
    let partial = args.meeting.join(format!("{TRANSCRIPT_FILE}.part"));
    std::fs::write(&partial, render(&transcript, &TimeZone::system()))
        .with_context(|| format!("cannot write {}", partial.display()))?;
    std::fs::rename(&partial, &path)?;
    eprintln!(
        "{} lines in {:.1} s",
        transcript.lines.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
