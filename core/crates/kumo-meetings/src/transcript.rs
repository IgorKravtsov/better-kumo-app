use std::fmt::Write as _;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::audio::{samples_to_ms, speech_regions, to_f32};
use crate::recording::{MeetingFile, RecordedMeeting, read_track};

pub const TRANSCRIPT_FILE: &str = "transcript.md";
const JOIN_GAP_MS: i64 = 1_500;
const GHOSTS: &[&str] = &[
    "продолжение следует",
    "субтитры сделал",
    "субтитры создавал",
    "редактор субтитров",
    "спасибо за просмотр",
    "thank you for watching",
    "thanks for watching",
    "дякую за перегляд",
    "амара.орг",
    "amara.org",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

pub trait SpeechToText {
    fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Segment>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub at_ms: i64,
    pub end_ms: i64,
    pub speaker_id: String,
    pub speaker: String,
    pub me: bool,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub meeting: MeetingFile,
    pub lines: Vec<Line>,
}

pub fn transcribe(
    recorded: &RecordedMeeting,
    stt: &mut dyn SpeechToText,
) -> anyhow::Result<Transcript> {
    let mut lines = Vec::new();
    for track in &recorded.meeting.tracks {
        let samples = to_f32(&read_track(&recorded.track_path(track))?);
        for region in speech_regions(&samples) {
            let offset = samples_to_ms(region.start);
            for segment in stt.transcribe(&samples[region.start..region.end])? {
                let text = clean(&segment.text);
                if text.is_empty() {
                    continue;
                }
                lines.push(Line {
                    at_ms: offset + segment.start_ms,
                    end_ms: offset + segment.end_ms,
                    speaker_id: track.id.clone(),
                    speaker: track.name.clone(),
                    me: track.me,
                    text,
                });
            }
        }
    }
    lines.sort_by_key(|l| l.at_ms);
    Ok(Transcript {
        meeting: recorded.meeting.clone(),
        lines: joined(lines),
    })
}

fn clean(text: &str) -> String {
    let text = text.trim();
    let lower = text.to_lowercase();
    if GHOSTS.iter().any(|g| lower.contains(g)) {
        return String::new();
    }
    if !text.chars().any(char::is_alphanumeric) {
        return String::new();
    }
    text.to_string()
}

fn joined(lines: Vec<Line>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    for line in lines {
        match out.last_mut() {
            Some(last)
                if last.speaker_id == line.speaker_id
                    && line.at_ms - last.end_ms <= JOIN_GAP_MS =>
            {
                last.text.push(' ');
                last.text.push_str(&line.text);
                last.end_ms = last.end_ms.max(line.end_ms);
            }
            _ => out.push(line),
        }
    }
    out
}

pub fn render(transcript: &Transcript, tz: &TimeZone) -> String {
    let meeting = &transcript.meeting;
    let start = local(meeting.started_ms, tz);
    let end = local(meeting.ended_ms, tz);
    let minutes = ((meeting.ended_ms - meeting.started_ms) as f64 / 60_000.0).round() as i64;
    let where_ = if meeting.zones.is_empty() {
        "the open floor".to_string()
    } else {
        meeting.zones.join(", ")
    };
    let who: Vec<String> = meeting
        .tracks
        .iter()
        .map(|t| {
            if t.me {
                format!("{} (me)", t.name)
            } else {
                t.name.clone()
            }
        })
        .collect();

    let mut out = String::new();
    let _ = writeln!(out, "# {} · {}", start.strftime("%Y-%m-%d %H:%M"), where_);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "- **When:** {}–{} ({} min)",
        start.strftime("%Y-%m-%d %H:%M"),
        end.strftime("%H:%M"),
        minutes.max(1)
    );
    let _ = writeln!(out, "- **Where:** {where_}");
    let _ = writeln!(out, "- **Who:** {}", who.join(", "));
    let _ = writeln!(out);
    let _ = writeln!(out, "---");
    let _ = writeln!(out);
    if transcript.lines.is_empty() {
        let _ = writeln!(out, "_No speech was recognised._");
    }
    for line in &transcript.lines {
        let _ = writeln!(
            out,
            "**[{}] {}:** {}",
            clock(line.at_ms),
            line.speaker,
            line.text
        );
        let _ = writeln!(out);
    }
    out
}

fn local(ms: i64, tz: &TimeZone) -> jiff::Zoned {
    Timestamp::from_millisecond(ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(tz.clone())
}

fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1_000;
    format!("{:02}:{:02}:{:02}", s / 3_600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Chunk;
    use crate::audio::Speaker;
    use crate::audio::tests::tone_i16;
    use crate::recording::Recording;

    struct Fake {
        calls: Vec<usize>,
    }

    impl SpeechToText for Fake {
        fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Segment>> {
            self.calls.push(samples.len());
            let n = self.calls.len();
            Ok(vec![Segment {
                start_ms: 100,
                end_ms: samples_to_ms(samples.len()) - 100,
                text: format!(" phrase {n} "),
            }])
        }
    }

    fn speaker(id: &str, name: &str, me: bool) -> Speaker {
        Speaker {
            id: id.into(),
            name: name.into(),
            me,
        }
    }

    fn talk(rec: &mut Recording, who: &Speaker, at_ms: i64, ms: i64) {
        rec.push(&Chunk {
            speaker: who.clone(),
            at_ms,
            samples: tone_i16(ms, 0.3),
        })
        .unwrap();
    }

    #[test]
    fn interleaves_every_speaker_by_time_and_names_them() {
        let dir = tempfile::tempdir().unwrap();
        let me = speaker("u-me", "Igor", true);
        let anna = speaker("u-anna", "Anna", false);
        let mut rec = Recording::create(dir.path().join("m"), 0).unwrap();
        talk(&mut rec, &me, 1_000, 2_000);
        talk(&mut rec, &anna, 5_000, 2_000);
        talk(&mut rec, &me, 10_000, 1_000);
        rec.note_zone("Kitchen");
        let recorded = rec.finish().unwrap();

        let mut stt = Fake { calls: vec![] };
        let transcript = transcribe(&recorded, &mut stt).unwrap();
        let said: Vec<(String, i64)> = transcript
            .lines
            .iter()
            .map(|l| (l.speaker.clone(), l.at_ms / 1_000))
            .collect();
        assert_eq!(
            said,
            vec![("Igor".into(), 0), ("Anna".into(), 4), ("Igor".into(), 9)]
        );
        assert_eq!(
            stt.calls.len(),
            3,
            "only the speech regions are transcribed"
        );

        let text = render(&transcript, &TimeZone::UTC);
        assert!(text.starts_with("# 1970-01-01 00:00 · Kitchen\n"));
        assert!(
            text.contains("- **Who:** Igor (me), Anna\n")
                || text.contains("- **Who:** Anna, Igor (me)\n")
        );
        assert!(text.contains("**[00:00:04] Anna:** phrase"));
    }

    #[test]
    fn joins_one_speakers_close_lines_and_drops_ghost_phrases() {
        let line = |at_ms: i64, speaker: &str, text: &str| Line {
            at_ms,
            end_ms: at_ms + 1_000,
            speaker_id: format!("u-{speaker}"),
            speaker: speaker.into(),
            me: false,
            text: text.into(),
        };
        let joined = joined(vec![
            line(0, "Anna", "Hello"),
            line(1_500, "Anna", "there"),
            line(5_000, "Anna", "Later"),
            line(6_000, "Bob", "Hi"),
        ]);
        assert_eq!(joined.len(), 3);
        assert_eq!(joined[0].text, "Hello there");
        assert_eq!(clean("Продолжение следует..."), "");
        assert_eq!(clean(" ... "), "");
        assert_eq!(clean(" Привет "), "Привет");
    }

    #[test]
    fn two_people_with_the_same_name_stay_apart() {
        let dir = tempfile::tempdir().unwrap();
        let me = speaker("u-alex-1", "Alex", true);
        let other = speaker("u-alex-2", "Alex", false);
        let mut rec = Recording::create(dir.path().join("m"), 0).unwrap();
        talk(&mut rec, &me, 1_000, 1_000);
        talk(&mut rec, &other, 2_200, 1_000);
        let recorded = rec.finish().unwrap();
        let transcript = transcribe(&recorded, &mut Fake { calls: vec![] }).unwrap();
        let who: Vec<(&str, bool)> = transcript
            .lines
            .iter()
            .map(|l| (l.speaker_id.as_str(), l.me))
            .collect();
        assert_eq!(who, vec![("u-alex-1", true), ("u-alex-2", false)]);
    }

    #[test]
    fn formats_clock_offsets() {
        assert_eq!(clock(3_725_000), "01:02:05");
        assert_eq!(clock(-5), "00:00:00");
    }
}
