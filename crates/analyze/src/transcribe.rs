//! Running Whisper on one chunk of speech at a time.
//!
//! Each chunk is decoded on its own (`no_context`), with a prompt built here: the
//! keyword hint, then the end of the text heard so far. whisper.cpp would drop the
//! hint once its own rolling context fills up, and whisper-rs 0.16 cannot set
//! `carry_initial_prompt`, so the prompt is assembled by hand to keep names spelled
//! right all through the file.

use std::os::raw::c_int;
use std::path::Path;

use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters, WhisperState, WhisperTokenId,
};

use crate::Language;
use crate::audio::SAMPLE_RATE;
use crate::models::ModelSize;

/// Whisper's prompt holds at most this many tokens: half its 448-token text context,
/// less the token that marks the prompt.
const MAX_PROMPT_TOKENS: usize = 223;
/// The keyword hint may fill at most half of the prompt.
pub(crate) const MAX_KEYWORD_TOKENS: usize = 111;
/// A chunk shorter than this gets no previous text in its prompt: with little audio,
/// Whisper tends to repeat the prompt (whisper.cpp drops it in the same case).
const MIN_CONTEXT_SAMPLES: usize = 5 * SAMPLE_RATE;
const SAMPLES_PER_CENTISECOND: i64 = (SAMPLE_RATE / 100) as i64;

/// How the model is loaded and run.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    /// CPU threads for Whisper.
    pub threads: u16,
    /// whisper.cpp's flash attention. Ignored when `dtw` is set.
    pub flash_attn: bool,
    /// Word starts from dynamic time warping, with the alignment heads of this model.
    pub dtw: Option<ModelSize>,
}

/// A word in a chunk, in samples of the chunk.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChunkWord {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub probability: f32,
}

pub(crate) struct Engine {
    context: WhisperContext,
    state: WhisperState,
    threads: c_int,
    dtw: bool,
    end_of_text: WhisperTokenId,
    keywords: Vec<WhisperTokenId>,
    heard: Vec<WhisperTokenId>,
}

impl Engine {
    pub(crate) fn load(model: &Path, options: Options) -> Result<Self, String> {
        let mut params = WhisperContextParameters::default();
        params
            .use_gpu(false)
            .flash_attn(options.flash_attn && options.dtw.is_none());
        if let Some(size) = options.dtw {
            params.dtw_parameters(DtwParameters {
                mode: DtwMode::ModelPreset {
                    model_preset: dtw_preset(size),
                },
                ..DtwParameters::default()
            });
        }
        let context = WhisperContext::new_with_params(model, params)
            .map_err(|error| format!("cannot load the model: {error}"))?;
        let state = context
            .create_state()
            .map_err(|error| format!("cannot set up the model: {error}"))?;
        let end_of_text = context.token_eot();
        Ok(Self {
            context,
            state,
            threads: c_int::from(options.threads.max(1)),
            dtw: options.dtw.is_some(),
            end_of_text,
            keywords: Vec::new(),
            heard: Vec::new(),
        })
    }

    /// Sets the hint that helps Whisper spell names and terms, like
    /// "Supabase, Rust, Dinah.". Returns how many tokens it takes.
    pub(crate) fn set_keywords(&mut self, keywords: &[String]) -> Result<usize, String> {
        self.keywords.clear();
        if keywords.is_empty() {
            return Ok(0);
        }
        let hint = format!("{}.", keywords.join(", "));
        // Every token covers at least one byte, so this buffer is always big enough.
        let tokens = self
            .context
            .tokenize(&hint, hint.len() + 8)
            .map_err(|error| format!("cannot read the keywords: {error}"))?;
        if tokens.len() > MAX_KEYWORD_TOKENS {
            return Err(format!(
                "the keywords take {} tokens, and the model's hint holds {MAX_KEYWORD_TOKENS}: \
                 use fewer or shorter keywords",
                tokens.len()
            ));
        }
        self.keywords = tokens;
        Ok(self.keywords.len())
    }

    /// Tells English from French in `audio`, with the probability of the answer
    /// relative to the other language.
    pub(crate) fn detect_language(&mut self, audio: &[f32]) -> Result<(Language, f32), String> {
        let threads = usize::try_from(self.threads).unwrap_or(1);
        self.state
            .pcm_to_mel(audio, threads)
            .map_err(|error| format!("cannot analyse the audio: {error}"))?;
        let (_, probabilities) = self
            .state
            .lang_detect(0, threads)
            .map_err(|error| format!("cannot detect the language: {error}"))?;
        let probability = |language: Language| {
            whisper_rs::get_lang_id(language.code())
                .and_then(|id| usize::try_from(id).ok())
                .and_then(|id| probabilities.get(id).copied())
                .unwrap_or(0.0)
        };
        let (english, french) = (probability(Language::En), probability(Language::Fr));
        let (language, chosen) = if french > english {
            (Language::Fr, french)
        } else {
            (Language::En, english)
        };
        let total = english + french;
        Ok((language, if total > 0.0 { chosen / total } else { 0.5 }))
    }

    /// Transcribes one chunk. Word times are in samples of the chunk.
    pub(crate) fn transcribe(
        &mut self,
        audio: &[f32],
        language: Language,
    ) -> Result<Vec<ChunkWord>, String> {
        let mut prompt = self.keywords.clone();
        if audio.len() >= MIN_CONTEXT_SAMPLES {
            let room = MAX_PROMPT_TOKENS.saturating_sub(prompt.len());
            let skip = self.heard.len().saturating_sub(room);
            prompt.extend(self.heard.iter().skip(skip));
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_language(Some(language.code()));
        params.set_translate(false);
        params.set_no_context(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        // One word per segment, with token-level timestamps.
        params.set_token_timestamps(true);
        params.set_max_len(1);
        params.set_split_on_word(true);
        if !prompt.is_empty() {
            params.set_tokens(&prompt);
        }
        self.state
            .full(params, audio)
            .map_err(|error| format!("transcription failed: {error}"))?;
        self.collect()
    }

    fn collect(&mut self) -> Result<Vec<ChunkWord>, String> {
        let mut words = Vec::new();
        let mut tokens = Vec::new();
        // whisper.cpp gives each token the time at which the alignment leaves it
        // (`t_dtw`), which is where the next token starts.
        let mut next_token_start: Option<i64> = None;
        for segment in self.state.as_iter() {
            let text = segment
                .to_str_lossy()
                .map_err(|error| format!("cannot read the transcript: {error}"))?;
            let mut probability_sum = 0.0_f32;
            let mut count = 0_u16;
            let mut dtw_start = None;
            for index in 0..segment.n_tokens() {
                let Some(token) = segment.get_token(index) else {
                    continue;
                };
                let data = token.token_data();
                if data.id >= self.end_of_text {
                    continue;
                }
                tokens.push(data.id);
                if count == 0 {
                    dtw_start = next_token_start;
                }
                probability_sum += data.p;
                count = count.saturating_add(1);
                next_token_start = (data.t_dtw >= 0).then_some(data.t_dtw);
            }
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let start = match dtw_start {
                Some(start) if self.dtw => start,
                _ => segment.start_timestamp(),
            };
            let end = segment.end_timestamp().max(start);
            words.push(ChunkWord {
                text: text.to_owned(),
                start: to_samples(start),
                end: to_samples(end),
                probability: if count > 0 {
                    probability_sum / f32::from(count)
                } else {
                    0.0
                },
            });
        }
        self.heard.extend(tokens);
        let excess = self.heard.len().saturating_sub(MAX_PROMPT_TOKENS);
        self.heard.drain(..excess);
        Ok(words)
    }
}

fn to_samples(centiseconds: i64) -> usize {
    usize::try_from(centiseconds.max(0).saturating_mul(SAMPLES_PER_CENTISECOND)).unwrap_or(0)
}

fn dtw_preset(size: ModelSize) -> DtwModelPreset {
    match size {
        ModelSize::Tiny => DtwModelPreset::Tiny,
        ModelSize::Base => DtwModelPreset::Base,
        ModelSize::Small => DtwModelPreset::Small,
    }
}
