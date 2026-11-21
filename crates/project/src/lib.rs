//! Portable `.demo` session metadata and streaming WAV IO. Never call from an audio callback.

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const CHANNELS: usize = 4;
const FRAME_BYTES: u64 = 4 * CHANNELS as u64;
const MAX_FRAMES: u64 = (u32::MAX as u64 - 128) / FRAME_BYTES;

pub struct Guide {
    reader: WavReader<BufReader<File>>,
}

impl Guide {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = BufReader::new(File::open(path)?);
        let reader = WavReader::new(&mut file)?;
        let spec = reader.spec();
        let duration = reader.duration();
        if !matches!(spec.channels, 1 | 2)
            || !matches!(spec.sample_rate, 44_100 | 48_000)
            || !matches!(
                (spec.sample_format, spec.bits_per_sample),
                (SampleFormat::Int, 8 | 16 | 24 | 32) | (SampleFormat::Float, 32)
            )
            || duration == 0
        {
            return Err(
                "Choose a non-empty mono or stereo PCM/float WAV at 44.1 or 48 kHz.".into(),
            );
        }
        // Hound exposes valid bits, not container width; it decodes padded
        // 24-bit PCM differently from standard left-aligned extensible WAVs.
        // The reader left the cursor just past the data chunk length field.
        file.seek_relative(-4)?;
        let mut data_len = [0; 4];
        file.read_exact(&mut data_len)?;
        let expected = u64::from(duration) * u64::from(spec.channels * (spec.bits_per_sample / 8));
        if u64::from(u32::from_le_bytes(data_len)) != expected {
            return Err(
                "Padded WAV samples aren't supported; export packed PCM or 32-bit float.".into(),
            );
        }
        file.rewind()?;
        Ok(Self {
            reader: WavReader::new(file)?,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.reader.spec().sample_rate
    }

    /// Decode up to `limit` frames; a mono source plays centered.
    pub fn read_frames(&mut self, limit: usize, mut push: impl FnMut([f32; 2])) -> Result<usize> {
        fn read<T: hound::Sample>(
            samples: impl Iterator<Item = hound::Result<T>>,
            limit: usize,
            stereo: bool,
            convert: impl Fn(T) -> f32,
            push: &mut impl FnMut([f32; 2]),
        ) -> Result<usize> {
            let mut samples = samples;
            let mut read = 0;
            while read < limit {
                let Some(left) = samples.next().transpose()? else {
                    break;
                };
                let left = convert(left);
                let right = if stereo {
                    convert(samples.next().ok_or("Incomplete stereo WAV frame")??)
                } else {
                    left
                };
                let sanitize = |v: f32| {
                    if v.is_finite() {
                        v.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                };
                push([sanitize(left), sanitize(right)]);
                read += 1;
            }
            Ok(read)
        }
        let spec = self.reader.spec();
        let stereo = spec.channels == 2;
        match spec.sample_format {
            SampleFormat::Float => read(
                self.reader.samples::<f32>(),
                limit,
                stereo,
                |v| v,
                &mut push,
            ),
            SampleFormat::Int => {
                let full_scale = (1_u32 << (spec.bits_per_sample - 1)) as f32;
                read(
                    self.reader.samples::<i32>(),
                    limit,
                    stereo,
                    |v| v as f32 / full_scale,
                    &mut push,
                )
            }
        }
    }
}

/// Validate a single directory name that can travel between supported platforms.
pub fn validate_project_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err("Enter a project name.".into());
    }
    if name.trim() != name || name.ends_with('.') {
        return Err(
            "Use a project name without leading or trailing whitespace or a trailing dot.".into(),
        );
    }
    if name.len() > 255 {
        return Err("The project name must be at most 255 bytes.".into());
    }
    if name
        .chars()
        .any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
    {
        return Err(
            "The project name contains a character not supported in portable filenames.".into(),
        );
    }
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    {
        return Err("That project name is reserved on Windows. Choose another name.".into());
    }
    Ok(())
}

#[derive(Clone)]
pub struct Project {
    pub name: String,
    pub entry: PathBuf,
}

impl Project {
    pub fn create(parent: &Path, name: &str) -> Result<Self> {
        validate_project_name(name)?;
        let folder = parent.join(name);
        fs::create_dir(&folder)?;
        let entry = folder.join("Session.demo");
        if let Err(err) = Self::initialize(&folder, &entry) {
            let _ = fs::remove_dir_all(&folder);
            return Err(err);
        }
        Ok(Self {
            name: name.to_owned(),
            entry,
        })
    }

    fn initialize(folder: &Path, entry: &Path) -> Result<()> {
        fs::create_dir(folder.join("media"))?;
        let database = Connection::open(entry)?;
        database.execute_batch(
            "PRAGMA synchronous = FULL;
             PRAGMA application_id = 0x44454D4F;
             PRAGMA user_version = 3;
             CREATE TABLE take (
                id INTEGER PRIMARY KEY,
                audio_path TEXT NOT NULL UNIQUE,
                guide_path TEXT UNIQUE,
                sample_rate INTEGER NOT NULL,
                channels INTEGER NOT NULL,
                bpm INTEGER NOT NULL,
                meter_beats INTEGER NOT NULL,
                meter_unit INTEGER NOT NULL,
                click_accent INTEGER NOT NULL,
                click_level INTEGER NOT NULL,
                click_enabled INTEGER NOT NULL,
                frames INTEGER NOT NULL DEFAULT 0,
                guide_start_frame INTEGER,
                state TEXT NOT NULL CHECK(state IN ('recording', 'saved', 'interrupted')),
                issue TEXT
             );
             CREATE TABLE take_timing_change (
                take_id INTEGER NOT NULL REFERENCES take(id),
                bar INTEGER NOT NULL CHECK(bar >= 2),
                bpm INTEGER,
                meter_beats INTEGER,
                meter_unit INTEGER,
                PRIMARY KEY (take_id, bar)
             );",
        )?;
        Ok(())
    }
}

/// Timing snapshot associated with a take, independent of the audio backend.
pub struct TakeTiming {
    pub bpm: u16,
    pub meter: (u8, u8),
    pub accent: bool,
    pub level: u8,
    pub changes: Vec<TakeTimingChange>,
}

pub struct TakeTimingChange {
    pub bar: u32,
    pub bpm: Option<u16>,
    pub meter: Option<(u8, u8)>,
}

pub struct TakeWriter {
    entry: PathBuf,
    database: Connection,
    number: i64,
    wav_path: PathBuf,
    guide_path: Option<PathBuf>,
    writer: WavWriter<BufWriter<File>>,
    frames: u64,
}

impl TakeWriter {
    pub fn start(
        project: &Project,
        guide: Option<&Path>,
        sample_rate: u32,
        timing: &TakeTiming,
        click_enabled: bool,
    ) -> Result<Self> {
        let entry = project.entry.clone();
        let folder = entry
            .parent()
            .ok_or("The project entry has no parent folder.")?;
        let mut database = Connection::open_with_flags(&entry, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        database.execute_batch("PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;")?;
        let transaction = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut number: i64 =
            transaction.query_row("SELECT COALESCE(MAX(id), 0) FROM take", [], |row| {
                row.get(0)
            })?;
        // Reserve a new media directory without overwriting media from an aborted start.
        let (relative, media) = loop {
            number = number
                .checked_add(1)
                .ok_or("No more take numbers are available.")?;
            let relative = format!("media/take-{number}");
            let media = folder.join(&relative);
            match fs::create_dir(&media) {
                Ok(()) => break (relative, media),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err.into()),
            }
        };
        let wav_path = media.join("take.wav");
        let guide_path = guide.map(|_| media.join("guide.wav"));
        let result = (|| -> Result<_> {
            if let (Some(source), Some(path)) = (guide, &guide_path) {
                let mut collected = File::create_new(path)?;
                io::copy(&mut File::open(source)?, &mut collected)?;
                collected.sync_all()?;
                drop(collected);
                // Decode the collected copy, not the user's external source.
                if Guide::open(path)?.sample_rate() != sample_rate {
                    return Err("The guide WAV changed sample rate. Choose it again.".into());
                }
            }
            let mut writer = WavWriter::new(
                BufWriter::new(File::create_new(&wav_path)?),
                WavSpec {
                    channels: CHANNELS as u16,
                    sample_rate,
                    bits_per_sample: 32,
                    sample_format: SampleFormat::Float,
                },
            )?;
            writer.flush()?;
            transaction.execute(
                "INSERT INTO take (id, audio_path, guide_path, sample_rate, channels, bpm,
                     meter_beats, meter_unit, click_accent, click_level, click_enabled, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'recording')",
                params![
                    number,
                    format!("{relative}/take.wav"),
                    guide.map(|_| format!("{relative}/guide.wav")),
                    sample_rate,
                    CHANNELS as u16,
                    timing.bpm,
                    timing.meter.0,
                    timing.meter.1,
                    timing.accent,
                    timing.level,
                    click_enabled,
                ],
            )?;
            {
                let mut insert = transaction.prepare(
                    "INSERT INTO take_timing_change (take_id, bar, bpm, meter_beats, meter_unit)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for change in &timing.changes {
                    insert.execute(params![
                        number,
                        change.bar,
                        change.bpm,
                        change.meter.map(|meter| meter.0),
                        change.meter.map(|meter| meter.1),
                    ])?;
                }
            }
            transaction.commit()?;
            Ok(writer)
        })();
        let writer = match result {
            Ok(writer) => writer,
            Err(err) => {
                let _ = fs::remove_dir_all(&media);
                return Err(err);
            }
        };
        Ok(Self {
            entry,
            database,
            number,
            wav_path,
            guide_path,
            writer,
            frames: 0,
        })
    }

    pub fn entry(&self) -> &Path {
        &self.entry
    }

    pub fn guide_path(&self) -> Option<&Path> {
        self.guide_path.as_deref()
    }

    pub fn number(&self) -> i64 {
        self.number
    }

    pub fn write_frame(&mut self, frame: [f32; CHANNELS]) -> Result<()> {
        // Leave room for the header in a standard RIFF WAV.
        if self.frames >= MAX_FRAMES {
            return Err("The take reached the WAV file size limit.".into());
        }
        for sample in frame {
            self.writer.write_sample(sample)?;
        }
        self.frames += 1;
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Update the WAV header for completed frames. Power-loss durability is not implied.
    pub fn checkpoint(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    pub fn finish(self, guide_start_frame: Option<i64>, issue: Option<&str>) -> Result<PathBuf> {
        let finalized = self
            .writer
            .finalize()
            .map_err(io::Error::other)
            .and_then(|()| {
                fs::OpenOptions::new()
                    .write(true)
                    .open(&self.wav_path)?
                    .sync_all()
            });
        let mut frames = self.frames;
        let mut final_issue = issue.map(str::to_owned);
        let finalization_failed = finalized.is_err();
        if let Err(err) = finalized {
            let reason = match salvage_wav(&self.wav_path, frames) {
                Ok(recovered) => {
                    frames = recovered;
                    format!("WAV finalization failed ({err}); kept {frames} complete frames.")
                }
                Err(recovery) => {
                    frames = 0; // No durable frame count could be established.
                    format!("WAV finalization failed ({err}); repair failed ({recovery}).")
                }
            };
            final_issue = Some(match final_issue {
                Some(original) => format!("{original} {reason}"),
                None => reason,
            });
        }
        let update = self.database.execute(
            "UPDATE take SET frames=?1, guide_start_frame=?2, state=?3, issue=?4 WHERE id=?5",
            params![
                frames as i64, // The RIFF limit bounds this well below i64::MAX.
                guide_start_frame,
                if final_issue.is_some() {
                    "interrupted"
                } else {
                    "saved"
                },
                final_issue,
                self.number,
            ],
        );
        if let Err(error) = update {
            return Err(match final_issue {
                Some(issue) => format!("{issue} Couldn't update project metadata: {error}"),
                None => format!("Couldn't update project metadata: {error}"),
            }
            .into());
        }
        if finalization_failed {
            return Err(final_issue.unwrap().into());
        }
        Ok(self.entry)
    }
}

// Best effort for our own hound-generated four-channel float WAV, not a
// general WAV repair routine. Discard a partial frame and any unwritten tail.
fn salvage_wav(path: &Path, completed_frames: u64) -> io::Result<u64> {
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    let mut header = [0; 68];
    file.read_exact(&mut header)?;
    if &header[..4] != b"RIFF"
        || &header[8..16] != b"WAVEfmt "
        || header[16..20] != 40_u32.to_le_bytes()
        || &header[60..64] != b"data"
    {
        return Err(io::Error::other("Unexpected take WAV header"));
    }
    let frames =
        ((file.metadata()?.len() - header.len() as u64) / FRAME_BYTES).min(completed_frames);
    let data_len = frames * FRAME_BYTES;
    let total = header.len() as u64 + data_len;
    file.set_len(total)?;
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&((total - 8) as u32).to_le_bytes())?;
    file.seek(SeekFrom::Start(64))?;
    file.write_all(&(data_len as u32).to_le_bytes())?;
    file.sync_all()?;
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(bpm: u16) -> TakeTiming {
        TakeTiming {
            bpm,
            meter: (4, 4),
            accent: true,
            level: 70,
            changes: Vec::new(),
        }
    }

    #[test]
    fn take_saves_timing_map_with_independent_meter_and_tempo_changes() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path(), "Timing").unwrap();
        let timing = TakeTiming {
            meter: (6, 8),
            accent: false,
            level: 55,
            changes: vec![
                TakeTimingChange {
                    bar: 3,
                    bpm: Some(110),
                    meter: None,
                },
                TakeTimingChange {
                    bar: 7,
                    bpm: None,
                    meter: Some((5, 4)),
                },
            ],
            ..timing(130)
        };
        let take = TakeWriter::start(&project, None, 48_000, &timing, true).unwrap();
        take.finish(None, None).unwrap();
        let db = Connection::open(project.entry).unwrap();
        let initial: (u16, u8, u8, bool, u8) = db.query_row(
            "SELECT bpm, meter_beats, meter_unit, click_accent, click_level FROM take WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).unwrap();
        assert_eq!(initial, (130, 6, 8, false, 55));
        type ChangeRow = (u32, Option<u16>, Option<u8>, Option<u8>);
        let mut query = db.prepare("SELECT bar, bpm, meter_beats, meter_unit FROM take_timing_change WHERE take_id = 1 ORDER BY bar").unwrap();
        let changes: Vec<ChangeRow> = query
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            changes,
            [(3, Some(110), None, None), (7, None, Some(5), Some(4))]
        );
    }

    fn mono_guide(parent: &Path) -> PathBuf {
        let source = parent.join("mono.wav");
        let mut writer = WavWriter::create(
            &source,
            WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [-16384_i16, 0, 16384] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        source
    }

    #[test]
    fn project_creation_validates_names_and_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "",
            " ",
            ".",
            "..",
            "name.",
            "name ",
            " name",
            "../name",
            r"a\b",
            "C:foo",
            "a\0b",
            "a*b",
            "a?b",
            "a|b",
            "a<b",
            "a>b",
            "a\"b",
            "cOn.txt",
            "NUL",
            "COM1.wav",
            "lpt².dat",
            &"a".repeat(256),
        ] {
            assert!(validate_project_name(name).is_err(), "{name:?}");
            assert!(Project::create(dir.path(), name).is_err(), "{name:?}");
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

        let name = "Björk Session 1.2";
        let project = Project::create(dir.path(), name).unwrap();
        let folder = dir.path().join(name);
        assert_eq!(project.name, name);
        assert_eq!(project.entry, folder.join("Session.demo"));
        assert!(project.entry.is_file());
        let media = folder.join("media");
        assert_eq!(fs::read_dir(&media).unwrap().count(), 0);
        let sentinel = media.join("keep.wav");
        fs::write(&sentinel, b"existing media").unwrap();
        assert!(Project::create(dir.path(), name).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"existing media");
        let db = Connection::open(project.entry).unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM take", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn multiple_takes_finalize_independently_with_optional_guide_and_click_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path(), "Demo").unwrap();
        let source = mono_guide(dir.path());
        let mut first =
            TakeWriter::start(&project, Some(&source), 48_000, &timing(120), false).unwrap();
        let mut second = TakeWriter::start(&project, None, 44_100, &timing(95), true).unwrap();
        assert_eq!((first.number(), second.number()), (1, 2));
        assert_eq!(first.entry(), second.entry());
        let collected = first.guide_path().unwrap().to_owned();
        assert_ne!(collected, source);
        assert!(second.guide_path().is_none());

        first.write_frame([0.5; CHANNELS]).unwrap();
        second.write_frame([0.0; CHANNELS]).unwrap();
        second.write_frame([0.0; CHANNELS]).unwrap();
        assert_eq!(
            second.finish(None, Some("Input disconnected")).unwrap(),
            project.entry
        );
        assert_eq!(first.finish(Some(256), None).unwrap(), project.entry);

        let db = Connection::open(&project.entry).unwrap();
        let mut query = db
            .prepare(
                "SELECT audio_path, guide_path, sample_rate, bpm, click_enabled, frames, state
             FROM take ORDER BY id",
            )
            .unwrap();
        let mut rows = query.query([]).unwrap();
        let folder = project.entry.parent().unwrap();
        let mut audio_paths = Vec::new();
        for (guide, rate, bpm, click, frames, state) in [
            (
                Some(collected.as_path()),
                48_000_u32,
                120_u16,
                false,
                1_i64,
                "saved",
            ),
            (None, 44_100, 95, true, 2, "interrupted"),
        ] {
            let row = rows.next().unwrap().unwrap();
            let audio: String = row.get(0).unwrap();
            let relative_guide: Option<String> = row.get(1).unwrap();
            assert!(Path::new(&audio).is_relative());
            assert!(
                relative_guide
                    .as_deref()
                    .is_none_or(|path| Path::new(path).is_relative())
            );
            assert_eq!(
                relative_guide.map(|path| folder.join(path)).as_deref(),
                guide
            );
            assert_eq!(row.get::<_, u32>(2).unwrap(), rate);
            assert_eq!(row.get::<_, u16>(3).unwrap(), bpm);
            assert_eq!(row.get::<_, bool>(4).unwrap(), click);
            assert_eq!(row.get::<_, i64>(5).unwrap(), frames);
            assert_eq!(row.get::<_, String>(6).unwrap(), state);
            let wav = WavReader::open(folder.join(&audio)).unwrap();
            assert_eq!(i64::from(wav.duration()), frames);
            assert_eq!(wav.spec().sample_rate, rate);
            audio_paths.push(audio);
        }
        assert_ne!(audio_paths[0], audio_paths[1]);
        assert!(rows.next().unwrap().is_none());
    }

    #[test]
    fn failed_guide_start_leaves_project_ready_to_record() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path(), "Demo").unwrap();
        let guide = mono_guide(dir.path());
        assert!(TakeWriter::start(&project, Some(&guide), 44_100, &timing(120), false).is_err());
        let media = project.entry.parent().unwrap().join("media");
        assert_eq!(fs::read_dir(media).unwrap().count(), 0);
        let take = TakeWriter::start(&project, None, 44_100, &timing(120), false).unwrap();
        assert_eq!(take.number(), 1);
        take.finish(None, None).unwrap();
    }

    #[test]
    fn guide_decodes_in_bounded_chunks_and_centers_mono() {
        let dir = tempfile::tempdir().unwrap();
        let mut guide = Guide::open(&mono_guide(dir.path())).unwrap();
        let mut frames = Vec::new();
        assert_eq!(guide.read_frames(2, |f| frames.push(f)).unwrap(), 2);
        assert_eq!(guide.read_frames(2, |f| frames.push(f)).unwrap(), 1);
        assert_eq!(frames, [[-0.5; 2], [0.0; 2], [0.5; 2]]);
    }

    #[test]
    fn finalization_preserves_channel_order_and_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let guide = mono_guide(dir.path());
        let project = Project::create(dir.path(), "Demo").unwrap();
        let mut take =
            TakeWriter::start(&project, Some(&guide), 48_000, &timing(120), false).unwrap();
        let samples = [0.125_f32, -0.25, 0.5, -1.0];
        take.write_frame(samples).unwrap();
        take.checkpoint().unwrap();
        let entry = take.finish(Some(256), None).unwrap();
        let relocated = dir.path().join("Relocated Session");
        fs::rename(entry.parent().unwrap(), &relocated).unwrap();
        fs::remove_file(guide).unwrap();
        let db = Connection::open(relocated.join("Session.demo")).unwrap();
        let (audio, guide, frames, offset, state): (String, String, i64, i64, String) = db
            .query_row(
                "SELECT audio_path, guide_path, frames, guide_start_frame, state FROM take",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((frames, offset, state.as_str()), (1, 256, "saved"));
        let mut wav = WavReader::open(relocated.join(audio)).unwrap();
        assert_eq!(wav.spec().channels, 4);
        assert_eq!(
            wav.samples::<f32>()
                .collect::<hound::Result<Vec<_>>>()
                .unwrap(),
            samples
        );
        assert_eq!(
            Guide::open(&relocated.join(guide)).unwrap().sample_rate(),
            48_000
        );
    }

    #[test]
    fn padded_24_bit_guides_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padded.wav");
        let spec = hound::WavSpecEx {
            spec: WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 24,
                sample_format: SampleFormat::Int,
            },
            bytes_per_sample: 4,
        };
        let mut writer = WavWriter::new_with_spec_ex(File::create(&path).unwrap(), spec).unwrap();
        writer.write_sample(1_i32).unwrap();
        writer.finalize().unwrap();
        let error = Guide::open(&path).err().unwrap();
        assert!(error.to_string().contains("Padded WAV"));
    }

    #[test]
    fn finalization_repairs_a_partial_frame_and_marks_take_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path(), "Demo").unwrap();
        let mut take = TakeWriter::start(&project, None, 48_000, &timing(120), false).unwrap();
        let samples = [0.125_f32, -0.25, 0.5, -1.0];
        take.write_frame(samples).unwrap();
        take.writer.write_sample(0.25_f32).unwrap();
        let entry = take.entry().to_owned();
        let wav_path = take.wav_path.clone();
        let error = take
            .finish(None, Some("Simulated write failure"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Simulated write failure") && error.contains("kept 1 complete frames"),
            "{error}"
        );
        let mut wav = WavReader::open(&wav_path).unwrap();
        assert_eq!(
            wav.samples::<f32>()
                .collect::<hound::Result<Vec<_>>>()
                .unwrap(),
            samples
        );
        let db = Connection::open(entry).unwrap();
        let (frames, state): (i64, String) = db
            .query_row("SELECT frames, state FROM take", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((frames, state.as_str()), (1, "interrupted"));
    }

    #[test]
    fn interrupted_take_is_not_marked_saved() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path(), "Demo").unwrap();
        let mut take = TakeWriter::start(&project, None, 48_000, &timing(120), false).unwrap();
        take.write_frame([0.0; 4]).unwrap();
        let db = Connection::open(take.finish(None, Some("Input disconnected")).unwrap()).unwrap();
        let (state, issue): (String, String) = db
            .query_row("SELECT state, issue FROM take", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(
            (state.as_str(), issue.as_str()),
            ("interrupted", "Input disconnected")
        );
    }
}
