//! A worker owns driver streams, WAV writers and SQLite connections. The UI only
//! sends commands and reads snapshots; the callbacks never touch this worker's locks.

use demolition_audio_io::{
    CHANNELS, Capture, ClickPreview, ClickSettings, DeviceInventory, Playback, discover_devices,
};
use demolition_project::{Guide, Project, TakeTiming, TakeTimingChange, TakeWriter};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub enum Command {
    NewProject {
        name: String,
        parent: PathBuf,
    },
    Scan,
    InspectGuide(PathBuf),
    ClearGuide,
    Meter {
        input: String,
        rate: u32,
    },
    DisableMeters,
    PreviewClick {
        output: String,
        timing: ClickSettings,
    },
    StopClick,
    Start {
        output: Option<String>,
        timing: ClickSettings,
        click: bool,
    },
    Stop,
}

#[derive(Clone, Default)]
pub struct Status {
    pub completed: u64,
    pub project: Option<Arc<Project>>,
    pub inventory: Option<Arc<DeviceInventory>>,
    pub guide: Option<(PathBuf, u32)>,
    pub last_take: Option<i64>,
    pub saved_takes: u64,
    pub previewing_click: bool,
    pub meters: Option<[f32; CHANNELS]>,
    pub input_ready: bool,
    pub clipped: [bool; CHANNELS],
    pub recording_seconds: Option<u64>,
    pub notice: Option<String>,
    pub error: Option<String>,
}

impl Status {
    pub fn sample_rate(&self) -> u32 {
        self.guide.as_ref().map_or(48_000, |(_, rate)| *rate)
    }
}

pub struct Worker {
    commands: SyncSender<(u64, Command)>,
    status: Arc<Mutex<Status>>,
    thread: std::thread::JoinHandle<()>,
}

impl Worker {
    pub fn spawn() -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::sync_channel(1);
        let status = Arc::new(Mutex::new(Status::default()));
        let writer = status.clone();
        let thread = std::thread::Builder::new()
            .name("recording-worker".into())
            .spawn(move || run(receiver, writer))?;
        Ok(Self {
            commands,
            status,
            thread,
        })
    }

    pub fn send(&self, seq: u64, command: Command) -> Result<(), String> {
        self.commands.try_send((seq, command)).map_err(|_| {
            "The recording worker isn't ready. Try again or relaunch Demolition.".into()
        })
    }

    pub fn shutdown(self) {
        drop(self.commands);
        let _ = self.thread.join();
    }

    pub fn snapshot(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

struct Recording {
    take: TakeWriter,
    playback: Option<Playback>,
    has_guide: bool,
    guide: Option<Guide>,
    checkpoint: Instant,
    rate: u32,
}

impl Recording {
    fn fill_output(&mut self) -> Result<(), String> {
        let (Some(guide), Some(playback)) = (&mut self.guide, &mut self.playback) else {
            return Ok(());
        };
        let slots = playback.slots();
        if slots < 2 {
            return Ok(());
        }
        // Reserve room for the end marker; the callback never guesses whether a short read is EOF.
        let read = guide
            .read_frames(slots - 1, |frame| playback.push(frame))
            .map_err(|err| format!("Couldn't read the guide: {err}"))?;
        if read < slots - 1 {
            playback.end();
            self.guide = None;
        }
        Ok(())
    }

    fn drain_input(
        take: &mut TakeWriter,
        capture: &mut Capture,
        limit: usize,
    ) -> Result<(), String> {
        for _ in 0..limit {
            let Some(frame) = capture.next_frame() else {
                break;
            };
            take.write_frame(frame)
                .map_err(|err| format!("Couldn't write recorded audio: {err}"))?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct Session {
    status: Status,
    capture: Option<Capture>,
    capture_rate: Option<u32>,
    recording: Option<Recording>,
    click_preview: Option<ClickPreview>,
}

impl Session {
    fn command(&mut self, cmd: Command) -> Result<(), String> {
        self.status.error = None;
        self.status.notice = None;
        if self.recording.is_some() && !matches!(cmd, Command::Stop) {
            return Err("Stop recording before changing the audio setup.".into());
        }
        match cmd {
            Command::NewProject { name, parent } => {
                let project = Project::create(&parent, &name)
                    .map_err(|err| format!("Couldn't create project: {err}"))?;
                self.disable_meters();
                self.stop_click();
                self.status.project = Some(Arc::new(project));
                self.status.guide = None;
                self.status.last_take = None;
                self.status.saved_takes = 0;
            }
            Command::Scan => {
                self.disable_meters();
                self.status.inventory = None;
                self.status.inventory =
                    Some(Arc::new(discover_devices().map_err(|err| {
                        format!("Couldn't look up audio devices: {err}")
                    })?));
            }
            Command::InspectGuide(path) => {
                // A failed replacement leaves the previously selected guide intact.
                let guide =
                    Guide::open(&path).map_err(|err| format!("Couldn't open guide WAV: {err}"))?;
                self.disable_meters();
                self.status.guide = Some((path, guide.sample_rate()));
            }
            Command::ClearGuide => {
                self.disable_meters();
                self.status.guide = None;
            }
            Command::Meter { input, rate } => {
                self.disable_meters();
                self.capture = Some(Capture::open(&input, rate)?);
                self.capture_rate = Some(rate);
                self.status.meters = Some([0.0; CHANNELS]);
            }
            Command::DisableMeters => self.disable_meters(),
            Command::PreviewClick { output, timing } => {
                self.stop_click();
                self.click_preview = Some(ClickPreview::start(
                    &output,
                    self.status.sample_rate(),
                    timing,
                )?);
                self.status.previewing_click = true;
            }
            Command::StopClick => self.stop_click(),
            Command::Start {
                output,
                timing,
                click,
            } => self.start(output, timing, click)?,
            Command::Stop => self.finish(None, true),
        }
        Ok(())
    }

    fn disable_meters(&mut self) {
        self.capture = None;
        self.capture_rate = None;
        self.status.meters = None;
        self.status.input_ready = false;
        self.status.clipped = [false; CHANNELS];
    }

    fn stop_click(&mut self) {
        self.click_preview = None;
        self.status.previewing_click = false;
    }

    fn start(
        &mut self,
        output: Option<String>,
        timing: ClickSettings,
        click: bool,
    ) -> Result<(), String> {
        self.stop_click();
        timing.validate()?;
        let project = self
            .status
            .project
            .as_ref()
            .ok_or("Create a project first.")?;
        let rate = self.status.sample_rate();
        let guide = self.status.guide.as_ref().map(|(path, _)| path.as_path());
        let capture = self.capture.as_ref().ok_or("Enable input meters first.")?;
        if !capture.input_ready() || self.capture_rate != Some(rate) {
            return Err("Wait for the input meters before recording.".into());
        }
        if let Some(error) = capture.issue() {
            return Err(error.into());
        }
        let take_timing = TakeTiming {
            bpm: timing.bpm,
            meter: (timing.meter.beats, timing.meter.unit),
            accent: timing.accent,
            level: timing.level,
            changes: timing
                .changes
                .iter()
                .map(|change| TakeTimingChange {
                    bar: change.bar,
                    bpm: change.bpm,
                    meter: change.meter.map(|meter| (meter.beats, meter.unit)),
                })
                .collect(),
        };
        let has_guide = guide.is_some();
        let playback = if has_guide || click {
            let output = output
                .as_deref()
                .ok_or("Choose an audio output for the guide or click.")?;
            Some(capture.prepare_playback(output, has_guide, click.then_some(timing))?)
        } else {
            None
        };
        let take = TakeWriter::start(project, guide, rate, &take_timing, click)
            .map_err(|err| format!("Couldn't create take: {err}"))?;
        self.status.last_take = Some(take.number());
        let mut recording = Recording {
            guide: None,
            take,
            playback,
            has_guide,
            checkpoint: Instant::now(),
            rate,
        };
        let ready = (|| {
            if let Some(path) = recording.take.guide_path() {
                recording.guide = Some(
                    Guide::open(path)
                        .map_err(|err| format!("Couldn't reopen the project guide WAV: {err}"))?,
                );
                recording.fill_output()?;
            }
            capture.issue().map_or(Ok(()), |issue| Err(issue.into()))
        })();
        if let Err(issue) = ready {
            self.recording = Some(recording);
            self.finish(Some(issue), false);
            return Ok(());
        }
        capture.begin(recording.playback.is_some());
        self.recording = Some(recording);
        self.status.recording_seconds = Some(0);
        Ok(())
    }

    fn finish(&mut self, mut issue: Option<String>, drain: bool) {
        let Some(mut recording) = self.recording.take() else {
            return;
        };
        let mut capture = self.capture.take();
        if let Some(capture) = &mut capture {
            // Check watchdogs while the recording flag is still set.
            issue = issue.or_else(|| capture.issue().map(str::to_owned));
            capture.stop();
        }
        drop(recording.playback);
        let mut guide_start = None;
        if let Some(capture) = &mut capture {
            if drain
                && let Err(error) = Recording::drain_input(&mut recording.take, capture, usize::MAX)
            {
                issue = Some(match issue {
                    Some(original) => format!("{original} {error}"),
                    None => error,
                });
            }
            guide_start = capture.guide_start_frame();
            issue = issue.or_else(|| capture.issue().map(str::to_owned));
        }
        self.disable_meters();
        self.status.recording_seconds = None;
        if issue.is_none()
            && (recording.take.frames() == 0 || (recording.has_guide && guide_start.is_none()))
        {
            issue = Some("The take stopped before audio started arriving.".into());
        }
        match recording.take.finish(guide_start, issue.as_deref()) {
            Ok(_) if issue.is_some() => {
                self.status.error = issue;
                self.status.notice = Some("Interrupted take kept in its project folder.".into());
            }
            Ok(_) => {
                self.status.saved_takes += 1;
                self.status.notice = Some("Take saved.".into());
            }
            Err(err) => {
                self.status.error = Some(format!(
                    "Couldn't finalize the take. Partial media may remain in its project folder: {err}"
                ))
            }
        }
    }

    fn tick(&mut self, sample_meters: bool) {
        if let Some(issue) = self.click_preview.as_ref().and_then(ClickPreview::issue) {
            self.stop_click();
            self.status.error = Some(issue.into());
        }
        let Some(capture) = &mut self.capture else {
            return;
        };
        if let Some(issue) = capture.issue() {
            if self.recording.is_some() {
                self.finish(Some(issue.into()), true);
            } else {
                self.disable_meters();
                self.status.error = Some(issue.into());
            }
            return;
        }
        if sample_meters {
            self.status.input_ready = capture.input_ready();
            let peaks = capture.peaks();
            self.status.meters = Some(peaks);
            for (flag, peak) in self.status.clipped.iter_mut().zip(peaks) {
                *flag |= peak >= 1.0;
            }
        }
        let Some(rec) = &mut self.recording else {
            return;
        };
        if let Err(issue) = Recording::drain_input(&mut rec.take, capture, 4096) {
            self.finish(Some(issue), false);
            return;
        }
        if let Err(issue) = rec.fill_output() {
            self.finish(Some(issue), true);
            return;
        }
        if rec.checkpoint.elapsed() >= Duration::from_secs(1) {
            if let Err(err) = rec.take.checkpoint() {
                self.finish(Some(format!("Couldn't update the take WAV: {err}")), false);
                return;
            }
            rec.checkpoint = Instant::now();
        }
        self.status.recording_seconds = Some(rec.take.frames() / u64::from(rec.rate));
    }
}

fn run(commands: Receiver<(u64, Command)>, shared: Arc<Mutex<Status>>) {
    let mut session = Session::default();
    let mut next_meter = Instant::now();
    loop {
        let result = if session.capture.is_some() || session.click_preview.is_some() {
            let millis = if session.recording.is_some() { 5 } else { 35 };
            commands.recv_timeout(Duration::from_millis(millis))
        } else {
            commands.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        let acknowledged = match result {
            Ok((seq, command)) => {
                if let Err(err) = session.command(command) {
                    session.status.error = Some(err);
                }
                session.status.completed = seq;
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => {
                session.finish(
                    Some("Demolition closed before the take stopped.".into()),
                    true,
                );
                break;
            }
        };
        let sample = Instant::now() >= next_meter;
        session.tick(sample);
        if acknowledged || sample || (session.capture.is_none() && session.status.error.is_some()) {
            next_meter = Instant::now() + Duration::from_millis(45);
            *shared.lock().unwrap_or_else(|err| err.into_inner()) = session.status.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, thread};

    fn wait_for(worker: &Worker, ready: impl Fn(&Status) -> bool) -> Status {
        let start = Instant::now();
        loop {
            let status = worker.snapshot();
            assert!(status.error.is_none(), "{:?}", status.error);
            if ready(&status) {
                return status;
            }
            assert!(
                start.elapsed() < Duration::from_secs(12),
                "worker timed out"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    #[ignore = "requires a four-input audio interface"]
    fn record_hardware_take() {
        let setting = |name| env::var(name).expect(name);
        let input_name = setting("DEMO_TEST_INPUT");
        let output_name = env::var("DEMO_TEST_OUTPUT").ok();
        let guide = env::var_os("DEMO_TEST_GUIDE").map(PathBuf::from);
        let click = env::var_os("DEMO_TEST_CLICK").is_some();
        let worker = Worker::spawn().unwrap();
        worker.send(1, Command::Scan).unwrap();
        let inventory = wait_for(&worker, |status| status.completed == 1)
            .inventory
            .unwrap();
        let device = |name: &str, input| {
            let mut devices = inventory
                .devices
                .iter()
                .filter(|d| d.name == name && if input { d.input } else { d.output });
            let device = devices.next().expect(name);
            assert!(devices.next().is_none(), "device name is ambiguous: {name}");
            device.id.clone()
        };
        let output = output_name.as_deref().map(|name| device(name, false));
        let name = format!(
            "Smoke {}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        worker
            .send(
                2,
                Command::NewProject {
                    name,
                    parent: setting("DEMO_TEST_FOLDER").into(),
                },
            )
            .unwrap();
        let mut status = wait_for(&worker, |status| status.completed == 2);
        if let Some(path) = guide {
            worker.send(3, Command::InspectGuide(path)).unwrap();
            status = wait_for(&worker, |status| status.completed == 3);
        }
        let rate = status.sample_rate();
        let mut timing = ClickSettings::default();
        if env::var_os("DEMO_TEST_TIMING_CHANGES").is_some() {
            use demolition_audio_io::{Meter, TimingChange};
            timing.meter = Meter { beats: 3, unit: 4 };
            timing.changes.push(TimingChange {
                bar: 2,
                bpm: Some(150),
                meter: Some(Meter { beats: 6, unit: 8 }),
            });
        }
        if click {
            worker
                .send(
                    4,
                    Command::PreviewClick {
                        output: output.clone().unwrap(),
                        timing: timing.clone(),
                    },
                )
                .unwrap();
            wait_for(&worker, |status| {
                status.completed == 4 && status.previewing_click
            });
            thread::sleep(Duration::from_millis(600));
            worker.send(5, Command::StopClick).unwrap();
            wait_for(&worker, |status| {
                status.completed == 5 && !status.previewing_click
            });
        }
        worker
            .send(
                6,
                Command::Meter {
                    input: device(&input_name, true),
                    rate,
                },
            )
            .unwrap();
        wait_for(&worker, |status| status.input_ready);
        worker
            .send(
                7,
                Command::Start {
                    output,
                    timing,
                    click,
                },
            )
            .unwrap();
        wait_for(&worker, |status| {
            status.recording_seconds.is_some_and(|t| t >= 2)
        });
        worker.send(8, Command::Stop).unwrap();
        let finished = wait_for(&worker, |status| status.completed == 8);
        assert_eq!(finished.notice.as_deref(), Some("Take saved."));
        assert_eq!(finished.saved_takes, 1);
        println!("Saved take: {}", finished.project.unwrap().entry.display());
    }
}
