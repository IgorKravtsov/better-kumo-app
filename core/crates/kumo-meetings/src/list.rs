use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;

use crate::recording::{MEETING_FILE, RecordedMeeting};
use crate::transcript::TRANSCRIPT_FILE;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MeetingState {
    Recording,
    Transcribing,
    Queued,
    Failed { error: String },
    NotTranscribed,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeetingEntry {
    pub name: String,
    pub dir: PathBuf,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub zones: Vec<String>,
    pub people: Vec<String>,
    pub state: MeetingState,
    pub transcript: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Jobs {
    pub recording: Option<PathBuf>,
    pub transcribing: Option<PathBuf>,
    pub queued: HashSet<PathBuf>,
    pub failed: HashMap<PathBuf, String>,
}

impl Jobs {
    fn state(&self, dir: &Path, done: bool) -> MeetingState {
        if self.recording.as_deref() == Some(dir) {
            MeetingState::Recording
        } else if self.transcribing.as_deref() == Some(dir) {
            MeetingState::Transcribing
        } else if self.queued.contains(dir) {
            MeetingState::Queued
        } else if done {
            MeetingState::Done
        } else if let Some(error) = self.failed.get(dir) {
            MeetingState::Failed {
                error: error.clone(),
            }
        } else {
            MeetingState::NotTranscribed
        }
    }
}

pub(crate) fn list(folder: &Path, jobs: &Jobs) -> anyhow::Result<Vec<MeetingEntry>> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", folder.display())),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot read {}", folder.display()))?;
        dirs.push(entry.path());
    }
    let mut meetings: Vec<MeetingEntry> = dirs
        .into_iter()
        .filter(|dir| dir.join(MEETING_FILE).exists())
        .filter_map(|dir| RecordedMeeting::load(&dir).ok())
        .map(|recorded| entry(recorded, jobs))
        .collect();
    meetings.sort_by(|a, b| b.started_ms.cmp(&a.started_ms).then(b.name.cmp(&a.name)));
    Ok(meetings)
}

fn entry(recorded: RecordedMeeting, jobs: &Jobs) -> MeetingEntry {
    let transcript = Some(recorded.dir.join(TRANSCRIPT_FILE)).filter(|p| p.exists());
    let mut people: Vec<String> = Vec::new();
    for track in &recorded.meeting.tracks {
        if !track.name.is_empty() && !people.contains(&track.name) {
            people.push(track.name.clone());
        }
    }
    MeetingEntry {
        name: recorded
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        state: jobs.state(&recorded.dir, transcript.is_some()),
        started_ms: recorded.meeting.started_ms,
        ended_ms: recorded.meeting.ended_ms,
        zones: recorded.meeting.zones,
        people,
        transcript,
        dir: recorded.dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{MeetingFile, TrackFile};

    fn meeting(folder: &Path, name: &str, started_ms: i64, done: bool) -> PathBuf {
        let dir = folder.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = MeetingFile {
            started_ms,
            ended_ms: started_ms + 60_000,
            zones: vec!["Lounge".into()],
            tracks: vec![
                TrackFile {
                    id: "a".into(),
                    name: "Anna".into(),
                    me: false,
                    file: "01-a.wav".into(),
                },
                TrackFile {
                    id: "b".into(),
                    name: String::new(),
                    me: true,
                    file: "02-b.wav".into(),
                },
            ],
        };
        std::fs::write(
            dir.join(MEETING_FILE),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
        if done {
            std::fs::write(dir.join(TRANSCRIPT_FILE), "# done").unwrap();
        }
        dir
    }

    #[test]
    fn lists_every_meeting_newest_first_with_its_state() {
        let folder = tempfile::tempdir().unwrap();
        let done = meeting(folder.path(), "done", 1_000, true);
        let live = meeting(folder.path(), "live", 6_000, false);
        let running = meeting(folder.path(), "running", 5_000, false);
        let queued = meeting(folder.path(), "queued", 4_000, false);
        let failed = meeting(folder.path(), "failed", 3_000, false);
        meeting(folder.path(), "waiting", 2_000, false);
        std::fs::create_dir_all(folder.path().join("not a meeting")).unwrap();
        let jobs = Jobs {
            recording: Some(live),
            transcribing: Some(running),
            queued: HashSet::from([queued]),
            failed: HashMap::from([(failed, "whisper crashed".to_string())]),
        };

        let list = list(folder.path(), &jobs).unwrap();

        let states: Vec<(&str, &MeetingState)> =
            list.iter().map(|m| (m.name.as_str(), &m.state)).collect();
        assert_eq!(
            states,
            vec![
                ("live", &MeetingState::Recording),
                ("running", &MeetingState::Transcribing),
                ("queued", &MeetingState::Queued),
                (
                    "failed",
                    &MeetingState::Failed {
                        error: "whisper crashed".into()
                    }
                ),
                ("waiting", &MeetingState::NotTranscribed),
                ("done", &MeetingState::Done),
            ]
        );
        let last = list.last().unwrap();
        assert_eq!(last.transcript, Some(done.join(TRANSCRIPT_FILE)));
        assert_eq!(last.people, vec!["Anna".to_string()]);
        assert_eq!(last.zones, vec!["Lounge".to_string()]);
        assert_eq!(list[0].transcript, None);
    }

    #[test]
    fn a_missing_folder_has_no_meetings() {
        let folder = tempfile::tempdir().unwrap();
        assert!(
            list(&folder.path().join("nope"), &Jobs::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_folder_that_cannot_be_read_is_an_error_not_an_empty_list() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("a file");
        std::fs::write(&file, "").unwrap();
        assert!(list(&file, &Jobs::default()).is_err());
    }
}
