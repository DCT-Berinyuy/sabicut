//! `sabicut preview`: PNG stills at chosen output times and a contact sheet of them
//! (PRD F4), to check crops and zooms before a full render.
//!
//! Each still opens its source just before the frame it needs, decodes up to it and
//! runs the same filters as the render ([`crate::graph::video_chain`]) on that one
//! frame, so a still matches the rendered frame at the same time. Stills are made up to
//! four per ffmpeg run (each still has its own decoder, so this bounds memory), and the
//! contact sheet is laid out from the written PNGs.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::RenderError;
use crate::graph::{
    Graph, HDR_FILTERS, Inputs, VIDEO_FILTERS, ZOOM_FILTERS, seek_before, video_chain,
};
use crate::project::Project;
use crate::render::{SOURCE_TIMESTAMPS, base_args};
use crate::safezone::guide_filters;
use crate::tools::Ffmpeg;
use crate::work::WorkDir;

/// Width of a still on the contact sheet, in pixels.
pub const SHEET_CELL_WIDTH: u32 = 360;
/// Stills per row on the contact sheet.
pub const SHEET_COLUMNS: usize = 5;
/// Stills made per ffmpeg run. Each still has its own decoder, so this bounds memory.
const STILLS_PER_RUN: usize = 4;
/// Space around and between stills on the contact sheet, in pixels.
const SHEET_GAP: u32 = 16;
const SHEET_BACKGROUND: &str = "0x202020";

/// What to preview, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewOptions {
    /// Output times to take stills at, in seconds, in the order they appear on the
    /// contact sheet.
    pub times: Vec<f64>,
    /// Directory for the stills and the sheet; created if needed.
    pub out_dir: PathBuf,
    /// Shade the areas TikTok, Reels and Shorts cover with their interface
    /// (see [`crate::safezone`]).
    pub safe_zones: bool,
}

/// One still written by [`preview`].
#[derive(Clone, Debug, PartialEq)]
pub struct Still {
    /// The requested output time, in seconds.
    pub time: f64,
    /// The output frame shown at that time.
    pub frame: u64,
    /// The PNG written.
    pub path: PathBuf,
}

/// What a preview produced.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewReport {
    /// The stills, in the order requested.
    pub stills: Vec<Still>,
    /// The contact sheet: every still in one image.
    pub contact_sheet: PathBuf,
}

/// The file name of still number `index` (from 0) at `time` seconds.
pub fn still_name(index: usize, time: f64) -> String {
    format!("still-{:02}-{time:.3}s.png", index + 1)
}

/// The contact sheet's file name.
pub const CONTACT_SHEET_NAME: &str = "contact-sheet.png";

/// Writes a still for each of `options.times`, and the contact sheet.
pub fn preview(
    tools: &Ffmpeg,
    project: &Project,
    options: &PreviewOptions,
) -> Result<PreviewReport, RenderError> {
    if options.times.is_empty() {
        return Err(RenderError::NoTimes);
    }
    tools.require_filters(VIDEO_FILTERS, "cut and scale video")?;
    tools.require_filters(&["pad", "xstack"], "lay out the contact sheet")?;
    if project.zooms() {
        tools.require_filters(ZOOM_FILTERS, "zoom")?;
    }
    if project.tone_maps() {
        tools.require_filters(HDR_FILTERS, "tone-map HDR (PQ or HLG) video to SDR")?;
    }
    if options.safe_zones {
        tools.require_filters(&["drawbox"], "draw the safe-zone guide")?;
    }
    tools.require_encoder("png", "write PNG stills")?;
    let frames = options
        .times
        .iter()
        .map(|&time| project.frame_at(time, "--at"))
        .collect::<Result<Vec<u64>, RenderError>>()?;

    fs::create_dir_all(&options.out_dir).map_err(RenderError::io(&options.out_dir))?;
    let work = WorkDir::new(&options.out_dir)?;
    let stills: Vec<Still> = options
        .times
        .iter()
        .zip(&frames)
        .enumerate()
        .map(|(index, (&time, &frame))| Still {
            time,
            frame,
            path: options.out_dir.join(still_name(index, time)),
        })
        .collect();
    for (batch, chunk) in stills.chunks(STILLS_PER_RUN).enumerate() {
        write_stills(tools, project, options, &work, batch, chunk)?;
    }
    let contact_sheet = options.out_dir.join(CONTACT_SHEET_NAME);
    write_sheet(tools, project, &work, &stills, &contact_sheet)?;
    Ok(PreviewReport {
        stills,
        contact_sheet,
    })
}

/// Writes `stills` in one ffmpeg run: one input per still, each opened just before the
/// frame it needs.
fn write_stills(
    tools: &Ffmpeg,
    project: &Project,
    options: &PreviewOptions,
    work: &WorkDir,
    batch: usize,
    stills: &[Still],
) -> Result<(), RenderError> {
    let fps = project.timeline().fps();
    let (out_w, out_h) = project.out();
    let mut inputs = Inputs::default();
    let mut graph = Graph::default();
    let mut outputs = Vec::with_capacity(stills.len());
    for still in stills {
        let (cut, local) =
            project
                .locate(still.frame)
                .ok_or_else(|| RenderError::TimeOutOfRange {
                    what: "--at".to_owned(),
                    time: still.time,
                    duration: project.timeline().duration().to_f64(),
                })?;
        let seek = seek_before(cut.segment.source_in().saturating_add(local), fps);
        let input = inputs.add_seeked(&cut.path, seek, cut.spec.decoder_threads());
        let mut chain = video_chain(&cut.spec, &cut.curve, &cut.segment, local..local + 1);
        if options.safe_zones {
            chain.push(',');
            chain.push_str(&guide_filters(out_w, out_h));
        }
        let label = format!("s{input}");
        graph.chain(&[&format!("{input}:V:0")], &chain, &[&label]);
        outputs.push((label, &still.path));
    }
    let script = work.write(&format!("stills-{batch}.graph"), &graph.finish())?;
    let mut args = base_args("error");
    args.extend(SOURCE_TIMESTAMPS.map(OsString::from));
    args.extend_from_slice(inputs.args());
    args.extend(tools.graph_script_args(&script));
    for (label, path) in outputs {
        args.extend(["-map", &format!("[{label}]")].map(OsString::from));
        args.extend(PNG_OUTPUT.map(OsString::from));
        args.push(path.as_os_str().to_owned());
    }
    tools.run_ffmpeg(&args)?;
    Ok(())
}

/// Lays out the written stills on the contact sheet.
fn write_sheet(
    tools: &Ffmpeg,
    project: &Project,
    work: &WorkDir,
    stills: &[Still],
    sheet: &Path,
) -> Result<(), RenderError> {
    let mut inputs = Inputs::default();
    let cells: Vec<String> = stills
        .iter()
        .map(|still| format!("{}:v:0", inputs.add_file(&still.path)))
        .collect();
    let mut graph = Graph::default();
    sheet_chains(&mut graph, &cells, project.out());
    let script = work.write("sheet.graph", &graph.finish())?;
    let mut args = base_args("error");
    args.extend_from_slice(inputs.args());
    args.extend(tools.graph_script_args(&script));
    args.extend(["-map", "[sheet]"].map(OsString::from));
    args.extend(PNG_OUTPUT.map(OsString::from));
    args.push(sheet.as_os_str().to_owned());
    tools.run_ffmpeg(&args)?;
    Ok(())
}

/// Output options for a single PNG image.
const PNG_OUTPUT: [&str; 8] = [
    "-frames:v",
    "1",
    "-c:v",
    "png",
    "-update",
    "1",
    "-f",
    "image2",
];

/// Appends the chains that shrink the stills labelled `cells` and lay them out in rows
/// of up to [`SHEET_COLUMNS`], into `[sheet]`.
fn sheet_chains(graph: &mut Graph, cells: &[String], out: (u32, u32)) {
    let cell_w = SHEET_CELL_WIDTH;
    let cell_h = even_round(f64::from(cell_w) * f64::from(out.1) / f64::from(out.0.max(1)));
    let count = cells.len().max(1);
    let columns = count.min(SHEET_COLUMNS);
    let rows = count.div_ceil(columns);
    let width = to_u32(columns) * (cell_w + SHEET_GAP) + SHEET_GAP;
    let height = to_u32(rows) * (cell_h + SHEET_GAP) + SHEET_GAP;
    let mut shrunk = Vec::with_capacity(cells.len());
    for (index, cell) in cells.iter().enumerate() {
        let label = format!("g{index}");
        graph.chain(
            &[cell],
            &format!("scale={cell_w}:{cell_h}:flags=area"),
            &[&label],
        );
        shrunk.push(label);
    }
    let pad = format!("pad={width}:{height}:{{x}}:{{y}}:color={SHEET_BACKGROUND}");
    match shrunk.as_slice() {
        [only] => graph.chain(
            &[only],
            &pad.replace("{x}", &SHEET_GAP.to_string())
                .replace("{y}", &SHEET_GAP.to_string()),
            &["sheet"],
        ),
        _ => {
            let layout: Vec<String> = (0..shrunk.len())
                .map(|index| {
                    let (column, row) = (to_u32(index % columns), to_u32(index / columns));
                    let x = SHEET_GAP + column * (cell_w + SHEET_GAP);
                    let y = SHEET_GAP + row * (cell_h + SHEET_GAP);
                    format!("{x}_{y}")
                })
                .collect();
            let labels: Vec<&str> = shrunk.iter().map(String::as_str).collect();
            graph.chain(
                &labels,
                &format!(
                    "xstack=inputs={}:layout={}:fill={SHEET_BACKGROUND},{}",
                    shrunk.len(),
                    layout.join("|"),
                    pad.replace("{x}", "0").replace("{y}", "0"),
                ),
                &["sheet"],
            );
        }
    }
}

fn even_round(value: f64) -> u32 {
    let even = ((value / 2.0).round() * 2.0).clamp(2.0, 8192.0);
    // Clamped to a small positive whole number, so the cast is exact.
    even as u32
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_stills_sit_in_one_row() {
        let mut graph = Graph::default();
        let cells: Vec<String> = (0..3).map(|i| format!("c{i}")).collect();
        sheet_chains(&mut graph, &cells, (1080, 1920));
        let text = graph.finish();
        assert!(
            text.starts_with("[c0]scale=360:640:flags=area[g0];\n"),
            "{text}"
        );
        assert!(
            text.ends_with("[g0][g1][g2]xstack=inputs=3:layout=16_16|392_16|768_16:fill=0x202020,pad=1144:672:0:0:color=0x202020[sheet]\n"),
            "{text}"
        );
    }

    #[test]
    fn seven_stills_wrap_after_five() {
        let mut graph = Graph::default();
        let cells: Vec<String> = (0..7).map(|i| format!("c{i}")).collect();
        sheet_chains(&mut graph, &cells, (1080, 1920));
        let text = graph.finish();
        assert!(
            text.contains("layout=16_16|392_16|768_16|1144_16|1520_16|16_672|392_672:"),
            "{text}"
        );
        assert!(text.contains("pad=1896:1328:0:0"), "{text}");
    }

    #[test]
    fn one_still_is_just_padded() {
        let mut graph = Graph::default();
        sheet_chains(&mut graph, &["c0".to_owned()], (1080, 1920));
        assert!(
            graph
                .finish()
                .ends_with("[g0]pad=392:672:16:16:color=0x202020[sheet]\n")
        );
    }

    #[test]
    fn still_names_sort_in_order() {
        assert_eq!(still_name(0, 1.5), "still-01-1.500s.png");
        assert_eq!(still_name(11, 22.0), "still-12-22.000s.png");
    }
}
