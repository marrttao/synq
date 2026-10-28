//! The Whisper model itself (whisper-rs / whisper.cpp).

use std::path::Path;
use std::sync::Once;

use thiserror::Error;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::filter::{RawSegment, RawToken};

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("cannot load the model: {0}")]
    Load(String),
    #[error("recognition failed: {0}")]
    Run(String),
}

pub struct Engine {
    ctx: WhisperContext,
}

impl Engine {
    pub fn load(model: &Path) -> Result<Self, EngineError> {
        // Without this whisper.cpp writes its log to stderr, over the UI.
        static HOOKS: Once = Once::new();
        HOOKS.call_once(whisper_rs::install_logging_hooks);

        let params = || WhisperContextParameters::default();
        // whisper.cpp opens the file itself, which fails on Windows for very
        // long or non-ASCII paths (e.g. a user name in Cyrillic). Then the
        // model is read here and handed over as a buffer instead.
        let by_path = model
            .to_str()
            .and_then(|p| WhisperContext::new_with_params(p, params()).ok());
        let ctx = match by_path {
            Some(ctx) => ctx,
            None => {
                tracing::info!("loading the model through a buffer");
                let bytes = std::fs::read(model)
                    .map_err(|e| EngineError::Load(format!("{}: {e}", model.display())))?;
                WhisperContext::new_from_buffer_with_params(&bytes, params())
                    .map_err(|e| EngineError::Load(e.to_string()))?
            }
        };
        Ok(Self { ctx })
    }

    /// Recognize 16 kHz mono audio. `language` is a code like "en", or
    /// "auto"/"" to detect it. `progress` receives 0..=100.
    pub fn transcribe(
        &self,
        audio: &[f32],
        language: &str,
        threads: usize,
        progress: impl FnMut(i32) + 'static,
    ) -> Result<Vec<RawSegment>, EngineError> {
        let run = |e: whisper_rs::WhisperError| EngineError::Run(e.to_string());
        let mut state = self.ctx.create_state().map_err(run)?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let language = language.trim();
        if language.is_empty() || language.eq_ignore_ascii_case("auto") {
            // "auto" makes whisper.cpp detect the language and go on. The
            // separate `detect_language` flag would stop right after that.
            params.set_language(Some("auto"));
            params.set_detect_language(false);
        } else {
            params.set_language(Some(language));
        }
        params.set_n_threads(threads.clamp(1, 16) as i32);
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_token_timestamps(true);
        // Music invites hallucinated filler; these settings reduce it.
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_no_speech_thold(0.6);
        params.set_progress_callback_safe(progress);

        state.full(params, audio).map_err(run)?;

        let mut out = Vec::new();
        for seg in state.as_iter() {
            // Timestamps are in centiseconds; negative ones are bogus.
            let ms = |cs: i64| cs.max(0) as u64 * 10;
            let mut tokens = Vec::new();
            for i in 0..seg.n_tokens() {
                let Some(tok) = seg.get_token(i) else {
                    continue;
                };
                let Ok(text) = tok.to_str_lossy() else {
                    continue;
                };
                tokens.push(RawToken {
                    text: text.into_owned(),
                    start_ms: ms(tok.token_data().t0),
                });
            }
            out.push(RawSegment {
                start_ms: ms(seg.start_timestamp()),
                end_ms: ms(seg.end_timestamp()),
                text: seg.to_str_lossy().map_err(run)?.into_owned(),
                no_speech_prob: seg.no_speech_probability(),
                tokens,
            });
        }
        Ok(out)
    }
}
