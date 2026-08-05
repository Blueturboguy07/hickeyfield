//! The compositor: a pure FFmpeg filtergraph builder.
//!
//! This module spawns nothing. It turns a [`Timeline`] into an argv vector and
//! stops — [`crate::ffmpeg`] runs it. That separation is deliberate and is the
//! main reason the compositor is cheap to trust: every interesting decision
//! (crossfade offsets, crop versus pad, caption escaping, audio ducking) is a
//! pure function of the timeline, so it can be tested exhaustively in
//! milliseconds without touching a file.
//!
//! Halation was originally planned around a hand-written WebCodecs frame
//! pipeline in the browser. Going native replaced roughly 2,000 lines of frame
//! plumbing — plus a 2 GB wasm ceiling, a COOP/COEP requirement and a Firefox
//! AAC gap — with a string builder. There is no length or resolution limit
//! here: a 30-minute 4K export is the same graph as a 5-second one.
//!
//! ## The shape of a graph
//!
//! ```text
//! per clip:  [i:v] setpts, fps, scale+crop|pad, setsar        -> [ci]
//! join:      [c0][c1] concat | xfade                          -> [vseq]
//! captions:  [vseq] drawtext, drawtext, ...                   -> [vcap]
//! finish:    [vcap] format=yuv420p                            -> [vout]
//!
//! per track: [j:a] asetpts, volume, aformat                   -> [aj]
//! duck:      [music][narration] sidechaincompress             -> [ducked]
//! mix:       [narration][ducked][sfx] amix, apad, aformat     -> [aout]
//! ```

use crate::ffmpeg::{Encoder, Quality};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Everything needed to produce one output file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub clips: Vec<Clip>,
    pub audio: Vec<AudioTrack>,
    pub captions: Vec<Caption>,
    pub output: OutputSpec,
}

/// One source video, trimmed to a sub-range.
///
/// `trim_start`/`trim_end` are positions in the *source* file, so the caller
/// must already know the source duration. That is not a limitation worth
/// engineering around: crossfade offsets cannot be computed without it, and the
/// library already probes every asset it imports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub path: PathBuf,
    pub trim_start: f64,
    pub trim_end: f64,
    /// How this clip enters — i.e. the transition **from the previous clip into
    /// this one**. Ignored on the first clip, which has nothing to come from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
}

impl Clip {
    pub fn new(path: impl Into<PathBuf>, trim_start: f64, trim_end: f64) -> Self {
        Clip {
            path: path.into(),
            trim_start,
            trim_end,
            transition: None,
        }
    }

    /// Same clip, entered with a crossfade from whatever precedes it.
    pub fn crossfade_in(mut self, seconds: f64) -> Self {
        self.transition = Some(Transition::Crossfade { seconds });
        self
    }

    pub fn duration(&self) -> f64 {
        (self.trim_end - self.trim_start).max(0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Transition {
    Crossfade { seconds: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioTrack {
    pub path: PathBuf,
    pub role: AudioRole,
    /// Gain in dB. Negative attenuates; 0 leaves the source alone.
    pub gain_db: f64,
}

impl AudioTrack {
    pub fn new(path: impl Into<PathBuf>, role: AudioRole, gain_db: f64) -> Self {
        AudioTrack {
            path: path.into(),
            role,
            gain_db,
        }
    }
}

/// What a track is for. This is not cosmetic — it decides the mix topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioRole {
    /// Voice. Drives the ducking sidechain and is never itself ducked.
    Narration,
    /// Bed music. Ducked under narration when both are present.
    Music,
    /// Spot effects. Mixed flat; too short to be worth ducking.
    Sfx,
}

/// A burned-in caption.
///
/// Burned in rather than a subtitle track because the output of this app is
/// usually going straight to a social platform, where a soft subtitle track is
/// discarded on upload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    pub text: String,
    /// Seconds on the *output* timeline, i.e. after trimming and crossfades.
    pub start: f64,
    pub end: f64,
    pub style: CaptionStyle,
}

impl Caption {
    pub fn new(text: impl Into<String>, start: f64, end: f64) -> Self {
        Caption {
            text: text.into(),
            start,
            end,
            style: CaptionStyle::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionStyle {
    /// An explicit font file. Strongly preferred: `font=` needs fontconfig,
    /// which is not present in every static build we might bundle, and the app
    /// already ships its own fonts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_file: Option<PathBuf>,
    pub font_size: u32,
    pub font_color: String,
    /// A translucent plate behind the text. When `None`, an outline is drawn
    /// instead — one of the two is mandatory, because white text on unknown
    /// footage is illegible roughly half the time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub box_color: Option<String>,
    pub box_padding: u32,
    pub border_width: u32,
    pub border_color: String,
    pub position: CaptionPosition,
    /// Distance in pixels from the edge the caption is anchored to.
    pub margin: u32,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        CaptionStyle {
            font_file: None,
            font_size: 48,
            font_color: "white".into(),
            box_color: None,
            box_padding: 16,
            border_width: 3,
            border_color: "black@0.85".into(),
            position: CaptionPosition::Bottom,
            margin: 96,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptionPosition {
    Top,
    Middle,
    Bottom,
}

impl CaptionPosition {
    fn y_expr(self, margin: u32) -> String {
        match self {
            CaptionPosition::Top => margin.to_string(),
            CaptionPosition::Middle => "(h-text_h)/2".to_string(),
            CaptionPosition::Bottom => format!("h-text_h-{margin}"),
        }
    }
}

/// How a source frame is fitted into an output frame whose aspect ratio differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AspectMode {
    /// Letterbox: the whole frame survives, bars fill the remainder.
    Fit,
    /// Crop: the frame fills the output, the overflow is cut.
    Fill,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub aspect: AspectMode,
    pub encoder: Encoder,
    /// The quality knob — CRF for encoders that have one, a bitrate otherwise.
    /// See [`Encoder::quality_args`], which does the per-encoder translation.
    pub crf_or_bitrate: Quality,
    pub path: PathBuf,
}

impl OutputSpec {
    pub fn new(width: u32, height: u32, fps: u32, path: impl Into<PathBuf>) -> Self {
        OutputSpec {
            width,
            height,
            fps,
            aspect: AspectMode::Fill,
            encoder: Encoder::Libx264,
            crf_or_bitrate: Quality::default(),
            path: path.into(),
        }
    }

    /// The scale-and-reframe stage.
    ///
    /// The important part is what is *absent*: a bare `scale=W:H`. That is the
    /// squash — it happily turns a 16:9 face into a 9:16 face by stretching it,
    /// and it looks wrong without ever failing. Both branches below preserve
    /// the source aspect ratio and then either cut or pad the difference.
    fn reframe(&self) -> String {
        let (w, h) = (self.width, self.height);
        match self.aspect {
            // `increase` scales until the frame *covers* the output, then crop
            // takes the centre. Centre-crop is the right default for generated
            // footage, which is almost always subject-centred.
            AspectMode::Fill => {
                format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}")
            }
            // `decrease` scales until the frame *fits*, then pad fills the rest.
            AspectMode::Fit => format!(
                "scale={w}:{h}:force_original_aspect_ratio=decrease,\
                 pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black"
            ),
        }
    }
}

/// A deliverable aspect ratio. The same timeline renders to all three by
/// changing only the reframe stage and the output size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AspectRatio {
    /// 16:9 — landscape.
    Wide,
    /// 9:16 — vertical.
    Vertical,
    /// 1:1 — square.
    Square,
}

impl AspectRatio {
    pub const ALL: [AspectRatio; 3] = [
        AspectRatio::Wide,
        AspectRatio::Vertical,
        AspectRatio::Square,
    ];

    pub const fn ratio(self) -> (u32, u32) {
        match self {
            AspectRatio::Wide => (16, 9),
            AspectRatio::Vertical => (9, 16),
            AspectRatio::Square => (1, 1),
        }
    }

    /// Used as the file-name suffix, so it has to stay filesystem-safe.
    pub const fn label(self) -> &'static str {
        match self {
            AspectRatio::Wide => "16x9",
            AspectRatio::Vertical => "9x16",
            AspectRatio::Square => "1x1",
        }
    }

    /// Pixel dimensions with `long_edge` as the longer side.
    ///
    /// Rounded down to even numbers: yuv420p subsamples chroma by two, so an
    /// odd dimension is rejected outright by every H.264 encoder.
    pub fn dimensions(self, long_edge: u32) -> (u32, u32) {
        let (rw, rh) = self.ratio();
        let short = even(long_edge * rw.min(rh) / rw.max(rh));
        let long = even(long_edge);
        match self {
            AspectRatio::Wide => (long, short),
            AspectRatio::Vertical => (short, long),
            AspectRatio::Square => (long, long),
        }
    }
}

const fn even(n: u32) -> u32 {
    n & !1
}

/// One rendered aspect variant: the argv plus the metadata a caller needs to
/// report on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub ratio: AspectRatio,
    pub width: u32,
    pub height: u32,
    pub path: PathBuf,
    pub args: Vec<String>,
}

/// One join in the assembled sequence, describing how clip `i+1` attaches to
/// everything before it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Join {
    /// `None` is a hard cut.
    crossfade: Option<f64>,
    /// Where the transition starts, on the accumulated output's timeline.
    offset: f64,
}

impl Timeline {
    pub fn new(clips: Vec<Clip>, output: OutputSpec) -> Self {
        Timeline {
            clips,
            audio: Vec::new(),
            captions: Vec::new(),
            output,
        }
    }

    /// Work out every join and the resulting length.
    ///
    /// The crossfade maths lives here, once, so the graph builder and
    /// [`Timeline::total_duration`] cannot drift apart.
    ///
    /// `xfade`'s `offset` is the moment the transition *begins*, measured on
    /// the first input's timeline — not the moment it ends, and not a position
    /// in the second clip. An xfade of two clips is `d1 + d2 - transition`
    /// long, so when several are chained the accumulator has to carry the
    /// already-overlapped length. Using the raw sum instead pushes every
    /// subsequent offset late by the total of the preceding transitions, which
    /// does not error: it silently swallows the head of a later clip.
    fn joins(&self) -> (Vec<Join>, f64) {
        let Some(first) = self.clips.first() else {
            return (Vec::new(), 0.0);
        };
        let mut acc = first.duration();
        let mut joins = Vec::with_capacity(self.clips.len().saturating_sub(1));
        for clip in self.clips.iter().skip(1) {
            let requested = match clip.transition {
                Some(Transition::Crossfade { seconds }) => seconds,
                None => 0.0,
            };
            // A transition cannot be longer than either side of it: xfade needs
            // a non-negative offset on the left and at least `duration` of
            // material on the right. Clamping keeps a sloppy timeline
            // renderable; `validate()` is where the user is told about it.
            let d = requested.min(acc).min(clip.duration());
            if d > 0.0 {
                joins.push(Join {
                    crossfade: Some(d),
                    offset: (acc - d).max(0.0),
                });
                acc += clip.duration() - d;
            } else {
                joins.push(Join {
                    crossfade: None,
                    offset: acc,
                });
                acc += clip.duration();
            }
        }
        (joins, acc)
    }

    /// Output length in seconds — the sum of the trimmed clips minus every
    /// crossfade, since a crossfade is an overlap rather than an insertion.
    pub fn total_duration(&self) -> f64 {
        self.joins().1
    }

    /// The complete `-filter_complex` string.
    ///
    /// Public because a broken graph is the single most common failure and the
    /// first thing to look at is the graph itself.
    pub fn filtergraph(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        self.video_graph(&mut parts);
        self.audio_graph(&mut parts);
        parts.join(";")
    }

    fn video_graph(&self, parts: &mut Vec<String>) {
        let o = &self.output;
        if self.clips.is_empty() {
            // Degenerate but parseable, so a caller who skipped `validate()`
            // gets an empty file rather than an ffmpeg syntax error they cannot
            // act on.
            parts.push(format!(
                "color=c=black:s={}x{}:r={}:d=1,format=yuv420p[vout]",
                o.width, o.height, o.fps
            ));
            return;
        }

        // Normalise every clip to identical size, rate, SAR and timebase.
        // `xfade` and `concat` both refuse mismatched inputs, and `setpts=
        // PTS-STARTPTS` is not optional: input seeking with `-ss` leaves the
        // first PTS at the seek position, which would place the whole clip that
        // far into the output.
        for (i, _clip) in self.clips.iter().enumerate() {
            parts.push(format!(
                "[{i}:v]setpts=PTS-STARTPTS,fps={},{},setsar=1[c{i}]",
                o.fps,
                o.reframe()
            ));
        }

        let (joins, _) = self.joins();
        let mut label = "c0".to_string();
        if joins.iter().all(|j| j.crossfade.is_none()) && self.clips.len() > 1 {
            // All hard cuts: one n-way concat is cheaper and easier to read
            // than a pairwise chain.
            let inputs: String = (0..self.clips.len()).map(|i| format!("[c{i}]")).collect();
            parts.push(format!(
                "{inputs}concat=n={}:v=1:a=0[vseq]",
                self.clips.len()
            ));
            label = "vseq".into();
        } else {
            for (k, join) in joins.iter().enumerate() {
                let i = k + 1;
                let next = format!("x{i}");
                match join.crossfade {
                    Some(d) => parts.push(format!(
                        "[{label}][c{i}]xfade=transition=fade:duration={}:offset={}[{next}]",
                        secs(d),
                        secs(join.offset)
                    )),
                    None => parts.push(format!("[{label}][c{i}]concat=n=2:v=1:a=0[{next}]")),
                }
                label = next;
            }
        }

        if !self.captions.is_empty() {
            let chain: Vec<String> = self.captions.iter().map(drawtext).collect();
            parts.push(format!("[{label}]{}[vcap]", chain.join(",")));
            label = "vcap".into();
        }

        // Always a real filter, never a no-op relabel: this guarantees `[vout]`
        // exists for every shape of timeline, and pins the pixel format that
        // consumer players actually decode.
        parts.push(format!("[{label}]format=yuv420p[vout]"));
    }

    fn audio_graph(&self, parts: &mut Vec<String>) {
        if self.audio.is_empty() {
            return;
        }
        // Clip audio is deliberately ignored. Generated video is usually mute,
        // and `concat` with `a=1` fails outright on an input with no audio
        // stream — so honouring clip audio would make the common case break.
        let base = self.clips.len();
        let (mut narration, mut music, mut sfx) = (Vec::new(), Vec::new(), Vec::new());
        for (j, track) in self.audio.iter().enumerate() {
            let label = format!("a{j}");
            // A common format on every track is what lets `sidechaincompress`
            // and `amix` work at all: they reject inputs whose sample rate or
            // channel layout disagree, and a mono voiceover next to a stereo
            // bed is the normal case, not an edge case.
            parts.push(format!(
                "[{}:a]asetpts=PTS-STARTPTS,volume={}dB,{AFORMAT}[{label}]",
                base + j,
                secs(track.gain_db)
            ));
            match track.role {
                AudioRole::Narration => narration.push(label),
                AudioRole::Music => music.push(label),
                AudioRole::Sfx => sfx.push(label),
            }
        }

        let narration = mix_group(parts, &narration, "narration");
        let music = mix_group(parts, &music, "music");
        let sfx = mix_group(parts, &sfx, "sfx");

        let mut inputs: Vec<String> = Vec::new();
        match (narration, music) {
            (Some(n), Some(m)) => {
                // Ducking. `sidechaincompress` takes the signal to be
                // compressed first and the trigger second, and consumes the
                // trigger — so narration has to be split, one copy to drive the
                // compressor and one to actually be heard.
                parts.push(format!("[{n}]asplit=2[narr_mix][narr_sc]"));
                // threshold 0.03 (~-30 dBFS) so ordinary speech trips it rather
                // than only shouting; ratio 8 for a firm dip; attack 20ms so
                // the duck lands under the first syllable instead of after it;
                // release 300ms so the bed does not pump between words.
                parts.push(format!(
                    "[{m}][narr_sc]sidechaincompress=threshold=0.03:ratio=8:attack=20:\
                     release=300:makeup=1:detection=rms[ducked]"
                ));
                inputs.push("narr_mix".into());
                inputs.push("ducked".into());
            }
            (n, m) => {
                inputs.extend(n);
                inputs.extend(m);
            }
        }
        inputs.extend(sfx);

        let mixed = if inputs.len() > 1 {
            let refs: String = inputs.iter().map(|l| format!("[{l}]")).collect();
            // `normalize=0` matters: amix defaults to dividing every input by
            // the input count, so adding a music bed would quietly halve the
            // narration. `dropout_transition=0` stops it ramping the gain back
            // up each time a shorter track ends.
            parts.push(format!(
                "{refs}amix=inputs={}:normalize=0:dropout_transition=0[amixed]",
                inputs.len()
            ));
            "amixed".to_string()
        } else {
            inputs.into_iter().next().unwrap_or_default()
        };

        // `apad` plus `-shortest` is the pairing that makes length behave:
        // apad extends the mix with silence so narration shorter than the video
        // cannot end the file early, and `-shortest` then stops at the last
        // video frame so music longer than the video cannot extend it.
        parts.push(format!("[{mixed}]apad,{AFORMAT}[aout]"));
    }

    /// The full ffmpeg argv, excluding the binary itself.
    ///
    /// Call [`Timeline::validate`] first — this function is total and will
    /// happily emit a graph for a nonsensical timeline.
    pub fn to_args(&self) -> Vec<String> {
        let o = &self.output;
        let mut a: Vec<String> = Vec::new();
        a.extend(
            [
                "-hide_banner",
                "-nostdin", // a sidecar must never try to read the app's stdin
                "-y",
                "-loglevel",
                "error",
                "-progress",
                "pipe:1",
            ]
            .map(String::from),
        );

        for clip in &self.clips {
            // `-ss`/`-t` before `-i` are *input* options, so ffmpeg seeks in the
            // container instead of decoding and discarding everything up to the
            // trim point. On a long source that is the difference between a
            // render finishing and a render appearing to hang. Modern ffmpeg
            // input seeking is frame-accurate, so there is no precision cost.
            a.push("-ss".into());
            a.push(secs(clip.trim_start));
            a.push("-t".into());
            a.push(secs(clip.duration()));
            a.push("-i".into());
            a.push(path_arg(&clip.path));
        }
        for track in &self.audio {
            a.push("-i".into());
            a.push(path_arg(&track.path));
        }

        a.push("-filter_complex".into());
        a.push(self.filtergraph());
        a.push("-map".into());
        a.push("[vout]".into());
        if !self.audio.is_empty() {
            a.push("-map".into());
            a.push("[aout]".into());
        }

        a.push("-r".into());
        a.push(o.fps.to_string());
        a.push("-c:v".into());
        a.push(o.encoder.as_str().into());
        a.extend(o.encoder.quality_args(o.crf_or_bitrate));
        a.push("-pix_fmt".into());
        a.push("yuv420p".into());

        if !self.audio.is_empty() {
            a.extend(
                ["-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-shortest"].map(String::from),
            );
        }
        // Moves the index to the front so the file starts playing before it has
        // fully downloaded — the difference between a shareable file and one
        // that spins for ten seconds on someone else's phone.
        a.push("-movflags".into());
        a.push("+faststart".into());
        a.push(path_arg(&o.path));
        a
    }

    /// The same timeline rendered to several aspect ratios.
    ///
    /// Only the reframe stage and the output size change; clips, trims,
    /// transitions, captions and the audio mix are untouched. The long edge is
    /// taken from the timeline's own output size, so a 1080p timeline yields
    /// 1920x1080, 1080x1920 and 1080x1080.
    pub fn render_variants(&self, ratios: &[AspectRatio]) -> Vec<Variant> {
        let long_edge = self.output.width.max(self.output.height);
        ratios
            .iter()
            .map(|&ratio| {
                let (width, height) = ratio.dimensions(long_edge);
                let path = suffixed(&self.output.path, ratio.label());
                let mut t = self.clone();
                t.output.width = width;
                t.output.height = height;
                t.output.path.clone_from(&path);
                Variant {
                    ratio,
                    width,
                    height,
                    path,
                    args: t.to_args(),
                }
            })
            .collect()
    }

    /// Everything wrong with this timeline, in human-readable form. Empty means
    /// it will render as written.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.clips.is_empty() {
            problems.push("timeline has no clips".into());
        }
        if self.output.width == 0 || self.output.height == 0 {
            problems.push("output size is zero".into());
        }
        if self.output.width % 2 != 0 || self.output.height % 2 != 0 {
            problems.push(format!(
                "output size {}x{} is odd; yuv420p requires even dimensions",
                self.output.width, self.output.height
            ));
        }
        if self.output.fps == 0 {
            problems.push("output fps is zero".into());
        }
        if self.clips.first().is_some_and(|c| c.transition.is_some()) {
            problems.push("the first clip has a transition in, which has nothing to come from and will be ignored".into());
        }

        let mut acc = self.clips.first().map_or(0.0, Clip::duration);
        for (i, clip) in self.clips.iter().enumerate() {
            if clip.duration() <= 0.0 {
                problems.push(format!(
                    "clip {i} has no length (trim_start {} >= trim_end {})",
                    clip.trim_start, clip.trim_end
                ));
            }
            if clip.trim_start < 0.0 {
                problems.push(format!("clip {i} has a negative trim_start"));
            }
            if i == 0 {
                continue;
            }
            if let Some(Transition::Crossfade { seconds }) = clip.transition {
                let limit = acc.min(clip.duration());
                if seconds > limit {
                    problems.push(format!(
                        "clip {i}: {seconds}s crossfade is longer than the {limit}s available and will be clamped"
                    ));
                }
                acc += clip.duration() - seconds.min(limit);
            } else {
                acc += clip.duration();
            }
        }

        let total = self.total_duration();
        for (i, cap) in self.captions.iter().enumerate() {
            if cap.end <= cap.start {
                problems.push(format!("caption {i} ends at or before it starts"));
            }
            if cap.start > total {
                problems.push(format!(
                    "caption {i} starts at {}s, past the {total}s end of the timeline",
                    cap.start
                ));
            }
            if cap.text.is_empty() {
                problems.push(format!("caption {i} is empty"));
            }
        }
        problems
    }
}

/// Forced on every audio stage. 48 kHz stereo float is what the AAC encoder
/// wants anyway, and pinning it early means format negotiation cannot silently
/// insert a resampler between the sidechain and the mix.
const AFORMAT: &str = "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo";

fn mix_group(parts: &mut Vec<String>, labels: &[String], out: &str) -> Option<String> {
    match labels {
        [] => None,
        [only] => Some(only.clone()),
        many => {
            let refs: String = many.iter().map(|l| format!("[{l}]")).collect();
            parts.push(format!(
                "{refs}amix=inputs={}:normalize=0:dropout_transition=0[{out}]",
                many.len()
            ));
            Some(out.to_string())
        }
    }
}

fn drawtext(cap: &Caption) -> String {
    let s = &cap.style;
    let mut f = String::from("drawtext=");
    match &s.font_file {
        // The font path goes through the same escaper as the text, and it has
        // to: a Windows path is `C:\Windows\Fonts\arial.ttf`, whose drive colon
        // ends the option and whose backslashes are escapes. Unescaped, the
        // graph fails to parse on Windows and works everywhere else — the worst
        // possible distribution of a bug.
        Some(p) => f.push_str(&format!(
            "fontfile={}:",
            escape_filter_value(&p.to_string_lossy())
        )),
        None => f.push_str("font=Sans:"),
    }
    f.push_str(&format!("text={}:", escape_filter_value(&cap.text)));
    // `expansion=none` is a safety property, not a formatting preference.
    // Under the default `normal` expansion, drawtext treats the text as a
    // template: a caption containing `%{localtime}` would render the user's
    // clock, and `%{eif:...}` with bad syntax aborts the render. Caption text
    // is content, never a format string. Turning expansion off also removes an
    // entire escaping level, so `%` needs no special handling at all.
    f.push_str("expansion=none:");
    f.push_str(&format!("fontsize={}:", s.font_size));
    f.push_str(&format!("fontcolor={}:", s.font_color));
    match &s.box_color {
        Some(colour) => f.push_str(&format!(
            "box=1:boxcolor={colour}:boxborderw={}:",
            s.box_padding
        )),
        None => f.push_str(&format!(
            "borderw={}:bordercolor={}:",
            s.border_width, s.border_color
        )),
    }
    f.push_str(&format!(
        "x=(w-text_w)/2:y={}:",
        s.position.y_expr(s.margin)
    ));
    // The `enable` expression contains commas, which end a filter at the graph
    // level. Single quotes keep them inside the value; this is the one place a
    // quoted value is easier than escaping.
    f.push_str(&format!(
        "enable='between(t,{},{})'",
        secs(cap.start),
        secs(cap.end)
    ));
    f
}

/// Escape a string for use as a filter option value inside `-filter_complex`.
///
/// This is the single easiest thing in the module to get wrong, and it fails
/// loudly for the author and silently for the user: a caption reading
/// "Revenue: up 50%" parses as an option named `Revenue` unless the colon is
/// escaped, and then the whole render dies with an unhelpful "Invalid argument".
///
/// There are **two** parsers between this string and the option value, and they
/// need different amounts of backslash, which is why a single uniform rule does
/// not work. Verified against ffmpeg 8.1 by rendering each case twice — once
/// via `textfile=` (which needs no escaping) and once inline — and comparing
/// frame hashes:
///
/// | character | emitted as | why |
/// |---|---|---|
/// | `\` | `\\\\` | escaped once for each parser |
/// | `'` | `\\\'` | quote-opener at the graph level *and* the option level |
/// | `:` | `\\:` | option separator; the graph parser eats one backslash first |
/// | `,` `;` `[` `]` | `\,` | graph-level only — a second backslash breaks it |
/// | leading/trailing space | `\\ ` | both parsers trim unescaped edge whitespace |
/// | `%` `=` | unchanged | only special under text expansion, which we disable |
///
/// Note the asymmetry: over-escaping the graph-level characters is just as
/// broken as under-escaping the option-level ones.
pub fn escape_filter_value(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    // ffmpeg's tokeniser strips unescaped whitespace from both ends of a value.
    let is_edge_ws = |c: &char| matches!(c, ' ' | '\t' | '\n' | '\r');
    let first = chars.iter().position(|c| !is_edge_ws(c));
    let last = chars.iter().rposition(|c| !is_edge_ws(c));

    let mut out = String::with_capacity(raw.len() + 8);
    for (i, c) in chars.iter().enumerate() {
        let inside = matches!((first, last), (Some(f), Some(l)) if i >= f && i <= l);
        if !inside {
            out.push_str("\\\\");
            out.push(*c);
            continue;
        }
        match c {
            '\\' => out.push_str(r"\\\\"),
            '\'' => out.push_str(r"\\\'"),
            ':' => out.push_str(r"\\:"),
            ',' | ';' | '[' | ']' => {
                out.push('\\');
                out.push(*c);
            }
            other => out.push(*other),
        }
    }
    out
}

/// Seconds as ffmpeg wants them: fixed precision, no exponent, no trailing
/// zeros. `format!("{}", 0.1+0.2)` produces `0.30000000000000004`, which ffmpeg
/// accepts but which makes every generated graph unreadable in a bug report.
fn secs(v: f64) -> String {
    let s = format!("{v:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    match trimmed {
        "" | "-0" => "0".to_string(),
        t => t.to_string(),
    }
}

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `out.mp4` + `9x16` -> `out-9x16.mp4`.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    let name = match (stem, ext) {
        (Some(s), Some(e)) => format!("{s}-{suffix}.{e}"),
        (Some(s), None) => format!("{s}-{suffix}"),
        _ => format!("{suffix}.mp4"),
    };
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> OutputSpec {
        OutputSpec::new(1920, 1080, 30, "/out/final.mp4")
    }

    fn two_clips() -> Vec<Clip> {
        vec![
            Clip::new("/media/a.mp4", 0.0, 4.0),
            Clip::new("/media/b.mp4", 1.0, 4.0),
        ]
    }

    /// The value of `-filter_complex`, for assertions.
    fn graph_of(args: &[String]) -> String {
        let i = args
            .iter()
            .position(|a| a == "-filter_complex")
            .expect("every render has a filtergraph");
        args[i + 1].clone()
    }

    // -- concat ------------------------------------------------------------

    #[test]
    fn two_clip_concat_builds_a_complete_graph() {
        let t = Timeline::new(two_clips(), spec());
        assert!(t.validate().is_empty(), "{:?}", t.validate());
        let g = t.filtergraph();

        // Both inputs normalised, joined once, terminated at [vout].
        assert!(g.contains("[0:v]setpts=PTS-STARTPTS,fps=30"), "{g}");
        assert!(g.contains("[1:v]setpts=PTS-STARTPTS,fps=30"), "{g}");
        assert!(g.contains("[c0][c1]concat=n=2:v=1:a=0[vseq]"), "{g}");
        assert!(g.ends_with("format=yuv420p[vout]"), "{g}");
        // No audio anywhere: the timeline has none.
        assert!(!g.contains("[aout]"), "{g}");

        let args = t.to_args();
        assert!(args.contains(&"[vout]".to_string()));
        assert!(!args.contains(&"[aout]".to_string()));
        assert_eq!(args.last().unwrap(), "/out/final.mp4");
    }

    #[test]
    fn trims_become_input_seeks_not_filter_trims() {
        // Input-side seeking is the whole reason a 30-minute source does not
        // take 30 minutes to skip past.
        let args = Timeline::new(two_clips(), spec()).to_args();
        let joined = args.join(" ");
        assert!(joined.contains("-ss 0 -t 4 -i /media/a.mp4"), "{joined}");
        assert!(joined.contains("-ss 1 -t 3 -i /media/b.mp4"), "{joined}");
        assert!(!graph_of(&args).contains("trim="), "no filter-side trim");
    }

    #[test]
    fn a_single_clip_still_terminates_at_vout() {
        let t = Timeline::new(vec![Clip::new("/m/a.mp4", 0.0, 2.0)], spec());
        let g = t.filtergraph();
        assert!(g.contains("[c0]format=yuv420p[vout]"), "{g}");
        assert!(!g.contains("concat"), "{g}");
    }

    #[test]
    fn an_empty_timeline_is_flagged_but_still_emits_a_parseable_graph() {
        let t = Timeline::new(vec![], spec());
        assert!(t.validate().iter().any(|p| p.contains("no clips")));
        assert!(t.filtergraph().ends_with("[vout]"));
    }

    // -- crossfade ---------------------------------------------------------

    #[test]
    fn crossfade_offset_is_the_running_length_minus_the_transition() {
        // Clip A is 4s, the transition is 1s, so the fade must begin at 3s.
        // Off by one transition here and the graph still renders — it just eats
        // the first second of clip B, which nobody notices until delivery.
        let clips = vec![
            Clip::new("/m/a.mp4", 0.0, 4.0),
            Clip::new("/m/b.mp4", 0.0, 3.0).crossfade_in(1.0),
        ];
        let t = Timeline::new(clips, spec());
        let g = t.filtergraph();
        assert!(
            g.contains("[c0][c1]xfade=transition=fade:duration=1:offset=3[x1]"),
            "{g}"
        );
        assert!((t.total_duration() - 6.0).abs() < 1e-9, "4 + 3 - 1 = 6");
    }

    #[test]
    fn chained_crossfades_accumulate_the_overlapped_length() {
        // Three 4s clips, two 1s crossfades.
        //   first  xfade at 4 - 1        = 3, output now 4 + 4 - 1 = 7
        //   second xfade at 7 - 1        = 6, output now 7 + 4 - 1 = 10
        // The seductive wrong answer for the second offset is 8 (the raw sum
        // 4 + 4 minus the transition), which is late by exactly the first
        // transition and silently clips two seconds out of the last shot.
        let clips = vec![
            Clip::new("/m/a.mp4", 0.0, 4.0),
            Clip::new("/m/b.mp4", 0.0, 4.0).crossfade_in(1.0),
            Clip::new("/m/c.mp4", 0.0, 4.0).crossfade_in(1.0),
        ];
        let t = Timeline::new(clips, spec());
        let g = t.filtergraph();
        assert!(
            g.contains("xfade=transition=fade:duration=1:offset=3[x1]"),
            "{g}"
        );
        assert!(
            g.contains("[x1][c2]xfade=transition=fade:duration=1:offset=6[x2]"),
            "second offset must be 6, not 8 — {g}"
        );
        assert!(!g.contains("offset=8"), "{g}");
        assert!((t.total_duration() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn crossfades_and_hard_cuts_can_be_mixed() {
        let clips = vec![
            Clip::new("/m/a.mp4", 0.0, 2.0),
            Clip::new("/m/b.mp4", 0.0, 2.0), // hard cut
            Clip::new("/m/c.mp4", 0.0, 2.0).crossfade_in(0.5),
        ];
        let t = Timeline::new(clips, spec());
        let g = t.filtergraph();
        assert!(g.contains("[c0][c1]concat=n=2:v=1:a=0[x1]"), "{g}");
        // The cut contributes its full length, so the fade starts at 4 - 0.5.
        assert!(
            g.contains("[x1][c2]xfade=transition=fade:duration=0.5:offset=3.5[x2]"),
            "{g}"
        );
        assert!((t.total_duration() - 5.5).abs() < 1e-9);
    }

    #[test]
    fn a_crossfade_longer_than_its_clip_is_clamped_and_reported() {
        // xfade rejects a negative offset outright, so an over-long transition
        // has to be clamped rather than passed through.
        let clips = vec![
            Clip::new("/m/a.mp4", 0.0, 1.0),
            Clip::new("/m/b.mp4", 0.0, 5.0).crossfade_in(3.0),
        ];
        let t = Timeline::new(clips, spec());
        let g = t.filtergraph();
        assert!(g.contains("duration=1:offset=0"), "{g}");
        assert!(
            !g.contains("offset=-"),
            "offsets can never be negative: {g}"
        );
        assert!(
            t.validate().iter().any(|p| p.contains("clamped")),
            "{:?}",
            t.validate()
        );
    }

    #[test]
    fn a_zero_length_crossfade_degrades_to_a_cut() {
        let clips = vec![
            Clip::new("/m/a.mp4", 0.0, 2.0),
            Clip::new("/m/b.mp4", 0.0, 2.0).crossfade_in(0.0),
        ];
        let g = Timeline::new(clips, spec()).filtergraph();
        assert!(g.contains("concat=n=2"), "{g}");
        assert!(!g.contains("xfade"), "xfade=duration=0 is invalid: {g}");
    }

    #[test]
    fn a_transition_on_the_first_clip_is_reported() {
        let clips = vec![
            Clip::new("/m/a.mp4", 0.0, 2.0).crossfade_in(0.5),
            Clip::new("/m/b.mp4", 0.0, 2.0),
        ];
        let t = Timeline::new(clips, spec());
        assert!(t.validate().iter().any(|p| p.contains("first clip")));
        assert!((t.total_duration() - 4.0).abs() < 1e-9, "it is ignored");
    }

    // -- aspect ------------------------------------------------------------

    #[test]
    fn vertical_from_a_wide_source_crops_rather_than_squashing() {
        let mut out = OutputSpec::new(1080, 1920, 30, "/out/v.mp4");
        out.aspect = AspectMode::Fill;
        let g = Timeline::new(two_clips(), out).filtergraph();
        assert!(
            g.contains("scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920"),
            "{g}"
        );
        // The squash is a bare `scale=W:H` with no aspect preservation. Its
        // absence is the actual assertion here.
        assert!(!g.contains("scale=1080:1920,"), "that would stretch: {g}");
        assert!(!g.contains("pad="), "Fill must not letterbox: {g}");
    }

    #[test]
    fn fit_letterboxes_rather_than_stretching() {
        let mut out = OutputSpec::new(1080, 1920, 30, "/out/v.mp4");
        out.aspect = AspectMode::Fit;
        let g = Timeline::new(two_clips(), out).filtergraph();
        assert!(
            g.contains("scale=1080:1920:force_original_aspect_ratio=decrease"),
            "{g}"
        );
        assert!(
            g.contains("pad=1080:1920:(ow-iw)/2:(oh-ih)/2:color=black"),
            "{g}"
        );
        assert!(!g.contains("crop="), "Fit must not crop: {g}");
    }

    #[test]
    fn every_variant_differs_only_in_size_and_reframe() {
        let t = Timeline::new(two_clips(), spec());
        let variants = t.render_variants(&AspectRatio::ALL);
        assert_eq!(variants.len(), 3);

        let sizes: Vec<_> = variants.iter().map(|v| (v.width, v.height)).collect();
        assert_eq!(sizes, vec![(1920, 1080), (1080, 1920), (1920, 1920)]);

        let paths: Vec<_> = variants
            .iter()
            .map(|v| v.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            paths,
            vec![
                "/out/final-16x9.mp4",
                "/out/final-9x16.mp4",
                "/out/final-1x1.mp4"
            ]
        );

        // The join, the trims and the output count are identical across
        // variants; only the reframe stage moves.
        for v in &variants {
            let g = graph_of(&v.args);
            assert!(g.contains("[c0][c1]concat=n=2:v=1:a=0[vseq]"), "{g}");
            assert!(g.contains(&format!("crop={}:{}", v.width, v.height)), "{g}");
        }
    }

    #[test]
    fn variant_dimensions_are_always_even() {
        // 1079 would give a 607-pixel short edge, which no H.264 encoder in
        // yuv420p will accept.
        for ratio in AspectRatio::ALL {
            let (w, h) = ratio.dimensions(1079);
            assert_eq!(w % 2, 0, "{ratio:?} width {w}");
            assert_eq!(h % 2, 0, "{ratio:?} height {h}");
        }
    }

    #[test]
    fn a_vertical_timeline_still_yields_a_wide_variant() {
        // The long edge, not the width, drives variant sizing.
        let t = Timeline::new(two_clips(), OutputSpec::new(1080, 1920, 30, "/out/x.mp4"));
        let wide = &t.render_variants(&[AspectRatio::Wide])[0];
        assert_eq!((wide.width, wide.height), (1920, 1080));
    }

    // -- audio -------------------------------------------------------------

    fn narr() -> AudioTrack {
        AudioTrack::new("/a/vo.wav", AudioRole::Narration, 0.0)
    }
    fn music() -> AudioTrack {
        AudioTrack::new("/a/bed.mp3", AudioRole::Music, -6.0)
    }

    #[test]
    fn music_ducks_under_narration_when_both_are_present() {
        let mut t = Timeline::new(two_clips(), spec());
        t.audio = vec![narr(), music()];
        let g = t.filtergraph();

        // Narration is split so one copy can drive the compressor.
        assert!(g.contains("[a0]asplit=2[narr_mix][narr_sc]"), "{g}");
        // Music is the main input, narration the sidechain — in that order.
        assert!(g.contains("[a1][narr_sc]sidechaincompress="), "{g}");
        assert!(g.contains("[ducked]"), "{g}");
        // The voice heard in the mix is the un-compressed copy.
        assert!(g.contains("[narr_mix][ducked]amix=inputs=2"), "{g}");
        // Never normalise: that would halve the narration for having added music.
        assert!(g.contains("normalize=0"), "{g}");
        assert!(g.ends_with("[aout]"), "{g}");

        let args = t.to_args();
        assert!(args.windows(2).any(|w| w == ["-map", "[aout]"]));
        assert!(args.contains(&"-shortest".to_string()));
    }

    #[test]
    fn music_alone_is_never_ducked() {
        // Nothing to duck against. A compressor here would pump against itself.
        let mut t = Timeline::new(two_clips(), spec());
        t.audio = vec![music()];
        let g = t.filtergraph();
        assert!(!g.contains("sidechaincompress"), "{g}");
        assert!(!g.contains("asplit"), "{g}");
        assert!(
            g.contains("[a0]apad,"),
            "single track goes straight to the mix: {g}"
        );
        assert!(g.contains("volume=-6dB"), "{g}");
    }

    #[test]
    fn narration_alone_is_never_ducked() {
        let mut t = Timeline::new(two_clips(), spec());
        t.audio = vec![narr()];
        let g = t.filtergraph();
        assert!(!g.contains("sidechaincompress"), "{g}");
        assert!(g.contains("[a0]apad,"), "{g}");
    }

    #[test]
    fn sfx_are_mixed_flat_alongside_the_ducked_bed() {
        let mut t = Timeline::new(two_clips(), spec());
        t.audio = vec![
            narr(),
            music(),
            AudioTrack::new("/a/whoosh.wav", AudioRole::Sfx, -3.0),
        ];
        let g = t.filtergraph();
        assert!(g.contains("sidechaincompress"), "{g}");
        // Three-way final mix: voice, ducked bed, effects — effects last.
        assert!(g.contains("[narr_mix][ducked][a2]amix=inputs=3"), "{g}");
    }

    #[test]
    fn multiple_tracks_of_one_role_are_pre_mixed() {
        let mut t = Timeline::new(two_clips(), spec());
        t.audio = vec![
            narr(),
            AudioTrack::new("/a/vo2.wav", AudioRole::Narration, -2.0),
            music(),
        ];
        let g = t.filtergraph();
        assert!(g.contains("[a0][a1]amix=inputs=2:normalize=0"), "{g}");
        assert!(g.contains("[narration]asplit=2"), "{g}");
    }

    #[test]
    fn no_audio_means_no_audio_flags_at_all() {
        let args = Timeline::new(two_clips(), spec()).to_args();
        assert!(!args.contains(&"-c:a".to_string()));
        assert!(!args.contains(&"-shortest".to_string()));
        assert!(!args.contains(&"[aout]".to_string()));
    }

    #[test]
    fn every_track_is_conformed_before_it_reaches_the_mix() {
        // sidechaincompress and amix both refuse mismatched inputs, and a mono
        // voiceover next to a stereo bed is the normal case.
        let mut t = Timeline::new(two_clips(), spec());
        t.audio = vec![narr(), music()];
        let g = t.filtergraph();
        assert_eq!(
            g.matches("aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo")
                .count(),
            3,
            "two inputs plus the final stage: {g}"
        );
    }

    // -- captions and escaping --------------------------------------------

    #[test]
    fn drawtext_escapes_every_character_that_breaks_the_parser() {
        // Verified against ffmpeg 8.1 by frame-hash comparison with `textfile=`.
        assert_eq!(escape_filter_value("A:B"), r"A\\:B");
        assert_eq!(escape_filter_value("A'B"), r"A\\\'B");
        assert_eq!(escape_filter_value(r"A\B"), r"A\\\\B");
        assert_eq!(escape_filter_value("A,B"), r"A\,B");
        assert_eq!(escape_filter_value("A;B"), r"A\;B");
        assert_eq!(escape_filter_value("A[B]"), r"A\[B\]");
    }

    #[test]
    fn graph_level_characters_are_escaped_once_and_option_level_twice() {
        // The asymmetry is the whole point: `\\,` is as broken as a bare `:`.
        assert!(!escape_filter_value("A,B").contains(r"\\,"));
        assert!(escape_filter_value("A:B").contains(r"\\:"));
    }

    #[test]
    fn percent_and_equals_pass_through_untouched() {
        // Only meaningful under text expansion, which `drawtext` is told to
        // disable. Escaping them anyway would print a literal backslash.
        assert_eq!(escape_filter_value("Up 50% = good"), "Up 50% = good");
        assert_eq!(escape_filter_value("%{localtime}"), "%{localtime}");
        let cap = Caption::new("Up 50%", 0.0, 1.0);
        let f = drawtext(&cap);
        assert!(f.contains("text=Up 50%:"), "{f}");
        assert!(f.contains("expansion=none"), "{f}");
    }

    #[test]
    fn edge_whitespace_survives_the_tokeniser() {
        // ffmpeg trims unescaped whitespace from both ends of a value, so a
        // deliberately indented caption would silently lose its indent.
        assert_eq!(escape_filter_value("  hi  "), r"\\ \\ hi\\ \\ ");
        assert_eq!(
            escape_filter_value("a b"),
            "a b",
            "interior spaces are fine"
        );
    }

    #[test]
    fn a_realistic_nasty_caption_is_fully_escaped() {
        let raw = r"Cost: 50% off - it's a \ backslash [x], ok;";
        let escaped = escape_filter_value(raw);
        // Nothing that would terminate the option or the filter survives raw.
        for (i, c) in escaped.char_indices() {
            if matches!(c, ':' | ',' | ';' | '[' | ']' | '\'') {
                let before = &escaped[..i];
                let slashes = before.len() - before.trim_end_matches('\\').len();
                assert!(slashes > 0, "unescaped {c:?} at {i} in {escaped}");
            }
        }
        let f = drawtext(&Caption::new(raw, 0.0, 2.0));
        assert!(f.starts_with("drawtext="), "{f}");
    }

    #[test]
    fn a_windows_font_path_does_not_break_the_graph() {
        // `C:\Windows\Fonts\arial.ttf` is a colon and three escapes. Left raw,
        // this parses fine everywhere except Windows.
        let mut cap = Caption::new("hello", 0.0, 1.0);
        cap.style.font_file = Some(PathBuf::from(r"C:\Windows\Fonts\arial.ttf"));
        let f = drawtext(&cap);
        assert!(
            f.contains(r"fontfile=C\\:\\\\Windows\\\\Fonts\\\\arial.ttf:"),
            "{f}"
        );
    }

    #[test]
    fn the_caption_window_quotes_its_commas() {
        let f = drawtext(&Caption::new("hi", 1.5, 4.25));
        assert!(f.contains("enable='between(t,1.5,4.25)'"), "{f}");
    }

    #[test]
    fn captions_chain_after_the_join_in_timeline_order() {
        let mut t = Timeline::new(two_clips(), spec());
        t.captions = vec![Caption::new("one", 0.0, 2.0), Caption::new("two", 2.0, 4.0)];
        let g = t.filtergraph();
        assert!(
            g.contains("[vseq]drawtext="),
            "captions apply to the join: {g}"
        );
        assert!(g.contains("[vcap]format=yuv420p[vout]"), "{g}");
        let first = g.find("text=one").unwrap();
        let second = g.find("text=two").unwrap();
        assert!(first < second, "order must be preserved");
    }

    #[test]
    fn captions_default_to_an_outline_and_can_take_a_plate() {
        let plain = drawtext(&Caption::new("hi", 0.0, 1.0));
        assert!(plain.contains("borderw=3"), "{plain}");
        assert!(!plain.contains("box=1"), "{plain}");

        let mut boxed = Caption::new("hi", 0.0, 1.0);
        boxed.style.box_color = Some("black@0.5".into());
        let f = drawtext(&boxed);
        assert!(f.contains("box=1:boxcolor=black@0.5:boxborderw=16"), "{f}");
    }

    #[test]
    fn caption_position_moves_only_the_y_expression() {
        let mut cap = Caption::new("hi", 0.0, 1.0);
        assert!(drawtext(&cap).contains("y=h-text_h-96"));
        cap.style.position = CaptionPosition::Top;
        assert!(drawtext(&cap).contains("y=96"));
        cap.style.position = CaptionPosition::Middle;
        assert!(drawtext(&cap).contains("y=(h-text_h)/2"));
        // Always horizontally centred.
        assert!(drawtext(&cap).contains("x=(w-text_w)/2"));
    }

    // -- encoding and validation ------------------------------------------

    #[test]
    fn the_encoder_and_its_quality_flags_reach_the_argv() {
        let mut out = spec();
        out.encoder = Encoder::VideoToolbox;
        out.crf_or_bitrate = Quality::Crf(18);
        let args = Timeline::new(two_clips(), out).to_args();
        let joined = args.join(" ");
        assert!(joined.contains("-c:v h264_videotoolbox"), "{joined}");
        assert!(joined.contains("-q:v 64"), "translated from CRF: {joined}");
        assert!(!joined.contains("-crf"), "{joined}");
        assert!(joined.contains("-pix_fmt yuv420p"), "{joined}");
        assert!(joined.contains("-movflags +faststart"), "{joined}");
        assert!(joined.contains("-progress pipe:1"), "the UI needs progress");
        assert!(joined.contains("-nostdin"), "{joined}");
    }

    #[test]
    fn validation_catches_the_mistakes_that_would_render_wrong() {
        let mut out = OutputSpec::new(1921, 1080, 0, "/out/x.mp4");
        out.aspect = AspectMode::Fill;
        let mut t = Timeline::new(
            vec![
                Clip::new("/m/a.mp4", 3.0, 3.0),
                Clip::new("/m/b.mp4", -1.0, 2.0),
            ],
            out,
        );
        t.captions = vec![
            Caption::new("late", 900.0, 901.0),
            Caption::new("", 0.0, 0.0),
        ];
        let p = t.validate().join(" | ");
        assert!(p.contains("no length"), "{p}");
        assert!(p.contains("negative trim_start"), "{p}");
        assert!(p.contains("odd"), "{p}");
        assert!(p.contains("fps is zero"), "{p}");
        assert!(p.contains("past the"), "{p}");
        assert!(p.contains("empty"), "{p}");
    }

    #[test]
    fn a_healthy_timeline_has_nothing_to_report() {
        let mut t = Timeline::new(two_clips(), spec());
        t.audio = vec![narr(), music()];
        t.captions = vec![Caption::new("fine", 0.5, 3.0)];
        assert!(t.validate().is_empty(), "{:?}", t.validate());
    }

    #[test]
    fn seconds_format_readably() {
        assert_eq!(secs(0.0), "0");
        assert_eq!(secs(4.0), "4");
        assert_eq!(secs(0.1 + 0.2), "0.3");
        assert_eq!(secs(1.5), "1.5");
        assert_eq!(secs(-6.0), "-6");
    }

    #[test]
    fn a_timeline_round_trips_through_json() {
        // Recipe export and Rerun both depend on this.
        let mut t = Timeline::new(two_clips(), spec());
        t.audio = vec![narr()];
        t.captions = vec![Caption::new("hi", 0.0, 1.0)];
        let json = serde_json::to_string(&t).unwrap();
        let back: Timeline = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.to_args(), t.to_args());
    }

    // -- real ffmpeg -------------------------------------------------------
    //
    // A filtergraph that reads correctly and that ffmpeg then rejects is the
    // default failure mode of this module, so these run the argv we actually
    // build against a real binary. Ignored by default because they need ffmpeg
    // and ffprobe on PATH (override with HALATION_FFMPEG / HALATION_FFPROBE)
    // and take a couple of seconds.
    //
    //   cargo test -p halation-core --ignored
    mod real {
        use super::*;
        use std::process::Command;

        fn ffmpeg() -> String {
            std::env::var("HALATION_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
        }
        fn ffprobe() -> String {
            std::env::var("HALATION_FFPROBE").unwrap_or_else(|_| "ffprobe".into())
        }

        fn workdir(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("halation-compositor-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            dir
        }

        /// A tiny synthetic clip, so the test needs no fixtures in the repo.
        fn synth(dir: &Path, name: &str, source: &str, seconds: f64) -> PathBuf {
            let path = dir.join(name);
            let status = Command::new(ffmpeg())
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("{source}=size=320x180:rate=25:duration={seconds}"),
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                ])
                .arg(&path)
                .status()
                .expect("ffmpeg must be on PATH for --ignored tests");
            assert!(status.success(), "could not synthesise {name}");
            path
        }

        fn synth_audio(dir: &Path, name: &str, source: &str) -> PathBuf {
            let path = dir.join(name);
            let status = Command::new(ffmpeg())
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    source,
                    "-c:a",
                    "aac",
                ])
                .arg(&path)
                .status()
                .expect("ffmpeg");
            assert!(status.success(), "could not synthesise {name}");
            path
        }

        fn duration_of(path: &Path) -> f64 {
            let out = Command::new(ffprobe())
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration",
                    "-of",
                    "csv=p=0",
                ])
                .arg(path)
                .output()
                .expect("ffprobe must be on PATH for --ignored tests");
            String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
        }

        fn run(args: &[String]) {
            let out = Command::new(ffmpeg()).args(args).output().expect("ffmpeg");
            assert!(
                out.status.success(),
                "ffmpeg rejected the graph.\nargs: {args:?}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        fn has_filter(name: &str) -> bool {
            let out = Command::new(ffmpeg())
                .args(["-hide_banner", "-filters"])
                .output()
                .expect("ffmpeg");
            String::from_utf8_lossy(&out.stdout).contains(&format!(" {name} "))
        }

        #[test]
        #[ignore = "needs ffmpeg on PATH"]
        fn a_real_two_clip_concat_renders_to_the_expected_length() {
            let dir = workdir("concat");
            let a = synth(&dir, "a.mp4", "testsrc", 2.0);
            let b = synth(&dir, "b.mp4", "smptebars", 2.0);
            let out = dir.join("out.mp4");

            let t = Timeline::new(
                vec![
                    Clip::new(&a, 0.0, 2.0),
                    Clip::new(&b, 0.5, 2.0), // 1.5s
                ],
                OutputSpec::new(320, 180, 25, &out),
            );
            assert!(t.validate().is_empty(), "{:?}", t.validate());
            run(&t.to_args());

            assert!(out.is_file(), "no output written");
            let got = duration_of(&out);
            let want = t.total_duration(); // 3.5
            assert!((got - want).abs() < 0.1, "expected ~{want}s, got {got}s");
        }

        #[test]
        #[ignore = "needs ffmpeg on PATH"]
        fn a_real_crossfade_shortens_the_output_by_the_transition() {
            // This is the assertion the offset maths exists for: two 3s clips
            // with a 0.5s crossfade must be 5.5s, not 6s and not 5s.
            let dir = workdir("xfade");
            let a = synth(&dir, "a.mp4", "testsrc", 3.0);
            let b = synth(&dir, "b.mp4", "smptebars", 3.0);
            let out = dir.join("out.mp4");

            let t = Timeline::new(
                vec![
                    Clip::new(&a, 0.0, 3.0),
                    Clip::new(&b, 0.0, 3.0).crossfade_in(0.5),
                ],
                OutputSpec::new(320, 180, 25, &out),
            );
            run(&t.to_args());
            let got = duration_of(&out);
            assert!((got - 5.5).abs() < 0.1, "expected ~5.5s, got {got}s");
        }

        #[test]
        #[ignore = "needs ffmpeg on PATH"]
        fn every_aspect_variant_renders() {
            let dir = workdir("variants");
            let a = synth(&dir, "a.mp4", "testsrc", 1.0);
            let b = synth(&dir, "b.mp4", "smptebars", 1.0);

            for mode in [AspectMode::Fill, AspectMode::Fit] {
                let mut out = OutputSpec::new(640, 360, 25, dir.join(format!("{mode:?}.mp4")));
                out.aspect = mode;
                let t = Timeline::new(vec![Clip::new(&a, 0.0, 1.0), Clip::new(&b, 0.0, 1.0)], out);
                for v in t.render_variants(&AspectRatio::ALL) {
                    run(&v.args);
                    assert!(v.path.is_file(), "{:?} {mode:?} missing", v.ratio);
                    let (w, h) = probe_size(&v.path);
                    assert_eq!((w, h), (v.width, v.height), "{:?}", v.ratio);
                }
            }
        }

        fn probe_size(path: &Path) -> (u32, u32) {
            let out = Command::new(ffprobe())
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=width,height",
                    "-of",
                    "csv=p=0",
                ])
                .arg(path)
                .output()
                .expect("ffprobe");
            let text = String::from_utf8_lossy(&out.stdout);
            let mut it = text.trim().split(',');
            (
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap().parse().unwrap(),
            )
        }

        #[test]
        #[ignore = "needs ffmpeg on PATH"]
        fn a_real_ducked_mix_renders_with_both_streams() {
            let dir = workdir("audio");
            let a = synth(&dir, "a.mp4", "testsrc", 3.0);
            let vo = synth_audio(
                &dir,
                "vo.m4a",
                "sine=frequency=440:duration=2:sample_rate=48000",
            );
            let bed = synth_audio(
                &dir,
                "bed.m4a",
                "anoisesrc=duration=6:color=pink:sample_rate=48000:amplitude=0.4",
            );
            let out = dir.join("mix.mp4");

            let mut t = Timeline::new(
                vec![Clip::new(&a, 0.0, 3.0)],
                OutputSpec::new(320, 180, 25, &out),
            );
            t.audio = vec![
                AudioTrack::new(&vo, AudioRole::Narration, 0.0),
                AudioTrack::new(&bed, AudioRole::Music, -8.0),
            ];
            assert!(t.filtergraph().contains("sidechaincompress"));
            run(&t.to_args());

            let out_streams = Command::new(ffprobe())
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=codec_type",
                    "-of",
                    "csv=p=0",
                ])
                .arg(&out)
                .output()
                .expect("ffprobe");
            let streams = String::from_utf8_lossy(&out_streams.stdout);
            assert!(streams.contains("video"), "{streams}");
            assert!(streams.contains("audio"), "{streams}");
            // apad + -shortest: the 6s bed must not extend the 3s video.
            let got = duration_of(&out);
            assert!((got - 3.0).abs() < 0.2, "expected ~3s, got {got}s");
        }

        #[test]
        #[ignore = "needs ffmpeg with libfreetype on PATH"]
        fn a_caption_full_of_metacharacters_renders() {
            // The escaping table in `escape_filter_value` was derived from this
            // exact class of input. If it regresses, ffmpeg fails to parse.
            if !has_filter("drawtext") {
                eprintln!("skipping: this ffmpeg has no drawtext (needs --enable-libfreetype)");
                return;
            }
            let dir = workdir("captions");
            let a = synth(&dir, "a.mp4", "testsrc", 2.0);
            let out = dir.join("cap.mp4");

            let mut t = Timeline::new(
                vec![Clip::new(&a, 0.0, 2.0)],
                OutputSpec::new(640, 360, 25, &out),
            );
            t.captions = vec![
                Caption::new(r"Cost: 50% off - it's a \ backslash [x], ok;", 0.0, 1.0),
                Caption::new("%{localtime} must stay literal", 1.0, 2.0),
            ];
            run(&t.to_args());
            assert!(out.is_file());
        }
    }
}
