use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::audio::{Chunk, SAMPLE_RATE, Speaker, ms_to_samples};

const ALIGN_SAMPLES: u64 = SAMPLE_RATE as u64 / 100;
pub const MEETING_FILE: &str = "meeting.json";
pub const AUDIO_DIR: &str = "audio";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackFile {
    pub id: String,
    pub name: String,
    pub me: bool,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingFile {
    pub started_ms: i64,
    pub ended_ms: i64,
    pub zones: Vec<String>,
    pub tracks: Vec<TrackFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedMeeting {
    pub dir: PathBuf,
    pub meeting: MeetingFile,
}

impl RecordedMeeting {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(dir.join(MEETING_FILE))
            .with_context(|| format!("no {MEETING_FILE} in {}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            meeting: serde_json::from_str(&text)?,
        })
    }

    pub fn track_path(&self, track: &TrackFile) -> PathBuf {
        self.dir.join(AUDIO_DIR).join(&track.file)
    }
}

struct Track {
    speaker: Speaker,
    file: String,
    writer: hound::WavWriter<BufWriter<File>>,
    written: u64,
}

pub struct Recording {
    dir: PathBuf,
    started_ms: i64,
    last_ms: i64,
    zones: Vec<String>,
    tracks: BTreeMap<String, Track>,
}

impl Recording {
    pub fn create(dir: PathBuf, started_ms: i64) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir.join(AUDIO_DIR))
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let recording = Self {
            dir,
            started_ms,
            last_ms: started_ms,
            zones: Vec::new(),
            tracks: BTreeMap::new(),
        };
        recording.save()?;
        Ok(recording)
    }

    pub fn checkpoint(&mut self) -> anyhow::Result<()> {
        for track in self.tracks.values_mut() {
            track.writer.flush()?;
        }
        self.save()
    }

    fn meeting(&self) -> MeetingFile {
        MeetingFile {
            started_ms: self.started_ms,
            ended_ms: self.last_ms,
            zones: self.zones.clone(),
            tracks: self
                .tracks
                .values()
                .map(|t| TrackFile {
                    id: t.speaker.id.clone(),
                    name: t.speaker.name.clone(),
                    me: t.speaker.me,
                    file: t.file.clone(),
                })
                .collect(),
        }
    }

    fn save(&self) -> anyhow::Result<()> {
        let partial = self.dir.join(format!("{MEETING_FILE}.part"));
        std::fs::write(&partial, serde_json::to_string_pretty(&self.meeting())?)?;
        std::fs::rename(&partial, self.dir.join(MEETING_FILE))?;
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn started_ms(&self) -> i64 {
        self.started_ms
    }

    pub fn note_zone(&mut self, zone: &str) {
        if !zone.is_empty() && !self.zones.iter().any(|z| z == zone) {
            self.zones.push(zone.to_string());
        }
    }

    pub fn push(&mut self, chunk: &Chunk) -> anyhow::Result<()> {
        if chunk.end_ms() <= self.started_ms {
            return Ok(());
        }
        let skip = ms_to_samples(self.started_ms - chunk.at_ms).min(chunk.samples.len());
        let mut samples = &chunk.samples[skip..];
        let offset = ms_to_samples(chunk.at_ms - self.started_ms) as u64;
        let track = self.track(&chunk.speaker)?;
        if offset > track.written + ALIGN_SAMPLES {
            for _ in track.written..offset {
                track.writer.write_sample(0i16)?;
            }
            track.written = offset;
        } else if offset + ALIGN_SAMPLES < track.written {
            let overlap = usize::try_from(track.written - offset).unwrap_or(usize::MAX);
            samples = &samples[overlap.min(samples.len())..];
        }
        for s in samples {
            track.writer.write_sample(*s)?;
        }
        track.written += samples.len() as u64;
        self.last_ms = self.last_ms.max(chunk.end_ms());
        Ok(())
    }

    pub fn finish(mut self) -> anyhow::Result<RecordedMeeting> {
        self.checkpoint()?;
        let meeting = self.meeting();
        for (_, track) in std::mem::take(&mut self.tracks) {
            track.writer.finalize()?;
        }
        Ok(RecordedMeeting {
            dir: self.dir,
            meeting,
        })
    }

    fn track(&mut self, speaker: &Speaker) -> anyhow::Result<&mut Track> {
        if !self.tracks.contains_key(&speaker.id) {
            let file = format!(
                "{:02}-{}.wav",
                self.tracks.len() + 1,
                file_stem(&speaker.id)
            );
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let writer = hound::WavWriter::create(self.dir.join(AUDIO_DIR).join(&file), spec)?;
            self.tracks.insert(
                speaker.id.clone(),
                Track {
                    speaker: speaker.clone(),
                    file,
                    writer,
                    written: 0,
                },
            );
        }
        let track = self.tracks.get_mut(&speaker.id).context("track vanished")?;
        if !speaker.name.is_empty() {
            track.speaker.name = speaker.name.clone();
        }
        Ok(track)
    }
}

pub fn read_track(path: &Path) -> anyhow::Result<Vec<i16>> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    Ok(reader.samples::<i16>().collect::<Result<_, _>>()?)
}

pub fn file_stem(id: &str) -> String {
    let stem: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if stem.is_empty() {
        "speaker".into()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::samples_to_ms;
    use crate::audio::tests::tone_i16;

    fn speaker(id: &str, name: &str, me: bool) -> Speaker {
        Speaker {
            id: id.into(),
            name: name.into(),
            me,
        }
    }

    fn chunk(who: &Speaker, at_ms: i64, ms: i64) -> Chunk {
        Chunk {
            speaker: who.clone(),
            at_ms,
            samples: tone_i16(ms, 0.3),
        }
    }

    #[test]
    fn writes_one_aligned_track_per_speaker() {
        let dir = tempfile::tempdir().unwrap();
        let anna = speaker("u-anna", "Anna", false);
        let me = speaker("u-me", "Igor", true);
        let mut rec = Recording::create(dir.path().join("m"), 10_000).unwrap();
        rec.push(&chunk(&me, 10_000, 1_000)).unwrap();
        rec.push(&chunk(&anna, 12_000, 1_000)).unwrap();
        rec.push(&chunk(&anna, 13_050, 500)).unwrap();
        rec.note_zone("Kitchen");
        rec.note_zone("Kitchen");
        let done = rec.finish().unwrap();

        assert_eq!(done.meeting.started_ms, 10_000);
        assert_eq!(done.meeting.ended_ms, 13_550);
        assert_eq!(done.meeting.zones, vec!["Kitchen".to_string()]);
        let anna_track = done
            .meeting
            .tracks
            .iter()
            .find(|t| t.id == "u-anna")
            .unwrap();
        let samples = read_track(&done.track_path(anna_track)).unwrap();
        assert_eq!(
            samples_to_ms(samples.len()),
            3_550,
            "2 s of silence, 1 s, the 50 ms gap, then 0.5 s"
        );
        assert!(samples[..ms_to_samples(2_000)].iter().all(|s| *s == 0));
        assert_eq!(RecordedMeeting::load(&done.dir).unwrap(), done);
    }

    #[test]
    fn a_late_first_chunk_and_short_gaps_keep_their_place_and_overlaps_are_cut() {
        let dir = tempfile::tempdir().unwrap();
        let anna = speaker("u-anna", "Anna", false);
        let mut rec = Recording::create(dir.path().join("m"), 0).unwrap();
        rec.push(&chunk(&anna, 200, 1_000)).unwrap();
        rec.push(&chunk(&anna, 1_205, 500)).unwrap();
        rec.push(&chunk(&anna, 1_300, 1_000)).unwrap();
        let done = rec.finish().unwrap();
        let samples = read_track(&done.track_path(&done.meeting.tracks[0])).unwrap();
        assert!(samples[..ms_to_samples(200)].iter().all(|s| *s == 0));
        assert_ne!(samples[ms_to_samples(200) + 10], 0);
        assert_eq!(samples_to_ms(samples.len()), 2_300);
    }

    #[test]
    fn audio_from_before_the_start_is_cut() {
        let dir = tempfile::tempdir().unwrap();
        let anna = speaker("u-anna", "Anna", false);
        let mut rec = Recording::create(dir.path().join("m"), 10_000).unwrap();
        rec.push(&chunk(&anna, 8_000, 1_000)).unwrap();
        rec.push(&chunk(&anna, 9_500, 1_000)).unwrap();
        let done = rec.finish().unwrap();
        let samples = read_track(&done.track_path(&done.meeting.tracks[0])).unwrap();
        assert_eq!(samples_to_ms(samples.len()), 500);
    }

    #[test]
    fn speakers_whose_ids_look_alike_get_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = Recording::create(dir.path().join("m"), 0).unwrap();
        rec.push(&chunk(&speaker("a.b", "Anna", false), 0, 1_000))
            .unwrap();
        rec.push(&chunk(&speaker("a_b", "Bob", false), 0, 500))
            .unwrap();
        let done = rec.finish().unwrap();
        let files: Vec<&str> = done
            .meeting
            .tracks
            .iter()
            .map(|t| t.file.as_str())
            .collect();
        assert_eq!(files.len(), 2);
        assert_ne!(files[0], files[1]);
        for track in &done.meeting.tracks {
            let ms = samples_to_ms(read_track(&done.track_path(track)).unwrap().len());
            assert_eq!(ms, if track.name == "Anna" { 1_000 } else { 500 });
        }
    }

    #[test]
    fn a_recording_cut_off_mid_meeting_can_still_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let anna = speaker("u-anna", "Anna", false);
        let mut rec = Recording::create(dir.path().join("m"), 0).unwrap();
        assert!(
            RecordedMeeting::load(rec.dir())
                .unwrap()
                .meeting
                .tracks
                .is_empty()
        );
        rec.push(&chunk(&anna, 0, 1_000)).unwrap();
        rec.checkpoint().unwrap();
        let path = rec.dir().to_path_buf();
        std::mem::forget(rec);
        let recovered = RecordedMeeting::load(&path).unwrap();
        assert_eq!(recovered.meeting.tracks.len(), 1);
        assert_eq!(recovered.meeting.ended_ms, 1_000);
        let samples = read_track(&recovered.track_path(&recovered.meeting.tracks[0])).unwrap();
        assert_eq!(samples_to_ms(samples.len()), 1_000);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_stem("a/b:c"), "a_b_c");
        assert_eq!(file_stem(""), "speaker");
    }
}
