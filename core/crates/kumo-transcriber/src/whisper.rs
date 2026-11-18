use std::path::Path;

use anyhow::Context;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use kumo_meetings::transcript::{Segment, SpeechToText};

const NO_SPEECH: f32 = 0.6;

pub struct Whisper {
    state: WhisperState,
    language: String,
    threads: i32,
}

impl Whisper {
    pub fn load(model: &Path, language: &str) -> anyhow::Result<Self> {
        whisper_rs::install_logging_hooks();
        let path = model.to_str().context("the model path is not UTF-8")?;
        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("cannot load the speech model {}", model.display()))?;
        let state = context
            .create_state()
            .context("cannot start the speech model")?;
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(4) as i32;
        Ok(Self {
            state,
            language: language.trim().to_lowercase(),
            threads,
        })
    }
}

impl SpeechToText for Whisper {
    fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Segment>> {
        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        });
        let language = if self.language.is_empty() {
            "auto"
        } else {
            self.language.as_str()
        };
        params.set_language(Some(language));
        params.set_n_threads(self.threads);
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        self.state
            .full(params, samples)
            .context("speech recognition failed")?;
        Ok(self
            .state
            .as_iter()
            .filter(|s| s.no_speech_probability() < NO_SPEECH)
            .filter_map(|s| {
                Some(Segment {
                    start_ms: s.start_timestamp() * 10,
                    end_ms: s.end_timestamp() * 10,
                    text: s.to_str_lossy().ok()?.into_owned(),
                })
            })
            .collect())
    }
}
