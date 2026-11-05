//! A plan with its source clips probed: where render and preview start.

use std::path::{Path, PathBuf};

use sabicut_core::{Plan, Rational, Segment, Timeline, ValidationErrors};
use sabicut_overlay::Graphics;

use crate::error::RenderError;
use crate::geometry::CropRect;
use crate::graph::{AudioSpan, VideoSpec};
use crate::probe::{SourceInfo, probe};
use crate::tools::Ffmpeg;
use crate::zoom::ZoomCurve;

/// A source clip that the plan's cuts use, probed.
#[derive(Clone, Debug)]
pub struct ProbedSource {
    id: String,
    path: PathBuf,
    info: SourceInfo,
    spec: VideoSpec,
}

impl ProbedSource {
    /// The source's id in the plan.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The clip's path, resolved from the plan file's directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the clip contains.
    pub fn info(&self) -> &SourceInfo {
        &self.info
    }

    /// The vertical crop window on the upright frame.
    pub fn crop(&self) -> CropRect {
        self.spec.crop()
    }
}

/// One cut, ready to become filters.
#[derive(Clone, Debug)]
pub(crate) struct CutJob {
    pub(crate) segment: Segment,
    /// The source clip's path.
    pub(crate) path: PathBuf,
    /// How the source's frames become output frames.
    pub(crate) spec: VideoSpec,
    /// Whether the source has audio; if not, the cut is silent.
    pub(crate) has_audio: bool,
    pub(crate) curve: ZoomCurve,
    pub(crate) audio: AudioSpan,
}

/// A valid plan whose source clips have been probed, with its captions and hook if it
/// has them ([`Project::with_graphics`]).
#[derive(Clone, Debug)]
pub struct Project {
    timeline: Timeline,
    out: (u32, u32),
    sources: Vec<ProbedSource>,
    cuts: Vec<CutJob>,
    graphics: Option<Graphics>,
}

/// Why a project could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The plan breaks schema v1's rules.
    #[error("the plan is not valid:\n{0}")]
    Invalid(ValidationErrors),
    /// A source could not be probed, or a cut does not fit its source.
    #[error(transparent)]
    Render(#[from] RenderError),
}

impl Project {
    /// Validates `plan` and probes the source clips its cuts use. Relative paths are
    /// resolved from `dir`, the plan file's directory.
    ///
    /// Fails if a source cannot be read, has no video, or is shorter than a cut taken
    /// from it (by more than one frame; a cut that overruns by less holds the source's
    /// last frame).
    pub fn open(tools: &Ffmpeg, plan: &Plan, dir: &Path) -> Result<Self, OpenError> {
        let timeline = Timeline::from_plan(plan).map_err(OpenError::Invalid)?;
        let fps = timeline.fps();
        let out = (plan.meta().out().w(), plan.meta().out().h());
        let mut sources: Vec<ProbedSource> = Vec::new();
        let mut cuts = Vec::with_capacity(timeline.segments().len());
        for (index, segment) in timeline.segments().iter().enumerate() {
            let source = match sources.iter().position(|s| s.id == segment.source()) {
                Some(found) => found,
                None => {
                    let declared = plan.sources().iter().find(|s| s.id() == segment.source());
                    // Validation guarantees every cut's source is declared.
                    let declared = declared.ok_or_else(|| RenderError::UnknownSource {
                        cut: index,
                        id: segment.source().to_owned(),
                    })?;
                    let path = dir.join(declared.path());
                    let info = probe(tools, &path).map_err(|source| RenderError::Probe {
                        id: declared.id().to_owned(),
                        path: path.clone(),
                        source: Box::new(source),
                    })?;
                    let spec = VideoSpec::new(&info, declared.crop_x(), fps, out);
                    sources.push(ProbedSource {
                        id: declared.id().to_owned(),
                        path,
                        info,
                        spec,
                    });
                    sources.len() - 1
                }
            };
            let probed = sources
                .get(source)
                .ok_or_else(|| RenderError::UnknownSource {
                    cut: index,
                    id: segment.source().to_owned(),
                })?;
            check_cut_fits(index, segment, probed, &timeline)?;
            cuts.push(CutJob {
                segment: segment.clone(),
                path: probed.path.clone(),
                spec: probed.spec.clone(),
                has_audio: probed.info.has_audio,
                curve: ZoomCurve::new(segment, fps),
                audio: AudioSpan::new(segment, fps),
            });
        }
        Ok(Self {
            timeline,
            out,
            sources,
            cuts,
            graphics: None,
        })
    }

    /// The project with captions, the hook and subtitles drawn from `graphics` (made
    /// from the same plan, for example with [`Graphics::load`]). Renders and previews
    /// then show them, through the same code.
    pub fn with_graphics(mut self, graphics: Option<Graphics>) -> Self {
        self.graphics = graphics;
        self
    }

    /// The captions, hook and subtitles, if any.
    pub fn graphics(&self) -> Option<&Graphics> {
        self.graphics.as_ref()
    }

    /// Whether anything is drawn over the video: captions or a hook.
    pub fn draws_graphics(&self) -> bool {
        self.graphics.as_ref().is_some_and(Graphics::draws)
    }

    /// The output timeline.
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// Output frame size `(width, height)`.
    pub fn out(&self) -> (u32, u32) {
        self.out
    }

    /// The source clips the cuts use, in order of first use.
    pub fn sources(&self) -> &[ProbedSource] {
        &self.sources
    }

    pub(crate) fn cuts(&self) -> &[CutJob] {
        &self.cuts
    }

    /// Whether any cut zooms.
    pub fn zooms(&self) -> bool {
        self.cuts.iter().any(|cut| !cut.curve.is_flat())
    }

    /// Whether any source used is HDR and will be tone-mapped.
    pub fn tone_maps(&self) -> bool {
        self.sources.iter().any(|source| source.spec.is_hdr())
    }

    /// The output frame shown at `time` seconds (the frame whose interval contains
    /// it). `what` names the option the time came from, for the error.
    pub fn frame_at(&self, time: f64, what: &str) -> Result<u64, RenderError> {
        let total = self.timeline.total_frames();
        let out_of_range = || RenderError::TimeOutOfRange {
            what: what.to_owned(),
            time,
            duration: self.timeline.duration().to_f64(),
        };
        let exact = Rational::from_secs_f64(time)
            .filter(|_| time >= 0.0)
            .ok_or_else(out_of_range)?;
        let fps = i128::from(self.timeline.fps());
        let frame = (i128::from(exact.numer()) * fps).div_euclid(i128::from(exact.denom()));
        u64::try_from(frame)
            .ok()
            .filter(|frame| *frame < total)
            .ok_or_else(out_of_range)
    }

    /// The cut that output frame `frame` belongs to, and the frame's number within it.
    pub(crate) fn locate(&self, frame: u64) -> Option<(&CutJob, u64)> {
        self.cuts.iter().find_map(|cut| {
            let segment = &cut.segment;
            (segment.output_start() <= frame && frame < segment.output_end())
                .then(|| (cut, frame - segment.output_start()))
        })
    }
}

/// Fails if `segment` ends more than one frame after its source's video.
fn check_cut_fits(
    index: usize,
    segment: &Segment,
    source: &ProbedSource,
    timeline: &Timeline,
) -> Result<(), RenderError> {
    let Some(video_end) = source.info.video_end else {
        return Ok(());
    };
    let end = timeline.frame_time(segment.source_out()).to_f64();
    let one_frame = 1.0 / f64::from(timeline.fps());
    if end > video_end + one_frame + 1e-6 {
        return Err(RenderError::CutPastEnd {
            cut: index,
            id: source.id.clone(),
            end,
            video_end,
        });
    }
    Ok(())
}
