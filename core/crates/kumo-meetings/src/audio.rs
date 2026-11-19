pub const SAMPLE_RATE: u32 = 16_000;

const FRAME: usize = 320;
const SPEECH_RMS: f32 = 0.012;
const MERGE_GAP_FRAMES: usize = 40;
const PAD_FRAMES: usize = 15;
const MIN_SPEECH_FRAMES: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speaker {
    pub id: String,
    pub name: String,
    pub me: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub speaker: Speaker,
    pub at_ms: i64,
    pub samples: Vec<i16>,
}

impl Chunk {
    pub fn duration_ms(&self) -> i64 {
        samples_to_ms(self.samples.len())
    }

    pub fn end_ms(&self) -> i64 {
        self.at_ms + self.duration_ms()
    }

    pub fn speech_ms(&self) -> i64 {
        let frames = self
            .samples
            .chunks(FRAME)
            .filter(|frame| rms_i16(frame) >= SPEECH_RMS)
            .count();
        samples_to_ms(frames * FRAME).min(self.duration_ms())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
}

pub fn samples_to_ms(samples: usize) -> i64 {
    (samples as i64) * 1_000 / SAMPLE_RATE as i64
}

pub fn ms_to_samples(ms: i64) -> usize {
    usize::try_from(ms.max(0) * SAMPLE_RATE as i64 / 1_000).unwrap_or(0)
}

pub fn to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|s| f32::from(*s) / 32_768.0).collect()
}

pub fn speech_regions(samples: &[f32]) -> Vec<Region> {
    let loud: Vec<bool> = samples
        .chunks(FRAME)
        .map(|frame| rms(frame) >= SPEECH_RMS)
        .collect();
    let mut regions: Vec<(usize, usize, usize)> = Vec::new();
    for (i, is_loud) in loud.iter().enumerate() {
        if !is_loud {
            continue;
        }
        match regions.last_mut() {
            Some((_, end, count)) if i - *end <= MERGE_GAP_FRAMES => {
                *end = i + 1;
                *count += 1;
            }
            _ => regions.push((i, i + 1, 1)),
        }
    }
    regions
        .into_iter()
        .filter(|(_, _, count)| *count >= MIN_SPEECH_FRAMES)
        .map(|(start, end, _)| Region {
            start: start.saturating_sub(PAD_FRAMES) * FRAME,
            end: ((end + PAD_FRAMES) * FRAME).min(samples.len()),
        })
        .fold(Vec::new(), |mut merged: Vec<Region>, region| {
            match merged.last_mut() {
                Some(last) if region.start <= last.end => last.end = region.end,
                _ => merged.push(region),
            }
            merged
        })
}

fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

fn rms_i16(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f32 = frame
        .iter()
        .map(|s| {
            let v = f32::from(*s) / 32_768.0;
            v * v
        })
        .sum();
    (sum / frame.len() as f32).sqrt()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn tone(ms: i64, level: f32) -> Vec<f32> {
        (0..ms_to_samples(ms))
            .map(|i| level * (i as f32 * 0.2).sin())
            .collect()
    }

    pub fn tone_i16(ms: i64, level: f32) -> Vec<i16> {
        tone(ms, level)
            .into_iter()
            .map(|v| (v * 32_767.0) as i16)
            .collect()
    }

    fn concat(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    #[test]
    fn silence_has_no_regions() {
        assert!(speech_regions(&tone(3_000, 0.0)).is_empty());
        assert!(speech_regions(&tone(3_000, 0.002)).is_empty());
    }

    #[test]
    fn finds_speech_with_some_padding_around_it() {
        let audio = concat(&[tone(2_000, 0.0), tone(1_000, 0.3), tone(2_000, 0.0)]);
        let regions = speech_regions(&audio);
        assert_eq!(regions.len(), 1);
        let r = regions[0];
        assert!(samples_to_ms(r.start) >= 1_600 && samples_to_ms(r.start) <= 2_000);
        assert!(samples_to_ms(r.end) >= 3_000 && samples_to_ms(r.end) <= 3_400);
    }

    #[test]
    fn short_pauses_stay_inside_one_region_long_ones_split_it() {
        let audio = concat(&[
            tone(1_000, 0.3),
            tone(500, 0.0),
            tone(1_000, 0.3),
            tone(3_000, 0.0),
            tone(1_000, 0.3),
        ]);
        assert_eq!(speech_regions(&audio).len(), 2);
    }

    #[test]
    fn clicks_are_not_speech() {
        let audio = concat(&[tone(1_000, 0.0), tone(100, 0.5), tone(1_000, 0.0)]);
        assert!(speech_regions(&audio).is_empty());
    }

    #[test]
    fn chunk_speech_is_measured_by_loud_frames() {
        let speaker = Speaker {
            id: "a".into(),
            name: "Anna".into(),
            me: false,
        };
        let loud = Chunk {
            speaker: speaker.clone(),
            at_ms: 0,
            samples: tone_i16(100, 0.3),
        };
        let quiet = Chunk {
            speaker,
            at_ms: 0,
            samples: tone_i16(100, 0.001),
        };
        assert_eq!(loud.duration_ms(), 100);
        assert_eq!(loud.speech_ms(), 100);
        assert_eq!(quiet.speech_ms(), 0);
    }
}
