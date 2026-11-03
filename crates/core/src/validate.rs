//! Plan validation. Every problem names its field, with the cut index where there is one.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;

use crate::plan::{
    Captions, Cut, Hook, MAX_DIMENSION, MAX_FPS, MAX_ID_LEN, MAX_SECONDS, MAX_STROKE, MAX_ZOOM,
    MIN_ZOOM, Meta, Plan, SCHEMA_VERSION, Source,
};
use crate::rational::Rational;
use crate::timeline::{FrameGrid, Timeline, TimelineBuilder};

/// One problem in a plan: the field it is about, and what is wrong with it.
///
/// Displays as `<field> <message>`, for example `cut[3].out must be > in`. Indexes
/// count from 0, in the order the entries appear in the file.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{field} {message}")]
pub struct ValidationError {
    field: String,
    message: String,
}

impl ValidationError {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }

    /// Path of the field, like `cut[3].out` or `meta.fps`.
    pub fn field(&self) -> &str {
        &self.field
    }

    /// What is wrong with it, like `must be > in`.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Every problem found in a plan (or in a words.json or fixes.toml file), never empty.
///
/// Problems are grouped by section in schema order (for a plan: schema, meta, sources,
/// cuts, hook, captions) and listed in file order within a section. Displays one
/// problem per line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationErrors(Vec<ValidationError>);

impl ValidationErrors {
    /// The problems, in order.
    pub fn as_slice(&self) -> &[ValidationError] {
        &self.0
    }

    /// Iterates over the problems, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, ValidationError> {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a ValidationErrors {
    type Item = &'a ValidationError;
    type IntoIter = std::slice::Iter<'a, ValidationError>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, error) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

/// Collects problems as the checks run.
#[derive(Default)]
pub(crate) struct Report(Vec<ValidationError>);

impl Report {
    pub(crate) fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.0.push(ValidationError::new(field, message));
    }

    pub(crate) fn finish(self) -> Result<(), ValidationErrors> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors(self.0))
        }
    }
}

/// A cut that passed every check, snapped to the frame grid.
struct SnappedCut<'p> {
    source: &'p str,
    source_in: u64,
    source_out: u64,
    zooms: Vec<(u64, f64)>,
}

/// Checks every rule and, when the plan is valid, derives its timeline.
pub(crate) fn check(plan: &Plan) -> Result<Timeline, ValidationErrors> {
    let mut report = Report::default();
    if plan.schema() != SCHEMA_VERSION {
        report.push(
            "schema",
            format!("must be {SCHEMA_VERSION}, got {}", plan.schema()),
        );
    }
    let grid = check_meta(&mut report, plan.meta());
    let ids = check_sources(&mut report, plan.sources());
    let builder = check_cuts(&mut report, plan.cuts(), &ids, grid);
    let output_frames = builder.as_ref().map(TimelineBuilder::output_frames);
    let hook_frames = plan
        .hook()
        .and_then(|hook| check_hook(&mut report, hook, grid, output_frames));
    if let Some(captions) = plan.captions() {
        check_captions(&mut report, captions);
    }
    match (report.finish(), builder) {
        (Err(errors), _) => Err(errors),
        (Ok(()), Some(builder)) => Ok(builder.finish(hook_frames)),
        // Not reachable: every check that leaves the builder empty reports a problem.
        (Ok(()), None) => Err(ValidationErrors(vec![ValidationError::new(
            "cut",
            "could not be placed on the timeline",
        )])),
    }
}

fn check_meta(report: &mut Report, meta: &Meta) -> Option<FrameGrid> {
    let fps = meta.fps();
    let grid = NonZeroU32::new(fps)
        .filter(|_| fps <= MAX_FPS)
        .map(FrameGrid::new);
    if grid.is_none() {
        report.push(
            "meta.fps",
            format!("must be between 1 and {MAX_FPS}, got {fps}"),
        );
    }
    let out = meta.out();
    for (field, value) in [("meta.out.w", out.w()), ("meta.out.h", out.h())] {
        if !(2..=MAX_DIMENSION).contains(&value) {
            report.push(
                field,
                format!("must be between 2 and {MAX_DIMENSION}, got {value}"),
            );
        } else if value % 2 != 0 {
            report.push(field, format!("must be even, got {value}"));
        }
    }
    grid
}

/// Returns each id with the index of the first source that uses it.
fn check_sources<'p>(report: &mut Report, sources: &'p [Source]) -> HashMap<&'p str, usize> {
    if sources.is_empty() {
        report.push("source", "must have at least one entry");
    }
    let mut ids = HashMap::new();
    for (i, source) in sources.iter().enumerate() {
        let id = source.id();
        let first = *ids.entry(id).or_insert(i);
        if !is_valid_id(id) {
            report.push(
                format!("source[{i}].id"),
                format!("must be 1 to {MAX_ID_LEN} letters, digits, '_' or '-', got {id:?}"),
            );
        } else if first != i {
            report.push(
                format!("source[{i}].id"),
                format!("must be unique, but {id:?} is already source[{first}].id"),
            );
        }
        if source.path().is_empty() {
            report.push(format!("source[{i}].path"), "must not be empty");
        }
        let crop_x = source.crop_x();
        if !(0.0..=1.0).contains(&crop_x) {
            report.push(
                format!("source[{i}].crop_x"),
                format!("must be between 0 and 1, got {crop_x}"),
            );
        }
    }
    ids
}

/// Returns the cuts placed on a timeline, or `None` if any cut has a problem.
fn check_cuts(
    report: &mut Report,
    cuts: &[Cut],
    ids: &HashMap<&str, usize>,
    grid: Option<FrameGrid>,
) -> Option<TimelineBuilder> {
    if cuts.is_empty() {
        report.push("cut", "must have at least one entry");
        return None;
    }
    let snapped: Vec<Option<SnappedCut<'_>>> = cuts
        .iter()
        .enumerate()
        .map(|(i, cut)| check_cut(report, i, cut, ids, grid))
        .collect();
    let mut builder = TimelineBuilder::new(grid?);
    for cut in snapped {
        let cut = cut?;
        builder.push(cut.source, cut.source_in, cut.source_out, &cut.zooms);
    }
    Some(builder)
}

fn check_cut<'p>(
    report: &mut Report,
    i: usize,
    cut: &'p Cut,
    ids: &HashMap<&str, usize>,
    grid: Option<FrameGrid>,
) -> Option<SnappedCut<'p>> {
    let source_known = ids.contains_key(cut.src());
    if !source_known {
        report.push(
            format!("cut[{i}].src"),
            format!("must be the id of a [[source]], got {:?}", cut.src()),
        );
    }
    let start = check_seconds(report, &format!("cut[{i}].in"), cut.in_point());
    let end = check_seconds(report, &format!("cut[{i}].out"), cut.out_point());
    let range = match (start, end) {
        (Some(start), Some(end)) if end > start => Some((start, end)),
        (Some(_), Some(_)) => {
            report.push(format!("cut[{i}].out"), "must be > in");
            None
        }
        _ => None,
    };
    let zooms = check_zooms(report, i, cut, range, grid);

    let (grid, (start, end)) = (grid?, range?);
    let (source_in, source_out) = (grid.boundary(start), grid.boundary(end));
    if source_out <= source_in {
        report.push(
            format!("cut[{i}]"),
            format!(
                "must be at least 1 frame long at {} fps, but in ({}) and out ({}) both snap to frame {source_in}",
                grid.fps(),
                cut.in_point(),
                cut.out_point(),
            ),
        );
        return None;
    }
    if !source_known {
        return None;
    }
    Some(SnappedCut {
        source: cut.src(),
        // Both are at least 0 because `in` and `out` are.
        source_in: u64::try_from(source_in).unwrap_or_default(),
        source_out: u64::try_from(source_out).unwrap_or_default(),
        zooms: zooms?,
    })
}

/// Returns each zoom's snapped `at` and scale, or `None` if any zoom (or its cut) has
/// a problem.
fn check_zooms(
    report: &mut Report,
    i: usize,
    cut: &Cut,
    range: Option<(Rational, Rational)>,
    grid: Option<FrameGrid>,
) -> Option<Vec<(u64, f64)>> {
    let mut snapped = Some(Vec::with_capacity(cut.zooms().len()));
    let mut previous: Option<(usize, Rational)> = None;
    for (j, zoom) in cut.zooms().iter().enumerate() {
        let field = format!("cut[{i}].zoom[{j}].at");
        let at = check_seconds(report, &field, zoom.at());
        let mut ok = at.is_some();
        if let Some(at) = at {
            if let Some(problem) = zoom_at_problem(at, zoom.at(), range, previous, grid) {
                report.push(field, problem);
                ok = false;
            }
            previous = Some((j, at));
        }
        if !(MIN_ZOOM..=MAX_ZOOM).contains(&zoom.scale()) {
            report.push(
                format!("cut[{i}].zoom[{j}].scale"),
                format!(
                    "must be between {MIN_ZOOM} and {MAX_ZOOM}, got {}",
                    zoom.scale()
                ),
            );
            ok = false;
        }
        snapped = match (snapped, grid, at) {
            (Some(mut list), Some(grid), Some(at)) if ok => {
                list.push((
                    u64::try_from(grid.boundary(at)).unwrap_or_default(),
                    zoom.scale(),
                ));
                Some(list)
            }
            _ => None,
        };
    }
    snapped
}

/// What is wrong with a zoom that starts at `at` (written as `secs`), given its cut's
/// `in` and `out` and the zoom before it, if anything.
///
/// Each zoom must start on a frame of its own, before the cut's last boundary, so that
/// every zoom lasts at least 1 frame.
fn zoom_at_problem(
    at: Rational,
    secs: f64,
    range: Option<(Rational, Rational)>,
    previous: Option<(usize, Rational)>,
    grid: Option<FrameGrid>,
) -> Option<String> {
    if let Some((start, end)) = range
        && (at < start || at >= end)
    {
        return Some(format!(
            "must be inside the cut (in <= at < out), got {secs}"
        ));
    }
    if let Some((k, earlier)) = previous
        && at <= earlier
    {
        return Some(format!("must be greater than zoom[{k}].at"));
    }
    let grid = grid?;
    let frame = grid.boundary(at);
    // A cut shorter than 1 frame is reported on its own, so skip it here.
    if let Some((start, end)) = range
        && grid.boundary(start) < grid.boundary(end)
        && frame >= grid.boundary(end)
    {
        return Some(format!(
            "must be at least 1 frame before out at {} fps, got {secs}",
            grid.fps()
        ));
    }
    if let Some((k, earlier)) = previous
        && frame <= grid.boundary(earlier)
    {
        return Some(format!(
            "must be at least 1 frame after zoom[{k}].at at {} fps, got {secs}",
            grid.fps()
        ));
    }
    None
}

/// Returns how many output frames the hook covers, if it is valid.
fn check_hook(
    report: &mut Report,
    hook: &Hook,
    grid: Option<FrameGrid>,
    output_frames: Option<u64>,
) -> Option<u64> {
    if hook.text().trim().is_empty() {
        report.push("hook.text", "must not be blank");
    }
    let dur = hook.dur();
    let Some(seconds) = Rational::from_secs_f64(dur)
        .filter(|seconds| *seconds > Rational::ZERO && dur <= MAX_SECONDS)
    else {
        report.push(
            "hook.dur",
            format!("must be greater than 0 and at most {MAX_SECONDS} seconds, got {dur}"),
        );
        return None;
    };
    let grid = grid?;
    let frames = u64::try_from(grid.boundary(seconds)).unwrap_or_default();
    if frames == 0 {
        report.push(
            "hook.dur",
            format!(
                "must be at least 1 frame long at {} fps, got {dur}",
                grid.fps()
            ),
        );
        return None;
    }
    if let Some(total) = output_frames
        && frames > total
    {
        report.push(
            "hook.dur",
            format!(
                "must not be longer than the output ({:.3} s), got {dur}",
                grid.time(total).to_f64()
            ),
        );
        return None;
    }
    Some(frames)
}

fn check_captions(report: &mut Report, captions: &Captions) {
    if captions.words().is_empty() {
        report.push("captions.words", "must not be empty");
    }
    if captions.fixes().is_some_and(str::is_empty) {
        report.push("captions.fixes", "must not be empty");
    }
    for (k, keyword) in captions.keywords().iter().enumerate() {
        if keyword.trim().is_empty() {
            report.push(format!("captions.keywords[{k}]"), "must not be blank");
        }
    }
    if captions.font().is_empty() {
        report.push("captions.font", "must not be empty");
    }
    if !is_hex_colour(captions.active()) {
        report.push(
            "captions.active",
            format!(
                "must be a #RRGGBB colour like \"#FFC400\", got {:?}",
                captions.active()
            ),
        );
    }
    if captions.stroke() > MAX_STROKE {
        report.push(
            "captions.stroke",
            format!(
                "must be between 0 and {MAX_STROKE}, got {}",
                captions.stroke()
            ),
        );
    }
}

/// Reads a time in seconds, which must be in `0..=MAX_SECONDS`.
fn check_seconds(report: &mut Report, field: &str, secs: f64) -> Option<Rational> {
    let seconds = Some(secs)
        .filter(|secs| (0.0..=MAX_SECONDS).contains(secs))
        .and_then(Rational::from_secs_f64);
    if seconds.is_none() {
        report.push(
            field,
            format!("must be between 0 and {MAX_SECONDS} seconds, got {secs}"),
        );
    }
    seconds
}

fn is_valid_id(id: &str) -> bool {
    (1..=MAX_ID_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn is_hex_colour(text: &str) -> bool {
    text.strip_prefix('#')
        .is_some_and(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}
