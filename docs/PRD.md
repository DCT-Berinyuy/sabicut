# Sabicut — PRD (v0.4)

> **An offline AI video editor for creators who record on a laptop.**
> A Rust engine turns raw recordings (phone clips, or OBS screen and webcam recordings) into captioned vertical shorts on your own laptop. No upload, no subscription, no internet. It starts with tech and education creators.
>
> *Sabi* is Pidgin for "to know / know-how".

Owner: Mr. DCT (DevSafe) · **Status: ACTIVE (time-boxed)** · Updated: 2026-11-07
**Changes in v0.4:** Sabicut now targets laptop creators, starting with tech and education creators, instead of student communities (owner's decision, 2026-11-07). v0.2 builds automatic length and a local review page instead of the Wi-Fi hub, which moves to §11.

---

## 0. Time-box and kill date

| Phase | Window | Exit criterion (must be met to continue) |
|---|---|---|
| **Engine v0.1** | 2026-11-02 → 2026-11-16 | One DCT Lab short made with Sabicut and published |
| **Creator v0.2** | 2026-11-16 → 2026-11-30 | From a 10-minute recording, Sabicut picks a short of 60 s or less that I review in the local review page and publish without re-picking sections by hand |
| **Creator beta** | by 2026-12-07 | At least 5 creators other than me (from DCT Lab, GDGoC and OWASP Yaoundé) each publish a short made with Sabicut |

**Kill/review rule:** if any phase misses its exit criterion by more than 1 week, stop and review before writing more code.

## 1. Problem

Creators who record on a laptop (screen recordings, webcam talks, lessons, phone clips copied over) spend more time editing than recording:
- Cutting pauses and false starts, adding captions, and trimming a long recording to 60 seconds are slow by hand.
- Cloud AI editors do this, but they need the raw recording uploaded (about a gigabyte per 10 minutes) and a subscription paid in dollars by card. On Cameroonian mobile data, the upload alone is expensive.
- Many of these creators use older laptops, and many tech creators use Linux, where the desktop AI editors (Descript, CapCut, Premiere Pro) don't run.
- Auto-captions handle Pidgin, Camfranglais, and code-switched French/English badly.

CapCut is free and on most phones; Sabicut will not out-feature it. Sabicut competes where cloud and phone apps can't: **no upload, no subscription, an old laptop is enough (Linux included), every edit is a plan file you can review, and local languages.**

## 2. Positioning (existing tools checked 2026-11-02 and 2026-11-07)

| Tool | What it already does | Why Sabicut is still different |
|---|---|---|
| auto-editor (5.7k★, since 2020) | Cuts dead air by loudness or motion; exports to Premiere, Resolve, Final Cut, Shotcut and Kdenlive | No captions; doesn't know about words, so it can't cut false starts or pick the best 60 s |
| CutScript (270★) | Edits video by editing its transcript; Electron + Python | Heavy runtime; no styled vertical shorts; not built for low-spec hardware |
| Kinocut (237★) | Local MCP video editor for AI agents; Shorts/Reels; JSON plan format named "Cutfile" | Python, built for agents and developers rather than creators |
| Premiere Pro, Descript | Transcript-based editing and pause removal; Descript also captions and clips | Paid; no Linux version; Descript transcribes in its cloud |
| CapCut, Captions, Submagic | Auto-captions, styles and templates on phone or web | Need internet for their AI features; paid tiers; English-first captions |
| OpenScrub, Cloak | Redact secrets and PII in screen recordings | Already solved, so redaction stays out of Sabicut's scope |

**Sabicut's position:** offline from raw recording to captioned short, in one Rust binary that runs on an old laptop (Linux included), with a plan file you can review and edit, and captions aimed at African languages. It starts with tech and education creators: they record on laptops, often on Linux, and the owner is one of them.

## 3. Users

| User | Device | How they use Sabicut |
|---|---|---|
| **Tech or education creator** (first: me and DCT Lab; then GDGoC and OWASP Yaoundé members, and teachers recording lessons) | Laptop, often Linux; OBS for screen and webcam, or a phone for face clips | analyze → plan → review page → render; publishes to TikTok, Reels and Shorts |
| **Other laptop creators** | Laptop | The same workflow, through the review page instead of the command line |
| **Contributor** | Any | Adds Pidgin/French correction packs and caption styles as plain TOML files |

## 4. Goals and non-goals

**Goals**
1. Engine: raw recordings → vertical 1080×1920 30 fps MP4 + cover JPG + SRT, **fully offline**, within the RAM budget.
2. A hand-editable **edit plan** (`plan.toml`) with a **frozen, versioned schema**.
3. Creator workflow: a 10-minute recording becomes a short of 60 s or less, picked automatically, then checked and rendered from a local review page.

**Non-goals (until after the creator beta):** hosted SaaS, accounts, telemetry; a GUI timeline; the community hub (§11); MCP server or LLM planner; Tauri desktop app; native mobile app; secret redaction; rea reverse-engineering work; horizontal output; sound effects, stickers, confetti, B-roll.

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
| F5 | `sabicut render` | Frame-exact trims, short audio fades, concat. Each cut is decoded in turn and piped to one encoder, because a single ffmpeg graph peaked at 1.23 GB. Audio: denoise → high-pass → compressor → loudness normalization to −14 LUFS; AAC 160k 48 kHz; x264 CRF 20; fast-start. Also writes the SRT and the cover. The end card is deferred: schema v1 has no fields for its content |
| F6 | Captions and hook | Word-by-word captions (active word gold with a pop, keywords always gold, auto line wrap, platform safe zones); hook text for the first 2 s |

### Day-one benchmark: transcription speed is the main risk

10-minute speech sample, 4 threads, voice-activity detection on, quantized models (`q5_1`). The results go in `docs/bench/whisper.md`.

| Model | Measure | Keep it if |
|---|---|---|
| tiny, base, small | Wall-clock time, peak RSS, rough word error rate on 2 hand-checked minutes (English and French) | Transcribing 10 min takes ≤ 10 min **and** peak RSS < 1.5 GB |

Pick the largest model that passes. Quick drafts may use a smaller model.

## 7. Creator v0.2

| ID | Feature | Notes |
|---|---|---|
| C1 | Automatic length: `sabicut plan --max-duration 60` | Picks which kept sections go into the short. Rules-based and deterministic: keep whole sentences, prefer sentences with keywords and the opening line (the hook), keep the original order, never cut mid-word. Every dropped sentence is listed in the plan so it can be restored by hand. The LLM planner stays after the beta |
| C2 | Local review page: `sabicut serve` | A page on `127.0.0.1` with the transcript and click-to-fix words (writes `fixes.toml`), the picked and dropped sections with one-click restore, preview stills with the safe-zone guide, and a render button with progress. **≤ 100 KB on first load**, embedded in the binary. Plain HTML or a SvelteKit static build, decided at v0.2 |
| C3 | Recording guide | `docs/recording.md`: OBS settings (1080×1920 canvas, 30 fps, hardware H.264, 1-second keyframes, 48 kHz audio), phone settings, and the layout zones that the hook, captions and app buttons use |
| C4 *(stretch)* | Caption styles | 2–3 named styles in TOML (colours, size, position), chosen with an optional `captions.style` field |

**Review page security (non-negotiable):** bind to `127.0.0.1` only; a random token in the page URL; read and write files only inside the project folder, with no path traversal; make zero outbound network calls; run one render at a time through the shared job lock.

## 8. Technical design

Crates are added only when they contain real code, so there are no placeholder crates:

```
crates/
  core/     plan types, schema v1, validation, rational time, timeline mapping, words.json, fixes
  analyze/  whisper-rs, voice-activity detection
  plan/     rules engine (dead air, false starts, zooms; length in v0.2)
  overlay/  tiny-skia + rustybuzz, word cache, easing, SRT
  render/   ffmpeg pipeline, caption-band pipe, preview stills
  serve/    axum server and embedded review page (v0.2)
  cli/      clap entry point: `sabicut`
spikes/     throwaway experiments (e.g. whisper-bench), not workspace members
```

| Decision | Choice | Why |
|---|---|---|
| Video I/O | ffmpeg **binary as a subprocess** | Robust; no GPL linking; same approach as the proven Python pipeline |
| Overlay drawing | tiny-skia + rustybuzz | Kerned, outlined text with accents. cosmic-text was measured and rejected: it grows the binary by 2.28 MB against 0.78 MB, and its system-font fallback breaks determinism |
| Overlay transport | Render only the **caption band** (about 756×240 RGBA), placed with `overlay=x:y`; draw the hook only inside its frame window | Far fewer bytes than piping full frames |
| Transcription | whisper-rs, CPU, quantized models | Offline; model size chosen by the benchmark |
| Review page server | axum, bound to `127.0.0.1`, with static assets embedded in the binary | One binary, nothing else to install |
| Encoding presets | `medium` for final renders, `veryfast` for quick drafts | Drafts favour speed; finals favour quality |

**Memory:** stream everything, with at most 2 overlay buffers in flight. Peak memory < 800 MB, excluding the Whisper model.
**Testing:** golden tests on the plan → timeline mapping (exact frame counts), clamping edge cases, overlay PNGs at fixed timestamps, and SRT text. The final MP4 is **smoke-checked** (duration ±1 frame, stream properties, loudness ±1 LU), not byte-compared, because x264 output varies across builds and CPUs.

## 9. Success metrics

| Area | Metric | Target |
|---|---|---|
| Engine | 10-minute raw recording → 60 s short (4 CPUs, excluding transcription) | ≤ 8 min |
| Engine | Peak memory during render | < 800 MB |
| Engine | Cuts landing mid-word (manual review of 50 cuts) | < 5% |
| Engine | Network calls during the default pipeline | 0 |
| Creator v0.2 | Automatic length: 10-minute recording → short of 60 s or less | Published without re-picking sections in at least 3 of 5 test recordings |
| Creator v0.2 | Review page first load | ≤ 100 KB |
| Creator v0.2 | My time from opening the review page to a rendered short (after `analyze`) | ≤ 5 min |
| Beta | Creators other than me who publish a Sabicut short | ≥ 5 by 2026-12-07 |

## 10. After the creator beta (prioritized, no dates)

1. **Pidgin and Camfranglais caption packs:** community TOML correction dictionaries, later fine-tuning; bilingual EN/FR caption tracks. This is the long-term moat.
2. **Tutorial mode:** in screen recordings, zoom where the cursor or the typing is, instead of the centre.
3. **Retake detection:** when a line is said several times, keep the best take, judged by transcript similarity, fewer fillers, and steadier audio.
4. **Style from example:** extract cut rhythm, caption placement, and colours from a video you like. This analyzes the *output*, not anyone's software.
5. **Data-cost estimate:** show the file size and approximate data cost before sharing; WhatsApp Status and data-saver presets.
6. **Agent layer:** an MCP server (rmcp) and an LLM planner that emits validated plan patches, including smarter length picking.
7. **rea side quest:** observing outputs only, after a terms-of-service check. Never inspecting packages or models, and never on the critical path.

## 11. Possible futures (no commitment)

- **Community hub** (the v0.3 plan): `sabicut serve` opened on a laptop's own Wi-Fi, so a club or class uploads from phone browsers and gets shorts back with zero data. It needs a job queue, resumable uploads, session PINs, an automatic wipe, and consent for minors' footage. Revisit after the creator beta.
- **Hub-in-a-box:** DevSafe sets up preconfigured laptops for schools and clubs and charges for setup and training via Mobile Money, while the software stays open source.
- **Desktop app:** Tauri wrapping the review page.
- **Mobile:** a native Flutter app, only if phone-only creators ask for it.

## 12. Decisions

| Topic | Decision |
|---|---|
| Name | **Sabicut**. Checked 2026-11-02: free on crates.io, npm, and PyPI; no GitHub repository or account; no product found on the web; no DNS records for .com/.app/.dev/.io. A Sabicut domain lapsed in 2023, so confirm with a registrar before buying |
| Repository | `DCT-Berinyuy/sabicut`, public |
| License | MIT/Apache-2.0 dual |
| Audience | Laptop creators, starting with tech and education creators. Decided 2026-11-07; the community hub moved to §11 |
| Orientation | Vertical first |
| Plan file | `plan.toml`. Not "Cutfile", which Kinocut uses |
| Gate | Replaced by the time-box in §0 |

## 13. Risks

| Risk | Mitigation |
|---|---|
| Crowded creator market (CapCut, Descript, Captions, auto-editor) | Start with tech and education creators; compete on offline, free, old hardware, Linux and the plan file, not on feature count |
| Automatic length picks the wrong parts | Explainable rules; every dropped sentence is listed in the plan and restorable in the review page; preview before render |
| A slow laptop | Measured so far on a 2015 i7: the `small` model transcribes at about real time and a render peaks at 691 MB; smaller models and `veryfast` for drafts |
| Phones record HEVC or HDR (washed-out colours) | Decode with ffmpeg; tone-map HDR to SDR in the render graph; recommend 1080p recording |
| OBS recordings in odd sizes (e.g. a 900×1440 monitor capture) | The recording guide; render already crops to 9:16 and converts to a constant frame rate |
| Transcription quality for Pidgin | Human fix step from day one; correction packs; fine-tuning later |
| Scope creep | Non-goals in §4; kill/review rule in §0 |
