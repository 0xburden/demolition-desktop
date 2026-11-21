use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamInstant, SupportedStreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::time::{Duration, Instant};

pub const CHANNELS: usize = 4;
const NO_TIME: u64 = u64::MAX;
const STREAM_TIMEOUT: Duration = Duration::from_secs(5);
type Result<T> = std::result::Result<T, String>;

struct Signal {
    origin: Instant,
    recording: AtomicBool,
    input_time: AtomicU64,
    guide_time: AtomicU64,
    last_input: AtomicU64,
    last_output: AtomicU64,
    peaks: [AtomicU32; CHANNELS],
    input_fault: AtomicU32,
    output_fault: AtomicU32,
}

impl Signal {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            recording: false.into(),
            input_time: NO_TIME.into(),
            guide_time: NO_TIME.into(),
            last_input: NO_TIME.into(),
            last_output: NO_TIME.into(),
            peaks: Default::default(),
            input_fault: 0.into(),
            output_fault: 0.into(),
        }
    }

    // CPAL timestamps use a stream-specific clock. Translate their per-buffer latency
    // into our shared monotonic clock, without assuming input/output share an epoch.
    fn time(&self, callback: StreamInstant, frame: StreamInstant) -> u64 {
        let now = self.origin.elapsed().as_nanos() as i128;
        (now + (frame.as_nanos() as i128 - callback.as_nanos() as i128))
            .clamp(0, i128::from(NO_TIME - 1)) as u64
    }
}

fn set_fault(slot: &AtomicU32, code: u32) {
    let _ = slot.compare_exchange(0, code, Release, Relaxed);
}

fn report_cpal_fault(slot: &AtomicU32, error: cpal::Error) {
    match error.kind() {
        cpal::ErrorKind::RealtimeDenied => {}
        cpal::ErrorKind::Xrun => set_fault(slot, 2),
        cpal::ErrorKind::PermissionDenied => set_fault(slot, 4),
        _ => set_fault(slot, 1),
    }
}

pub struct Capture {
    stream: Option<cpal::Stream>,
    signal: Arc<Signal>,
    consumer: Consumer<[f32; CHANNELS]>,
    rate: u32,
}

#[derive(Clone, Copy, Debug)]
enum GuideFrame {
    Sample([f32; 2]),
    End,
}

pub struct Playback {
    _stream: cpal::Stream,
    producer: Producer<GuideFrame>,
}

pub const DEFAULT_BPM: u16 = 120;
pub const TEMPO_RANGE: std::ops::RangeInclusive<u16> = 30..=300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meter {
    pub beats: u8,
    pub unit: u8,
}

impl Default for Meter {
    fn default() -> Self {
        Self { beats: 4, unit: 4 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimingChange {
    /// One-based bar number. Unset fields carry over from the previous bar.
    pub bar: u32,
    pub bpm: Option<u16>,
    pub meter: Option<Meter>,
}

#[derive(Clone, Debug)]
pub struct ClickSettings {
    /// Quarter notes per minute, regardless of the meter's denominator.
    pub bpm: u16,
    pub meter: Meter,
    pub changes: Vec<TimingChange>,
    pub accent: bool,
    /// Click level as a percentage.
    pub level: u8,
}

impl Default for ClickSettings {
    fn default() -> Self {
        Self {
            bpm: DEFAULT_BPM,
            meter: Meter::default(),
            changes: Vec::new(),
            accent: true,
            level: 70,
        }
    }
}

impl ClickSettings {
    pub fn validate(&self) -> Result<()> {
        fn validate_bpm(bpm: u16) -> Result<()> {
            if !TEMPO_RANGE.contains(&bpm) {
                return Err("Set the tempo between 30 and 300 BPM.".into());
            }
            Ok(())
        }

        fn validate_meter(meter: Meter) -> Result<()> {
            if !(1..=32).contains(&meter.beats) || !matches!(meter.unit, 2 | 4 | 8 | 16) {
                return Err("Set the meter to 1–32 beats with a unit of 2, 4, 8, or 16.".into());
            }
            Ok(())
        }

        validate_bpm(self.bpm)?;
        validate_meter(self.meter)?;
        if self.level > 100 {
            return Err("Set the click level between 0 and 100%.".into());
        }
        let mut previous_bar = 1;
        for change in &self.changes {
            if change.bar <= previous_bar {
                return Err(
                    "Timing changes must be in increasing bar order, starting at bar 2 or later."
                        .into(),
                );
            }
            if change.bpm.is_none() && change.meter.is_none() {
                return Err("Each timing change must set a tempo or meter.".into());
            }
            if let Some(bpm) = change.bpm {
                validate_bpm(bpm)?;
            }
            if let Some(meter) = change.meter {
                validate_meter(meter)?;
            }
            previous_bar = change.bar;
        }
        Ok(())
    }
}

pub struct ClickPreview {
    _stream: cpal::Stream,
    signal: Arc<Signal>,
}

macro_rules! sample_type {
    ($format:expr, $build:ident($($params:expr),+)) => {
        match $format {
            SampleFormat::F32 => $build::<f32>($($params),+),
            SampleFormat::F64 => $build::<f64>($($params),+),
            SampleFormat::I8 => $build::<i8>($($params),+),
            SampleFormat::I16 => $build::<i16>($($params),+),
            SampleFormat::I24 => $build::<cpal::I24>($($params),+),
            SampleFormat::I32 => $build::<i32>($($params),+),
            SampleFormat::I64 => $build::<i64>($($params),+),
            SampleFormat::U8 => $build::<u8>($($params),+),
            SampleFormat::U16 => $build::<u16>($($params),+),
            SampleFormat::U24 => $build::<cpal::U24>($($params),+),
            SampleFormat::U32 => $build::<u32>($($params),+),
            SampleFormat::U64 => $build::<u64>($($params),+),
            _ => Err("This device uses an unsupported sample format.".into()),
        }
    };
}

impl ClickPreview {
    /// Start the preview on the audio worker, not the UI thread.
    pub fn start(id: &str, rate: u32, settings: ClickSettings) -> Result<Self> {
        settings.validate()?;
        let device = lookup(id)?;
        // Preview has no capture clock or guide to match, so it can use the
        // selected device's native rate when it can't run at the project rate.
        let cfg = config(&device, rate, false).or_else(|error| {
            let native = device.default_output_config().map_err(|_| error)?;
            config(&device, native.sample_rate(), false)
        })?;
        let signal = Arc::new(Signal::new());
        let click = Click::new(cfg.sample_rate(), settings);
        let stream = sample_type!(
            cfg.sample_format(),
            open_output(
                device,
                cfg.config(),
                None,
                Some(click),
                signal.clone(),
                false
            )
        )?;
        stream
            .play()
            .map_err(|err| format!("Couldn't start click: {err}"))?;
        Ok(Self {
            _stream: stream,
            signal,
        })
    }

    pub fn issue(&self) -> Option<&'static str> {
        output_issue(&self.signal, true)
    }
}

// Common precision for the fractional sample carried across tempo/meter changes.
const PHASE_SCALE: u128 = 1 << 48;
const CLICK_PEAK_GAIN: f32 = 0.45;

struct Click {
    settings: ClickSettings,
    phase: u128,
    interval: u128,
    step: u128,
    bpm: u16,
    meter: Meter,
    bar: u64,
    beat: u8,
    next_change: usize,
    wave_index: usize,
    waves: [Vec<f32>; 2],
}

impl Click {
    // Called on the worker with validated settings, before opening the stream.
    fn new(rate: u32, settings: ClickSettings) -> Self {
        // Build the short percussive waveforms off the audio thread.
        let len = rate as usize * 18 / 1000;
        let wave = |hz: f32, gain| {
            (0..len)
                .map(|i| {
                    let time = i as f32 / rate as f32;
                    let attack = (i as f32 / (rate as f32 * 0.001)).min(1.0);
                    let decay = (1.0 - i as f32 / len as f32).powi(3);
                    gain * attack * decay * (std::f32::consts::TAU * hz * time).sin()
                })
                .collect()
        };
        let bpm = settings.bpm;
        let meter = settings.meter;
        let level = f32::from(settings.level) / 100.0;
        Self {
            settings,
            phase: 0,
            interval: u128::from(rate) * 60 * 4 * PHASE_SCALE,
            step: Self::step(bpm, meter),
            bpm,
            meter,
            bar: 1,
            beat: 0,
            next_change: 0,
            wave_index: 0,
            waves: [
                wave(1600.0, CLICK_PEAK_GAIN * level),
                wave(1050.0, 0.25 * level),
            ],
        }
    }

    fn step(bpm: u16, meter: Meter) -> u128 {
        // A denominator-note pulse lasts rate * 60 * 4 / (bpm * unit) frames.
        u128::from(bpm) * u128::from(meter.unit) * PHASE_SCALE
    }

    fn next(&mut self) -> f32 {
        let accented = self.settings.accent && self.beat == 0;
        let wave = &self.waves[usize::from(!accented)];
        let sample = wave.get(self.wave_index).copied().unwrap_or(0.0);
        self.wave_index += usize::from(self.wave_index < wave.len());
        self.phase += self.step;
        if self.phase >= self.interval {
            self.phase -= self.interval;
            self.wave_index = 0;
            self.beat += 1;
            if self.beat == self.meter.beats {
                self.beat = 0;
                self.bar += 1;
                self.apply_change();
            }
        }
        sample
    }

    fn apply_change(&mut self) {
        if let Some(change) = self.settings.changes.get(self.next_change)
            && u64::from(change.bar) == self.bar
        {
            self.bpm = change.bpm.unwrap_or(self.bpm);
            self.meter = change.meter.unwrap_or(self.meter);
            let step = Self::step(self.bpm, self.meter);
            // Preserve the overshoot in sample time, not in old-tempo phase units.
            // Ceil only at changes so quantization can't delay an exact boundary.
            self.phase = (self.phase * step).div_ceil(self.step);
            self.step = step;
            self.next_change += 1;
        }
    }
}

fn output_issue(signal: &Signal, check_stall: bool) -> Option<&'static str> {
    match signal.output_fault.load(Acquire) {
        2 => return Some("The audio driver reported an output dropout."),
        3 => return Some("Guide playback couldn't keep up. The take was stopped."),
        n if n != 0 => return Some("The output stream stopped. Check the audio interface."),
        _ => {}
    }
    let last = signal.last_output.load(Relaxed);
    let age = signal
        .origin
        .elapsed()
        .saturating_sub(Duration::from_nanos(if last == NO_TIME { 0 } else { last }));
    (check_stall && age > STREAM_TIMEOUT)
        .then_some("No audio callbacks from the output. Check the audio interface.")
}

fn lookup(id: &str) -> Result<cpal::Device> {
    let id = id
        .parse::<cpal::DeviceId>()
        .map_err(|err| err.to_string())?;
    cpal::default_host()
        .device_by_id(&id)
        .ok_or_else(|| "Audio device is no longer available. Refresh the device list.".into())
}

fn config(device: &cpal::Device, rate: u32, input: bool) -> Result<SupportedStreamConfig> {
    let format_score = |format| match format {
        SampleFormat::F32 => 0,
        SampleFormat::I32 | SampleFormat::I24 => 1,
        SampleFormat::I16 => 2,
        SampleFormat::F64
        | SampleFormat::I8
        | SampleFormat::I64
        | SampleFormat::U8
        | SampleFormat::U16
        | SampleFormat::U24
        | SampleFormat::U32
        | SampleFormat::U64 => 3,
        _ => 4,
    };
    // CPAL uses different iterator types for input and output.
    let choose = |ranges: Vec<cpal::SupportedStreamConfigRange>| {
        ranges
            .into_iter()
            .filter(|range| {
                range.channels() >= if input { CHANNELS as u16 } else { 1 }
                    && format_score(range.sample_format()) < 4
            })
            .filter_map(|range| range.try_with_sample_rate(rate))
            .min_by_key(|conf| {
                (
                    if input {
                        conf.channels() - CHANNELS as u16
                    } else {
                        conf.channels().abs_diff(2)
                    },
                    format_score(conf.sample_format()),
                )
            })
    };
    let chosen = if input {
        choose(
            device
                .supported_input_configs()
                .map_err(|err| err.to_string())?
                .collect(),
        )
    } else {
        choose(
            device
                .supported_output_configs()
                .map_err(|err| err.to_string())?
                .collect(),
        )
    };
    chosen.ok_or_else(|| if input {
        format!("This input doesn't expose four channels at {rate} Hz. Choose another input or a guide WAV at a supported rate.")
    } else {
        format!("This output doesn't support {rate} Hz. Choose another output or a guide WAV at a supported rate.")
    })
}

impl Capture {
    pub fn open(id: &str, rate: u32) -> Result<Self> {
        let device = lookup(id)?;
        let cfg = config(&device, rate, true)?;
        let (producer, consumer) = RingBuffer::new(rate as usize);
        let signal = Arc::new(Signal::new());
        let stream = sample_type!(
            cfg.sample_format(),
            open_input(device, cfg.config(), producer, signal.clone())
        )?;
        stream
            .play()
            .map_err(|err| format!("Couldn't start input: {err}"))?;
        Ok(Self {
            stream: Some(stream),
            signal,
            consumer,
            rate,
        })
    }

    /// Prepare output on the audio worker, not the UI thread.
    pub fn prepare_playback(
        &self,
        id: &str,
        has_guide: bool,
        click: Option<ClickSettings>,
    ) -> Result<Playback> {
        if let Some(settings) = &click {
            settings.validate()?;
        }
        let device = lookup(id)?;
        let cfg = config(&device, self.rate, false)?;
        let click = click.map(|settings| Click::new(self.rate, settings));
        self.signal.output_fault.store(0, Relaxed);
        self.signal.last_output.store(NO_TIME, Relaxed);
        let (producer, consumer) = RingBuffer::new(self.rate as usize);
        let stream = sample_type!(
            cfg.sample_format(),
            open_output(
                device,
                cfg.config(),
                has_guide.then_some(consumer),
                click,
                self.signal.clone(),
                true
            )
        )?;
        stream
            .play()
            .map_err(|err| format!("Couldn't start output: {err}"))?;
        Ok(Playback {
            _stream: stream,
            producer,
        })
    }

    pub fn begin(&self, has_playback: bool) {
        if has_playback {
            let _ = self.signal.last_output.compare_exchange(
                NO_TIME,
                self.signal.origin.elapsed().as_nanos() as u64,
                Relaxed,
                Relaxed,
            );
        }
        self.signal.recording.store(true, Release);
    }

    pub fn stop(&mut self) {
        self.signal.recording.store(false, Release);
        // Stop and release the producer before the writer drains the remaining frames.
        drop(self.stream.take());
    }

    pub fn next_frame(&mut self) -> Option<[f32; CHANNELS]> {
        self.consumer.pop().ok()
    }

    pub fn peaks(&self) -> [f32; CHANNELS] {
        self.signal
            .peaks
            .each_ref()
            .map(|peak| f32::from_bits(peak.swap(0, Relaxed)))
    }

    pub fn input_ready(&self) -> bool {
        self.signal.last_input.load(Relaxed) != NO_TIME
    }

    pub fn guide_start_frame(&self) -> Option<i64> {
        let start = self.signal.input_time.load(Acquire);
        let guide = self.signal.guide_time.load(Acquire);
        if start == NO_TIME || guide == NO_TIME {
            return None;
        }
        let scaled_ns = (guide as i128 - start as i128) * i128::from(self.rate);
        // Round to the nearest project sample frame, including negative offsets.
        Some(((scaled_ns + scaled_ns.signum() * 500_000_000) / 1_000_000_000) as i64)
    }

    pub fn issue(&self) -> Option<&'static str> {
        match self.signal.input_fault.load(Relaxed) {
            2 => return Some("The audio driver reported an input dropout."),
            3 => return Some("Recording couldn't keep up with the input. The take was stopped."),
            4 => return Some("Microphone access was denied. Check your system privacy settings."),
            n if n != 0 => {
                return Some("The input stream stopped. Check the audio interface and refresh.");
            }
            _ => {}
        }
        let check_output =
            self.signal.recording.load(Relaxed) && self.signal.last_output.load(Relaxed) != NO_TIME;
        if let Some(issue) = output_issue(&self.signal, check_output) {
            return Some(issue);
        }
        let last = self.signal.last_input.load(Relaxed);
        let elapsed = self
            .signal
            .origin
            .elapsed()
            .saturating_sub(Duration::from_nanos(if last == NO_TIME { 0 } else { last }));
        (elapsed > STREAM_TIMEOUT)
            .then_some("No audio is arriving from the input. Check the connection and permissions.")
    }
}

impl Playback {
    pub fn slots(&self) -> usize {
        self.producer.slots()
    }

    pub fn push(&mut self, frame: [f32; 2]) {
        debug_assert!(self.producer.slots() > 0);
        let _ = self.producer.push(GuideFrame::Sample(frame));
    }

    pub fn end(&mut self) {
        debug_assert!(self.producer.slots() > 0);
        let _ = self.producer.push(GuideFrame::End);
    }
}

fn open_input<T: SizedSample>(
    device: cpal::Device,
    cfg: cpal::StreamConfig,
    mut producer: Producer<[f32; CHANNELS]>,
    signal: Arc<Signal>,
) -> Result<cpal::Stream>
where
    f32: FromSample<T>,
{
    let stride = usize::from(cfg.channels);
    let errors = signal.clone();
    device
        .build_input_stream(
            cfg,
            move |samples: &[T], info| {
                if samples.len() < stride || signal.input_fault.load(Acquire) != 0 {
                    return;
                }
                signal
                    .last_input
                    .store(signal.origin.elapsed().as_nanos() as u64, Relaxed);
                let recording = signal.recording.load(Acquire);
                let mut peaks = [0.0_f32; CHANNELS];
                if recording && signal.input_time.load(Relaxed) == NO_TIME {
                    let stamp = info.timestamp();
                    signal
                        .input_time
                        .store(signal.time(stamp.callback, stamp.capture), Release);
                }
                for input in samples.chunks_exact(stride) {
                    let frame = std::array::from_fn(|i| {
                        let v = f32::from_sample(input[i]);
                        let v = if v.is_finite() { v } else { 0.0 };
                        peaks[i] = peaks[i].max(v.abs());
                        v
                    });
                    if recording && producer.push(frame).is_err() {
                        set_fault(&signal.input_fault, 3);
                        signal.recording.store(false, Release);
                        break;
                    }
                }
                for (peak, value) in signal.peaks.iter().zip(peaks) {
                    peak.fetch_max(value.to_bits(), Relaxed);
                }
            },
            // Some drivers report a discontinuity immediately before handing us the
            // post-gap packet. The data callback rejects packets after this latch.
            move |error| report_cpal_fault(&errors.input_fault, error),
            None,
        )
        .map_err(|err| format!("Couldn't open input: {err}"))
}

fn open_output<T: SizedSample + FromSample<f32>>(
    device: cpal::Device,
    cfg: cpal::StreamConfig,
    mut consumer: Option<Consumer<GuideFrame>>,
    mut click: Option<Click>,
    signal: Arc<Signal>,
    sync_to_capture: bool,
) -> Result<cpal::Stream> {
    let stride = usize::from(cfg.channels);
    let errors = signal.clone();
    let mut guide_ended = consumer.is_none();
    // Reserve only the headroom the configured click needs; 0% leaves the guide alone.
    let guide_gain = click.as_ref().map_or(1.0, |click| {
        1.0 - CLICK_PEAK_GAIN * (f32::from(click.settings.level) / 100.0)
    });
    device
        .build_output_stream(
            cfg,
            move |out: &mut [T], info| {
                signal
                    .last_output
                    .store(signal.origin.elapsed().as_nanos() as u64, Relaxed);
                out.fill(T::EQUILIBRIUM);
                if sync_to_capture
                    && (!signal.recording.load(Acquire)
                        || signal.input_time.load(Acquire) == NO_TIME)
                {
                    return;
                }
                for frame in out.chunks_exact_mut(stride) {
                    let mut guide = [0.0; 2];
                    if !guide_ended && let Some(queue) = &mut consumer {
                        match queue.pop() {
                            Ok(GuideFrame::Sample(samples)) => {
                                guide = samples;
                                if signal.guide_time.load(Relaxed) == NO_TIME {
                                    let stamp = info.timestamp();
                                    signal.guide_time.store(
                                        signal.time(stamp.callback, stamp.playback),
                                        Release,
                                    );
                                }
                            }
                            Ok(GuideFrame::End) => guide_ended = true,
                            Err(_) => {
                                set_fault(&signal.output_fault, 3);
                                break;
                            }
                        }
                    }
                    if let Some(click) = &mut click {
                        let tick = click.next();
                        guide = guide.map(|sample| (sample * guide_gain + tick).clamp(-1.0, 1.0));
                    } else if guide_ended {
                        break;
                    }
                    if stride == 1 {
                        frame[0] = T::from_sample((guide[0] + guide[1]) * 0.5);
                    } else {
                        frame[0] = T::from_sample(guide[0]);
                        frame[1] = T::from_sample(guide[1]);
                    }
                }
            },
            move |error| report_cpal_fault(&errors.output_fault, error),
            None,
        )
        .map_err(|err| format!("Couldn't open audio output: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_accents_bar_and_preserves_fractional_beat_timing() {
        let rate = 44_100;
        let bpm = 137;
        let mut click = Click::new(
            rate,
            ClickSettings {
                bpm,
                ..ClickSettings::default()
            },
        );
        let mut peaks = Vec::new();
        for beat in 0..12_u32 {
            let start = (beat * rate * 60).div_ceil(u32::from(bpm));
            let end = ((beat + 1) * rate * 60).div_ceil(u32::from(bpm));
            let mut peak = 0.0_f32;
            for frame in start..end {
                let sample = click.next();
                if frame == start + 1 {
                    assert_ne!(sample, 0.0, "beat {beat} missed its sample boundary");
                }
                if frame >= start + rate * 18 / 1000 {
                    assert_eq!(sample, 0.0);
                }
                peak = peak.max(sample.abs());
            }
            peaks.push(peak);
        }
        assert!(peaks[0] > peaks[1] && peaks[0] == peaks[4] && peaks[1] == peaks[2]);
    }

    #[test]
    fn click_changes_preserve_fractional_sample_time_and_bar_accents() {
        let rate = 44_100;
        let settings = ClickSettings {
            bpm: 137,
            meter: Meter { beats: 6, unit: 8 },
            changes: vec![
                TimingChange {
                    bar: 2,
                    bpm: Some(173),
                    meter: None,
                },
                TimingChange {
                    bar: 4,
                    bpm: None,
                    meter: Some(Meter { beats: 3, unit: 4 }),
                },
                TimingChange {
                    bar: 5,
                    bpm: Some(137),
                    meter: Some(Meter { beats: 5, unit: 16 }),
                },
            ],
            ..ClickSettings::default()
        };
        assert!(settings.validate().is_ok());
        let mut click = Click::new(rate, settings);
        let onsets = [click.waves[0][1], click.waves[1][1]];
        assert_ne!(onsets[0], onsets[1]);

        // Reference the cumulative exact sample time with a common denominator.
        let denominator = 137 * 173 * 16;
        let mut time = 0_u64;
        let mut frame = 0;
        let bars = [
            (137, 6, 8),
            (173, 6, 8),
            (173, 6, 8),
            (173, 3, 4),
            (137, 5, 16),
        ];
        for (bpm, beats, unit) in bars {
            let pulse = u64::from(rate) * 60 * 4 * (denominator / (bpm * unit));
            for beat in 0..beats {
                let start = time.div_ceil(denominator);
                time += pulse;
                let end = time.div_ceil(denominator);
                assert_eq!(frame, start);
                while frame < end {
                    let sample = click.next();
                    if frame == start + 1 {
                        assert_eq!(sample, onsets[usize::from(beat != 0)], "frame {frame}");
                    } else if frame >= start + u64::from(rate) * 18 / 1000 {
                        assert_eq!(sample, 0.0, "frame {frame}");
                    }
                    frame += 1;
                }
            }
        }
    }

    #[test]
    fn click_can_disable_accent_and_scale_level() {
        let first_onsets = |accent, level| {
            let rate = 8_000;
            let mut click = Click::new(
                rate,
                ClickSettings {
                    accent,
                    level,
                    ..ClickSettings::default()
                },
            );
            click.next();
            let first = click.next();
            for _ in 2..=rate / 2 {
                click.next();
            }
            [first, click.next()]
        };
        let [downbeat, beat] = first_onsets(true, 100);
        assert_ne!(downbeat, beat);
        assert_eq!(first_onsets(false, 100), [beat, beat]);
        assert_eq!(first_onsets(true, 50), [downbeat * 0.5, beat * 0.5]);
        assert_eq!(first_onsets(true, 0), [0.0, 0.0]);
    }

    #[test]
    fn click_settings_validate_base_values_and_changes() {
        let defaults = ClickSettings::default();
        assert_eq!(defaults.bpm, 120);
        assert_eq!(defaults.meter, Meter { beats: 4, unit: 4 });
        assert!(defaults.changes.is_empty() && defaults.accent);
        assert_eq!(defaults.level, 70);
        assert!(defaults.validate().is_ok());

        let change = TimingChange {
            bar: 2,
            bpm: Some(300),
            meter: Some(Meter {
                beats: 32,
                unit: 16,
            }),
        };
        for bpm in [29, 301] {
            assert!(
                ClickSettings {
                    bpm,
                    ..defaults.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                ClickSettings {
                    changes: vec![TimingChange {
                        bpm: Some(bpm),
                        ..change
                    }],
                    ..defaults.clone()
                }
                .validate()
                .is_err()
            );
        }
        for meter in [
            Meter { beats: 0, unit: 4 },
            Meter { beats: 33, unit: 4 },
            Meter { beats: 4, unit: 3 },
        ] {
            assert!(
                ClickSettings {
                    meter,
                    ..defaults.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                ClickSettings {
                    changes: vec![TimingChange {
                        meter: Some(meter),
                        ..change
                    }],
                    ..defaults.clone()
                }
                .validate()
                .is_err()
            );
        }
        for changes in [
            vec![TimingChange { bar: 0, ..change }],
            vec![TimingChange { bar: 1, ..change }],
            vec![change, change],
            vec![TimingChange { bar: 3, ..change }, change],
            vec![TimingChange {
                bpm: None,
                meter: None,
                ..change
            }],
        ] {
            assert!(
                ClickSettings {
                    changes,
                    ..defaults.clone()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            ClickSettings {
                level: 101,
                ..defaults.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            ClickSettings {
                bpm: 30,
                meter: Meter { beats: 1, unit: 2 },
                changes: vec![change],
                level: 100,
                ..defaults
            }
            .validate()
            .is_ok()
        );
    }
}
