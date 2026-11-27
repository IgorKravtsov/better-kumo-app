use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;

use crate::audio::SAMPLE_RATE;
use crate::config::MeetingsConfig;
use crate::recording::{AUDIO_DIR, MEETING_FILE, RecordedMeeting};
use crate::transcript::TRANSCRIPT_FILE;

const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AudioUsage {
    pub bytes: u64,
    pub meetings: usize,
}

fn meeting_dirs(folder: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                // file_type does not follow links: a linked folder may be another meeting's.
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .filter(|dir| dir.join(MEETING_FILE).exists())
                .collect()
        })
        .unwrap_or_default()
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

pub fn audio_usage(folder: &Path) -> AudioUsage {
    let mut usage = AudioUsage::default();
    for dir in meeting_dirs(folder) {
        let audio = dir.join(AUDIO_DIR);
        if audio.is_dir() {
            usage.bytes += dir_size(&audio);
            usage.meetings += 1;
        }
    }
    usage
}

/// Untranscribed meetings still need their audio.
fn transcribed_with_audio(folder: &Path) -> Vec<RecordedMeeting> {
    meeting_dirs(folder)
        .into_iter()
        .filter(|dir| dir.join(TRANSCRIPT_FILE).exists() && dir.join(AUDIO_DIR).is_dir())
        .filter_map(|dir| RecordedMeeting::load(&dir).ok())
        .collect()
}

/// Fails, naming the first folder, when any audio could not be deleted.
pub fn delete_audio(folder: &Path) -> anyhow::Result<usize> {
    let mut deleted = 0;
    let mut failed = Vec::new();
    for meeting in transcribed_with_audio(folder) {
        let audio = meeting.dir.join(AUDIO_DIR);
        match std::fs::remove_dir_all(&audio) {
            Ok(()) => deleted += 1,
            Err(e) => failed.push(format!("{}: {e}", audio.display())),
        }
    }
    if let Some(first) = failed.first() {
        anyhow::bail!(
            "deleted the audio of {deleted} meetings, but not of {} (first: {first})",
            failed.len()
        );
    }
    Ok(deleted)
}

pub fn settle(recorded: &RecordedMeeting, config: &MeetingsConfig, now_ms: i64) {
    // Saturating: a hand-edited huge value must mean "long ago", never wrap into the future.
    let kept_ms = i64::try_from(config.audio_days)
        .unwrap_or(i64::MAX)
        .saturating_mul(DAY_MS);
    let expired =
        config.audio_days > 0 && recorded.meeting.ended_ms < now_ms.saturating_sub(kept_ms);
    if !config.keep_audio || expired {
        if std::fs::remove_dir_all(recorded.dir.join(AUDIO_DIR)).is_ok() {
            eprintln!("meetings: deleted the audio of {}", recorded.dir.display());
        }
    } else if config.compress_audio
        && let Err(e) = compress(recorded)
    {
        eprintln!(
            "meetings: cannot compress {}: {e:#}",
            recorded.dir.display()
        );
    }
}

/// `config` is read before each meeting; a pass stops when the folder setting moves.
pub fn tidy(folder: &Path, config: impl Fn() -> MeetingsConfig, now_ms: i64) {
    for recorded in transcribed_with_audio(folder) {
        let config = config();
        if config.folder() != folder {
            return;
        }
        settle(&recorded, &config, now_ms);
    }
}

/// A track name from meeting.json must stay inside the meeting's audio folder.
fn plain_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

/// WAVs are deleted only after meeting.json points at the FLACs, so a crash leaves both, never neither.
pub fn compress(recorded: &RecordedMeeting) -> anyhow::Result<()> {
    let mut meeting = recorded.meeting.clone();
    let mut changed = false;
    for track in &mut meeting.tracks {
        if !plain_name(&track.file) {
            anyhow::bail!("track {:?} is not a file name", track.file);
        }
        if !track.file.ends_with(".wav") {
            // A WAV left beside its FLAC by an earlier pass that stopped after meeting.json moved on.
            let wav = recorded
                .dir
                .join(AUDIO_DIR)
                .join(format!("{}.wav", track.file.trim_end_matches(".flac")));
            if track.file.ends_with(".flac")
                && recorded.track_path(track).is_file()
                && wav.is_file()
            {
                let _ = std::fs::remove_file(wav);
            }
            continue;
        }
        let wav = recorded.track_path(track);
        if !wav.exists() {
            continue;
        }
        let flac_name = format!("{}.flac", track.file.trim_end_matches(".wav"));
        let flac = recorded.dir.join(AUDIO_DIR).join(&flac_name);
        write_flac(&wav, &flac)?;
        track.file = flac_name;
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    let partial = recorded.dir.join(format!("{MEETING_FILE}.part"));
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&partial)?;
        file.write_all(serde_json::to_string_pretty(&meeting)?.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&partial, recorded.dir.join(MEETING_FILE))?;
    for track in &recorded.meeting.tracks {
        if track.file.ends_with(".wav") {
            let _ = std::fs::remove_file(recorded.track_path(track));
        }
    }
    eprintln!(
        "meetings: compressed the audio of {}",
        recorded.dir.display()
    );
    Ok(())
}

fn write_flac(wav: &Path, flac: &Path) -> anyhow::Result<()> {
    let partial = flac.with_extension("flac.part");
    let written = encode_flac(wav, &partial).and_then(|()| Ok(std::fs::rename(&partial, flac)?));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written
}

/// Encodes and writes one frame at a time: memory stays at one block whatever the track's length.
fn encode_flac(wav: &Path, partial: &Path) -> anyhow::Result<()> {
    use std::io::Write;

    use flacenc::component::{BitRepr, Stream};
    use flacenc::error::Verify;
    use flacenc::source::{Context, Fill, FrameBuf};

    let mut reader =
        hound::WavReader::open(wav).with_context(|| format!("cannot open {}", wav.display()))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.bits_per_sample != 16 || spec.sample_rate != SAMPLE_RATE {
        anyhow::bail!("{} is not a 16 kHz mono 16-bit track", wav.display());
    }
    let flac_err = |e: &dyn std::fmt::Debug| anyhow::anyhow!("flac encoding: {e:?}");
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| flac_err(&e))?;
    let block_size = config.block_size;
    let mut stream = Stream::new(SAMPLE_RATE as usize, 1, 16).map_err(|e| flac_err(&e))?;
    let info = stream.stream_info_mut();
    info.set_block_sizes(block_size, block_size)
        .map_err(|e| flac_err(&e))?;
    info.set_total_samples(reader.len() as usize);

    let file = std::fs::File::create(partial)
        .with_context(|| format!("cannot write {}", partial.display()))?;
    let mut out = std::io::BufWriter::new(file);
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| flac_err(&e))?;
    out.write_all(sink.as_slice())?;

    let mut buffers = (
        FrameBuf::with_size(1, block_size).map_err(|e| flac_err(&e))?,
        Context::new(16, 1),
    );
    let mut block: Vec<i32> = Vec::with_capacity(block_size);
    let mut samples = reader.samples::<i16>();
    loop {
        block.clear();
        for sample in samples.by_ref().take(block_size) {
            block.push(i32::from(sample?));
        }
        if block.is_empty() {
            break;
        }
        buffers.fill_interleaved(&block).map_err(|e| flac_err(&e))?;
        let number = buffers.1.current_frame_number().unwrap_or_default();
        let frame =
            flacenc::encode_fixed_size_frame(&config, &buffers.0, number, stream.stream_info())
                .map_err(|e| flac_err(&e))?;
        sink.clear();
        frame.write(&mut sink).map_err(|e| flac_err(&e))?;
        out.write_all(sink.as_slice())?;
    }
    out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{MeetingFile, TrackFile};

    fn meeting(folder: &Path, name: &str, ended_ms: i64, transcribed: bool) -> RecordedMeeting {
        let dir = folder.join(name);
        std::fs::create_dir_all(dir.join(AUDIO_DIR)).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::create(dir.join(AUDIO_DIR).join("01-a.wav"), spec).unwrap();
        // A minute that is mostly silence with a little tone, like a real track.
        for i in 0..SAMPLE_RATE * 60 {
            let s = if i < SAMPLE_RATE {
                ((i % 50) as i16 - 25) * 200
            } else {
                0
            };
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
        let meeting = MeetingFile {
            started_ms: ended_ms - 60_000,
            ended_ms,
            zones: vec!["Gold Office".into()],
            tracks: vec![TrackFile {
                id: "a".into(),
                name: "Amy".into(),
                me: false,
                file: "01-a.wav".into(),
            }],
        };
        std::fs::write(
            dir.join(MEETING_FILE),
            serde_json::to_string(&meeting).unwrap(),
        )
        .unwrap();
        if transcribed {
            std::fs::write(dir.join(TRANSCRIPT_FILE), "# done").unwrap();
        }
        RecordedMeeting::load(&dir).unwrap()
    }

    const NOW: i64 = 100 * DAY_MS;

    #[test]
    fn kept_audio_is_compressed_and_expires() {
        let folder = tempfile::tempdir().unwrap();
        let fresh = meeting(folder.path(), "fresh", NOW - DAY_MS, true);
        let old = meeting(folder.path(), "old", NOW - 8 * DAY_MS, true);
        let pending = meeting(folder.path(), "pending", NOW - 30 * DAY_MS, false);
        let before = audio_usage(folder.path());
        assert_eq!(before.meetings, 3);
        let config = MeetingsConfig {
            keep_audio: true,
            ..MeetingsConfig::default()
        };
        let current = || MeetingsConfig {
            folder: Some(folder.path().to_path_buf()),
            audio_days: 7,
            ..config.clone()
        };
        tidy(folder.path(), current, NOW);

        assert!(!old.dir.join(AUDIO_DIR).exists());
        assert!(pending.dir.join(AUDIO_DIR).join("01-a.wav").exists());
        let fresh = RecordedMeeting::load(&fresh.dir).unwrap();
        assert_eq!(fresh.meeting.tracks[0].file, "01-a.flac");
        let flac = fresh.track_path(&fresh.meeting.tracks[0]);
        assert!(flac.exists());
        assert!(!fresh.dir.join(AUDIO_DIR).join("01-a.wav").exists());
        let mut decoded = claxon::FlacReader::open(&flac).unwrap();
        assert_eq!(
            decoded.streaminfo().samples,
            Some(u64::from(SAMPLE_RATE) * 60)
        );
        let samples: Vec<i32> = decoded.samples().map(Result::unwrap).collect();
        assert_eq!(samples.len(), SAMPLE_RATE as usize * 60);
        assert_eq!(samples[49], (49 - 25) * 200);
        assert!(samples[SAMPLE_RATE as usize..].iter().all(|&s| s == 0));
        let flac_size = std::fs::metadata(&flac).unwrap().len();
        assert!(flac_size * 10 < 60 * 2 * SAMPLE_RATE as u64, "{flac_size}");
        assert_eq!(audio_usage(folder.path()).meetings, 2);

        std::fs::write(fresh.dir.join(AUDIO_DIR).join("01-a.wav"), b"left over").unwrap();
        tidy(
            folder.path(),
            || MeetingsConfig {
                audio_days: 0,
                ..current()
            },
            NOW + 1_000 * DAY_MS,
        );
        assert!(flac.exists());
        assert!(!fresh.dir.join(AUDIO_DIR).join("01-a.wav").exists());

        tidy(
            folder.path(),
            || MeetingsConfig {
                audio_days: u64::MAX,
                ..current()
            },
            NOW,
        );
        assert!(flac.exists());
    }

    #[test]
    fn a_damaged_track_stays_as_wav_with_no_partial_file() {
        let folder = tempfile::tempdir().unwrap();
        let recorded = meeting(folder.path(), "cut", NOW, true);
        let wav = recorded.dir.join(AUDIO_DIR).join("01-a.wav");
        let file = std::fs::OpenOptions::new().write(true).open(&wav).unwrap();
        file.set_len(100_000).unwrap();
        assert!(compress(&recorded).is_err());
        let left: Vec<String> = std::fs::read_dir(recorded.dir.join(AUDIO_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["01-a.wav"]);
        assert_eq!(
            RecordedMeeting::load(&recorded.dir).unwrap().meeting.tracks[0].file,
            "01-a.wav"
        );
    }

    #[test]
    fn without_keep_audio_or_on_request_the_audio_goes() {
        let folder = tempfile::tempdir().unwrap();
        let done = meeting(folder.path(), "done", NOW, true);
        let pending = meeting(folder.path(), "pending", NOW, false);
        let config = || MeetingsConfig {
            folder: Some(folder.path().to_path_buf()),
            ..MeetingsConfig::default()
        };
        tidy(folder.path(), config, NOW);
        assert!(!done.dir.join(AUDIO_DIR).exists());
        assert!(pending.dir.join(AUDIO_DIR).exists());

        let kept = meeting(folder.path(), "kept", NOW, true);
        assert_eq!(delete_audio(folder.path()).unwrap(), 1);
        assert!(!kept.dir.join(AUDIO_DIR).exists());
        assert!(pending.dir.join(AUDIO_DIR).exists());
    }
}
