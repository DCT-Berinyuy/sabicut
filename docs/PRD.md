# Sabicut — PRD (v0.3)

> **An offline video studio for communities without laptops or cheap data.**
> A Rust engine turns raw phone clips into captioned vertical shorts. One laptop "hub" serves a whole club over its own Wi-Fi: students upload from their phone browsers and get finished videos back. No internet, no data cost, no app to install.
>
> *Sabi* is Pidgin for "to know / know-how".

Owner: Mr. DCT (DevSafe) · **Status: ACTIVE (time-boxed)** · Updated: 2026-11-05

---

## 0. Time-box and kill date

| Phase | Window | Exit criterion (must be met to continue) |
|---|---|---|
| **Engine v0.1** | 2026-11-02 → 2026-11-16 | One DCT Lab short made with Sabicut and published |
| **Hub v0.2** | 2026-11-16 → 2026-11-30 | With internet **off**, 5 phones upload clips over the hub's Wi-Fi and each gets back a finished short |
| **Pilot** | by 2026-12-07 | One real club session (GDGoC / Cyber Security Club / OWASP Yaoundé) in which at least 10 students each leave with a short |

**Kill/review rule:** if any phase misses its exit criterion by more than 1 week, stop and review before writing more code.

## 1. Problem

Phone-first creators in Cameroon record good raw footage but struggle to turn it into shorts:
- Cloud AI editors need uploads of large raw files over expensive mobile data, plus card-paid USD subscriptions.
- Many students don't own a laptop. Clubs, schools, and cyber cafés usually have **one**.
- Auto-captions handle Pidgin, Camfranglais, and code-switched French/English badly.

CapCut is free and already on their phones; Sabicut will not out-feature it. Sabicut competes where a phone app can't: **zero data cost, shared hardware, local languages, and facilitator-run sessions.**

## 2. Positioning (existing tools checked 2026-11-02)

| Tool | What it already does | Why Sabicut is still different |
|---|---|---|
| auto-editor (5.7k★, since 2020) | Cuts dead air by loudness or motion; exports to Kdenlive and Resolve | Single-user desktop tool; no styled captions; doesn't know about words or retakes |
| CutScript (270★) | Edits video by editing its transcript; Electron + Python | Heavy runtime; single user; not built for low-spec hardware |
| Kinocut (237★) | Local MCP video editor for AI agents; Shorts/Reels; JSON plan format named "Cutfile" | Python, built for agents and developers; shared mobile/web processing is not shipped yet |
| OpenScrub, Cloak | Redact secrets and PII in screen recordings | Secret redaction is already solved, so it's dropped from Sabicut's scope |

**Sabicut's position:** the only one that is (1) a **multi-user hub served over local Wi-Fi to phone browsers**, (2) a **single Rust binary** that runs on a weak laptop, and (3) built around **African-language captions**.

## 3. Users

| User | Device | How they use Sabicut |
|---|---|---|
| **Facilitator** (club lead, teacher, cyber café owner) | One laptop | Runs `sabicut hub`, shows the QR code, manages the queue, wipes the session |
| **Phone-only creator** (student) | Phone browser | Joins the Wi-Fi, uploads clips, types a hook, downloads the short |
| **Laptop creator** (me, DCT Lab) | Laptop | Command-line pipeline: analyze → plan → preview → render |
| **Contributor** | Any | Adds Pidgin/French correction packs and style packs as plain TOML files |

## 4. Goals and non-goals

**Goals**
1. Engine: raw clips → vertical 1080×1920 30 fps MP4 + cover JPG + SRT, **fully offline**, within the RAM budget.
2. A hand-editable **edit plan** (`plan.toml`) with a **frozen, versioned schema**.
3. Hub: a phone-browser upload → queue → download flow on one laptop, no internet.

**Non-goals (until after the pilot):** hosted SaaS, accounts, telemetry; a GUI timeline; MCP server or LLM planner; Tauri desktop app; native mobile app; secret redaction; rea reverse-engineering work; horizontal tutorial mode; sound effects, stickers, confetti.

## 5. The edit plan: `plan.toml` (schema v1, frozen at v0.1)

```toml
schema = 1

[meta]
fps = 30
out = { w = 1080, h = 1920 }

[[source]]
id = "a"
path = "raw/IMG_0412.mov"
crop_x = 0.42                       # centre of the 9:16 window, 0–1

[[cut]]
src = "a"
in = 3.120
out = 9.880
zoom = [{ at = 5.4, scale = 1.25 }] # `at` is in source time

[hook]
text = "Stop leaking your API keys"
dur = 2.0

[captions]
words = "words.json"
fixes = "fixes.toml"
keywords = ["Rust", "Supabase"]
font = "fonts/Anton-Regular.ttf"
active = "#FFC400"
stroke = 10
```

**Schema rules**
- `schema` is an integer. A JSON Schema is generated from the Rust types (`schemars`) and committed as `schema/v1.json`.
- Within v1, only **additive, optional** fields may be added. A breaking change means `schema = 2` plus `sabicut migrate`, and v1 must stay readable. **One deliberate exception (2026-11-04):** when `[captions]` is present and the plan has two or more sources, each source used by a cut must set its own `words`, and `captions.words` is never a fallback; this tightens v1 validation instead of adding a field.
- Output timing is **derived** from the cuts, never stored. Each cut is trimmed to an exact frame count. Mapping source time to output time clamps to the nearest cut and **never panics**.
- Validation errors name the field and the cut index (e.g. `cut[3].out must be > in`).

## 6. Engine v0.1 (ported from the earlier Python pipeline's logic)

| ID | Command | Scope |
|---|---|---|
| F0 | `sabicut validate` / `schema` / `timeline` | Validate a plan, print the JSON Schema, print the derived output timeline |
| F1 | `sabicut analyze` | whisper.cpp (whisper-rs) with word timestamps; voice-activity detection to skip silence → `words.json` |
| F2 | `sabicut fix` | `fixes.toml` replacement rules, reapplied on every run |
| F3 | `sabicut plan` | Rules: cut dead air and false starts (threshold configurable), crop to 9:16, punch-in zooms on keywords → `plan.toml` |
| F4 | `sabicut preview --at 1.5,8,22` | PNG stills with a safe-zone guide, plus a contact sheet. Always runs before a full render |
| F5 | `sabicut render` | One ffmpeg graph: frame-exact trims, short audio fades, concat, end card. Audio: denoise → high-pass → compressor → loudness normalization to −14 LUFS; AAC 160k 48 kHz; x264 CRF 20; fast-start. Also writes the SRT and the cover |
| F6 | Captions and hook | Word-by-word captions (active word gold with a pop, keywords always gold, auto line wrap, platform safe zones); hook text for the first 2 s |

### Day-one benchmark: transcription speed is the main risk

10-minute speech sample, 4 threads, voice-activity detection on, quantized models (`q5_1`). The results go in `docs/bench/whisper.md`.

| Model | Measure | Keep it if |
|---|---|---|
| tiny, base, small | Wall-clock time, peak RSS, rough word error rate on 2 hand-checked minutes (English and French) | Transcribing 10 min takes ≤ 10 min **and** peak RSS < 1.5 GB |

Pick the largest model that passes. Hub mode may use a smaller model than personal mode.

## 7. Hub v0.2

| ID | Feature | Notes |
|---|---|---|
| H1 | `sabicut hub` | axum server on the LAN interface only; prints the URL and a QR code; writes a printable QR poster PNG |
| H2 | Phone upload page | Static page (SvelteKit static build or plain HTML; decided at v0.2), **≤ 100 KB on first load**; resumable chunked uploads that survive Wi-Fi drops |
| H3 | Brief form | Hook text, keywords, language (EN / FR / Pidgin), style choice |
| H4 | Job queue | One job at a time (4 CPUs); shows each phone its queue position and estimated wait |
| H5 | Download | Final MP4 (sized for WhatsApp Status) + SRT; a per-job random token instead of accounts |
| H6 | Facilitator view | Queue, disk usage, retry/cancel, **end session = wipe all uploads**; protected by a session PIN shown on the laptop |
| H7 *(stretch)* | Review on the phone | Preview stills and tap-a-word transcript fixes before rendering |

**Hub security (non-negotiable):** bind to the LAN only; check uploads by magic bytes and size cap, and run an `ffprobe` check before any processing; random job tokens; prevent path traversal; limit request rate per device; make zero outbound network calls; wipe automatically at session end. Students may be minors, so the facilitator confirms consent before a session starts.

**Wi-Fi:** v0.2 documents using the laptop's own hotspot (NetworkManager) or an existing router. Built-in hotspot control (`--hotspot`, reusing the omarchy-hotspot work) comes later.

## 8. Technical design

Crates are added only when they contain real code, so there are no placeholder crates:

```
crates/
  core/     plan types, schema v1, validation, rational time, timeline mapping
  analyze/  whisper-rs, voice-activity detection, loudness
  plan/     rules engine (dead air, false starts, zooms), fixes
  overlay/  tiny-skia + cosmic-text, glyph cache, easing
  render/   ffmpeg graph builder, caption-band pipe, preview stills
  hub/      axum server, queue, uploads, static page
  cli/      clap entry point: `sabicut`
spikes/     throwaway experiments (e.g. whisper-bench), not workspace members
```

| Decision | Choice | Why |
|---|---|---|
| Video I/O | ffmpeg **binary as a subprocess** | Robust; no GPL linking; same approach as the proven Python pipeline |
| Overlay drawing | tiny-skia + cosmic-text | Text shaping, emoji, and outlined text in pure Rust |
| Overlay transport | Render only the **caption band** (≈1080×400 RGBA), placed with `overlay=x:y`; draw the hook, zooms, and end card only inside `enable='between(t,a,b)'` windows | About 5× fewer bytes than piping full frames |
| Transcription | whisper-rs, CPU, quantized models | Offline; model size chosen by the benchmark |
| Hub server | axum + static assets embedded in the binary | One file to copy onto a club laptop |
| Encoding presets | `medium` for personal use, `veryfast` for hub mode | Hub throughput matters more than the last few percent of quality |

**Memory:** stream everything, with at most 2 overlay buffers in flight. Peak memory < 800 MB, excluding the Whisper model.
**Testing:** golden tests on the plan → timeline mapping (exact frame counts), clamping edge cases, overlay PNGs at fixed timestamps, and SRT text. The final MP4 is **smoke-checked** (duration ±1 frame, stream properties, loudness ±1 LU), not byte-compared, because x264 output varies across builds and CPUs.

## 9. Success metrics

| Area | Metric | Target |
|---|---|---|
| Engine | 10-minute raw recording → 60 s short (4 CPUs, excluding transcription) | ≤ 8 min |
| Engine | Peak memory during render | < 800 MB |
| Engine | Cuts landing mid-word (manual review of 50 cuts) | < 5% |
| Engine | Network calls during the default pipeline | 0 |
| Hub | Throughput on the reference laptop (≤ 3 min raw per student) | ≥ 10 shorts/hour |
| Hub | Phone page first load | ≤ 100 KB |
| Hub | Upload after a Wi-Fi drop | Resumes, never restarts |
| Pilot | Students leaving with a finished short | ≥ 80% of at least 10 |

## 10. After the pilot (prioritized, no dates)

1. **Pidgin and Camfranglais caption packs:** community TOML correction dictionaries, later fine-tuning; bilingual EN/FR caption tracks. This is the long-term moat.
2. **Retake detection:** when a line is said several times, keep the best take, judged by transcript similarity, fewer fillers, and steadier audio.
3. **Data-cost estimate:** show the file size and approximate data cost before sharing; WhatsApp Status and data-saver presets.
4. **Approval contact sheet:** a ~200 KB image of key frames that a client or teammate can approve over WhatsApp before rendering.
5. **Style from example:** extract cut rhythm, caption placement, and colours from a video you like. This analyzes the *output*, not anyone's software.
6. **Agent layer:** an MCP server (rmcp) and an LLM planner that emits validated plan patches.
7. **rea side quest:** observing outputs only, after a terms-of-service check. Never inspecting packages or models, and never on the critical path.

## 11. Possible futures (no commitment)

- **Hub-in-a-box:** DevSafe sets up preconfigured mini-PCs or laptops for schools and clubs and charges for setup and training via Mobile Money, while the software stays open source.
- A Tauri desktop app for facilitators.
- A native Flutter companion app, only if browser uploads prove insufficient.

## 12. Decisions

| Topic | Decision |
|---|---|
| Name | **Sabicut**. Checked 2026-11-02: free on crates.io, npm, and PyPI; no GitHub repository or account; no product found on the web; no DNS records for .com/.app/.dev/.io. A Sabicut domain lapsed in 2023, so confirm with a registrar before buying |
| Repository | `DCT-Berinyuy/sabicut`, public |
| License | MIT/Apache-2.0 dual |
| Orientation | Vertical first |
| Plan file | `plan.toml`. Not "Cutfile", which Kinocut uses |
| Gate | Replaced by the time-box in §0 |

## 13. Risks

| Risk | Mitigation |
|---|---|
| A weak hub laptop is too slow for a whole class | Benchmark first; one job at a time; `veryfast` preset; smaller model in hub mode; cap raw footage per student |
| Students' phones record HEVC or HDR (washed-out colours) | Decode with ffmpeg; tone-map HDR to SDR in the render graph; recommend 1080p recording |
| Laptop hotspot capacity (often about 8–10 clients) | Cap upload size; stagger uploads through the queue; document the router option |
| Privacy of minors' footage | LAN only, no accounts, automatic wipe, facilitator consent checklist |
| Transcription quality for Pidgin | Human fix step from day one; correction packs; fine-tuning later |
| Scope creep | Non-goals in §4; kill/review rule in §0 |
