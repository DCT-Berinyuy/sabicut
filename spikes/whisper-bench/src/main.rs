//! Sabicut day-one Whisper benchmark harness.
//!
//! Runs exactly one transcription per process, so the process's peak RSS reflects one model run.
//! Word timestamps use whisper.cpp token timestamps with `max_len = 1` and `split_on_word`,
//! which makes every output segment a single word. VAD runs in the application (see `vad.rs`),
//! because whisper-rs 0.16 cannot reach whisper.cpp's built-in VAD; every timestamp is then
//! mapped back to the original audio timeline.

mod vad;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WHISPER_CPP_VERSION,
    WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::vad::VadAudio;

const SAMPLE_RATE: u32 = 16_000;
const USAGE: &str = "usage: whisper-bench --model <ggml.bin> --audio <16k-mono.wav> --lang <en|fr> \
--out <prefix> [--threads 4] [--vad-model <ggml-silero.bin>] [--vad-threads 1] \
[--dtw tiny|base|small] [--flash-attn] [--beam <n>]";

struct Args {
    model: PathBuf,
    audio: PathBuf,
    lang: String,
    out_prefix: PathBuf,
    threads: i32,
    vad_model: Option<PathBuf>,
    vad_threads: i32,
    dtw: Option<String>,
    flash_attn: bool,
    beam_size: i32,
}

struct Word {
    seg_t0_cs: i64,
    seg_t1_cs: i64,
    tok_t0_cs: Option<i64>,
    tok_t1_cs: Option<i64>,
    dtw_cs: Option<i64>,
    p_mean: f32,
    text: String,
}

struct Timings {
    audio_s: f64,
    load_ms: f64,
    vad_ms: f64,
    full_ms: f64,
    total_ms: f64,
}

fn next_value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_args() -> Result<Args, String> {
    let (mut model, mut audio, mut lang, mut out_prefix) = (None, None, None, None);
    let (mut threads, mut vad_model, mut vad_threads) = (4, None, 1);
    let (mut dtw, mut flash_attn, mut beam_size) = (None, false, 0);
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--model" => model = Some(PathBuf::from(next_value(&mut it, &flag)?)),
            "--audio" => audio = Some(PathBuf::from(next_value(&mut it, &flag)?)),
            "--lang" => lang = Some(next_value(&mut it, &flag)?),
            "--out" => out_prefix = Some(PathBuf::from(next_value(&mut it, &flag)?)),
            "--threads" => threads = parse_int(&next_value(&mut it, &flag)?, &flag)?,
            "--vad-model" => vad_model = Some(PathBuf::from(next_value(&mut it, &flag)?)),
            "--vad-threads" => vad_threads = parse_int(&next_value(&mut it, &flag)?, &flag)?,
            "--dtw" => dtw = Some(next_value(&mut it, &flag)?),
            "--flash-attn" => flash_attn = true,
            "--beam" => beam_size = parse_int(&next_value(&mut it, &flag)?, &flag)?,
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown flag {other}\n{USAGE}")),
        }
    }
    let missing = |name: &str| format!("missing {name}\n{USAGE}");
    Ok(Args {
        model: model.ok_or_else(|| missing("--model"))?,
        audio: audio.ok_or_else(|| missing("--audio"))?,
        lang: lang.ok_or_else(|| missing("--lang"))?,
        out_prefix: out_prefix.ok_or_else(|| missing("--out"))?,
        threads,
        vad_model,
        vad_threads,
        dtw,
        flash_attn,
        beam_size,
    })
}

fn parse_int(value: &str, flag: &str) -> Result<i32, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects an integer, got {value:?}"))
}

fn dtw_preset(name: &str) -> Result<DtwModelPreset, String> {
    match name {
        "tiny" => Ok(DtwModelPreset::Tiny),
        "base" => Ok(DtwModelPreset::Base),
        "small" => Ok(DtwModelPreset::Small),
        other => Err(format!(
            "unsupported --dtw preset {other:?} (use tiny, base or small)"
        )),
    }
}

fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE || spec.channels != 1 {
        return Err(format!(
            "{}: need 16 kHz mono, got {} Hz with {} channels",
            path.display(),
            spec.sample_rate,
            spec.channels
        ));
    }
    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| f32::from(v) / 32768.0))
            .collect::<Result<Vec<_>, _>>(),
        (hound::SampleFormat::Float, 32) => reader.samples::<f32>().collect(),
        (format, bits) => {
            return Err(format!(
                "unsupported WAV sample format {format:?} {bits}-bit"
            ));
        }
    };
    samples.map_err(|e| format!("decode {}: {e}", path.display()))
}

/// Reads a kB field such as `VmHWM` (peak RSS) from `/proc/self/status`.
fn proc_status_kb(field: &str) -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix(field)?.strip_prefix(':')?;
        rest.trim().trim_end_matches("kB").trim().parse().ok()
    })
}

fn load_context(args: &Args) -> Result<WhisperContext, String> {
    let mut ctx_params = WhisperContextParameters::default();
    ctx_params.use_gpu(false).flash_attn(args.flash_attn);
    if let Some(name) = &args.dtw {
        ctx_params.dtw_parameters(DtwParameters {
            mode: DtwMode::ModelPreset {
                model_preset: dtw_preset(name)?,
            },
            ..DtwParameters::default()
        });
    }
    WhisperContext::new_with_params(&args.model, ctx_params)
        .map_err(|e| format!("load {}: {e:?}", args.model.display()))
}

fn full_params(args: &Args) -> FullParams<'_, '_> {
    let strategy = if args.beam_size > 1 {
        SamplingStrategy::BeamSearch {
            beam_size: args.beam_size,
            patience: -1.0,
        }
    } else {
        SamplingStrategy::Greedy { best_of: 1 }
    };
    let mut params = FullParams::new(strategy);
    params.set_n_threads(args.threads);
    params.set_language(Some(&args.lang));
    params.set_translate(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_token_timestamps(true);
    params.set_max_len(1);
    params.set_split_on_word(true);
    params
}

fn collect_words(
    ctx: &WhisperContext,
    state: &WhisperState,
    to_original: &dyn Fn(i64) -> i64,
) -> Result<(String, Vec<Word>), String> {
    let eot = ctx.token_eot();
    let mut transcript = String::new();
    let mut words = Vec::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .map_err(|e| format!("segment text: {e:?}"))?;
        transcript.push_str(&text);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (mut tok_t0_cs, mut tok_t1_cs, mut dtw_cs) = (None, None, None);
        let (mut p_sum, mut p_count) = (0.0_f32, 0_u16);
        for i in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(i) else {
                continue;
            };
            let data = token.token_data();
            if data.id >= eot {
                continue;
            }
            tok_t0_cs.get_or_insert(to_original(data.t0));
            tok_t1_cs = Some(to_original(data.t1));
            if data.t_dtw >= 0 {
                dtw_cs.get_or_insert(to_original(data.t_dtw));
            }
            p_sum += data.p;
            p_count += 1;
        }
        words.push(Word {
            seg_t0_cs: to_original(segment.start_timestamp()),
            seg_t1_cs: to_original(segment.end_timestamp()),
            tok_t0_cs,
            tok_t1_cs,
            dtw_cs,
            p_mean: if p_count > 0 {
                p_sum / f32::from(p_count)
            } else {
                0.0
            },
            text: trimmed.to_owned(),
        });
    }
    Ok((transcript.trim().to_owned(), words))
}

fn opt_ms(cs: Option<i64>) -> String {
    cs.map_or_else(|| "-1".to_owned(), |v| (v * 10).to_string())
}

fn write_outputs(args: &Args, transcript: &str, words: &[Word]) -> Result<(), String> {
    let with_ext = |ext: &str| {
        let mut path = args.out_prefix.clone().into_os_string();
        path.push(ext);
        PathBuf::from(path)
    };
    fs::write(with_ext(".txt"), format!("{transcript}\n")).map_err(|e| e.to_string())?;
    let file = File::create(with_ext(".words.tsv")).map_err(|e| e.to_string())?;
    let mut tsv = BufWriter::new(file);
    let io = |e: std::io::Error| e.to_string();
    writeln!(
        tsv,
        "seg_t0_ms\tseg_t1_ms\ttok_t0_ms\ttok_t1_ms\tdtw_ms\tp\tword"
    )
    .map_err(io)?;
    for w in words {
        writeln!(
            tsv,
            "{}\t{}\t{}\t{}\t{}\t{:.3}\t{}",
            w.seg_t0_cs * 10,
            w.seg_t1_cs * 10,
            opt_ms(w.tok_t0_cs),
            opt_ms(w.tok_t1_cs),
            opt_ms(w.dtw_cs),
            w.p_mean,
            w.text.replace('\t', " ")
        )
        .map_err(io)?;
    }
    tsv.flush().map_err(io)
}

fn json_str(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn metrics_json(
    args: &Args,
    model_type: &str,
    t: &Timings,
    vad: Option<&VadAudio>,
    n_words: usize,
) -> String {
    let pipeline_ms = t.vad_ms + t.full_ms;
    let fields = [
        ("model", json_str(&args.model.display().to_string())),
        ("model_type", json_str(model_type)),
        ("lang", json_str(&args.lang)),
        ("threads", args.threads.to_string()),
        ("vad", vad.is_some().to_string()),
        ("vad_threads", args.vad_threads.to_string()),
        ("vad_segments", vad.map_or(0, |v| v.n_segments).to_string()),
        (
            "speech_s",
            format!("{:.2}", vad.map_or(t.audio_s, VadAudio::speech_s)),
        ),
        ("dtw", args.dtw.is_some().to_string()),
        ("flash_attn", args.flash_attn.to_string()),
        ("beam", args.beam_size.to_string()),
        ("audio_s", format!("{:.2}", t.audio_s)),
        ("load_ms", format!("{:.0}", t.load_ms)),
        ("vad_ms", format!("{:.0}", t.vad_ms)),
        ("full_ms", format!("{:.0}", t.full_ms)),
        ("pipeline_ms", format!("{pipeline_ms:.0}")),
        ("total_ms", format!("{:.0}", t.total_ms)),
        ("rtf", format!("{:.4}", pipeline_ms / 1000.0 / t.audio_s)),
        ("n_words", n_words.to_string()),
        (
            "vm_hwm_kb",
            proc_status_kb("VmHWM").unwrap_or(0).to_string(),
        ),
        ("whisper_cpp_version", json_str(WHISPER_CPP_VERSION)),
    ];
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    format!("{{{}}}", body.join(","))
}

fn run(args: &Args) -> Result<(), String> {
    let t_process = Instant::now();
    let samples = read_wav(&args.audio)?;
    let audio_s = samples.len() as f64 / f64::from(SAMPLE_RATE);

    let t_load = Instant::now();
    let ctx = load_context(args)?;
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("create state: {e:?}"))?;
    let load_ms = t_load.elapsed().as_secs_f64() * 1000.0;

    let vad = match &args.vad_model {
        Some(path) => {
            let path = path.to_str().ok_or("VAD model path is not valid UTF-8")?;
            Some(vad::filter_speech(path, args.vad_threads, &samples)?)
        }
        None => None,
    };
    let audio = vad
        .as_ref()
        .map_or(samples.as_slice(), |v| v.samples.as_slice());
    let t_full = Instant::now();
    if !audio.is_empty() {
        state
            .full(full_params(args), audio)
            .map_err(|e| format!("transcribe: {e:?}"))?;
    }
    let full_ms = t_full.elapsed().as_secs_f64() * 1000.0;

    let to_original = |cs: i64| vad.as_ref().map_or(cs, |v| v.to_original_cs(cs));
    let (transcript, words) = collect_words(&ctx, &state, &to_original)?;
    write_outputs(args, &transcript, &words)?;
    let timings = Timings {
        audio_s,
        load_ms,
        vad_ms: vad.as_ref().map_or(0.0, |v| v.vad_ms),
        full_ms,
        total_ms: t_process.elapsed().as_secs_f64() * 1000.0,
    };
    let model_type = ctx.model_type_readable_str_lossy().unwrap_or_default();
    let json = metrics_json(args, &model_type, &timings, vad.as_ref(), words.len());
    let mut json_path = args.out_prefix.clone().into_os_string();
    json_path.push(".json");
    fs::write(&json_path, format!("{json}\n")).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

fn main() -> ExitCode {
    let result = parse_args().and_then(|args| run(&args));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("whisper-bench: {message}");
            ExitCode::FAILURE
        }
    }
}
