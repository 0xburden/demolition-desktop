#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod palette;
mod preferences;
mod recording;
mod timing;

use demolition_audio_io::CHANNELS;
use demolition_project::validate_project_name;
use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::Theme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::{
    App, AppContext, Bounds, Context, Div, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled,
    Window, WindowBounds, WindowOptions, div, px, relative, size,
};
use preferences::{Appearance, appearance_path, load_appearance, save_appearance};
use recording::{Command, Status, Worker};
use std::{path::PathBuf, sync::Arc, time::Duration};
use timing::TimingEditor;

#[derive(Clone)]
struct DeviceOption {
    name: SharedString,
    id: String,
}

impl SearchableListItem for DeviceOption {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &String {
        &self.id
    }
}

type DeviceSelect = SelectState<SearchableVec<DeviceOption>>;

#[derive(Clone, Copy)]
enum Destination {
    Guide,
    Folder,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Welcome,
    NewProject,
    Workspace,
}

struct Pending {
    label: &'static str,
    protects_take: bool,
}

struct RecorderView {
    worker: Option<Worker>,
    status: Status,
    sequence: u64,
    pending: Option<Pending>,
    input: Entity<DeviceSelect>,
    output: Entity<DeviceSelect>,
    screen: Screen,
    project_name: Entity<InputState>,
    project_attempt: Option<u64>,
    folder: Option<PathBuf>,
    timing: TimingEditor,
    show_timing: bool,
    appearance: Appearance,
    appearance_path: Option<PathBuf>,
    settings_error: Option<String>,
    show_settings: bool,
    record_click: bool,
    choosing_path: bool,
    local_error: Option<String>,
}

impl RecorderView {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        appearance: Appearance,
        appearance_path: Option<PathBuf>,
        settings_error: Option<String>,
    ) -> Self {
        appearance.apply(window.appearance(), cx);
        cx.observe_window_appearance(window, |view, window, cx| {
            view.appearance.apply(window.appearance(), cx);
        })
        .detach();
        let make_select = |cx: &mut Context<DeviceSelect>, window: &mut Window| {
            SelectState::new(SearchableVec::new(Vec::new()), None, window, cx)
        };
        let input = cx.new(|cx| make_select(cx, window));
        let output = cx.new(|cx| make_select(cx, window));
        cx.subscribe(&input, |view, _, _: &SelectEvent<_>, cx| {
            if view.status.meters.is_some() && !view.busy() {
                view.send(Command::DisableMeters, "Changing input…", cx);
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&output, |_, _, _: &SelectEvent<_>, cx| cx.notify())
            .detach();

        let worker = Worker::spawn();
        let local_error = worker
            .as_ref()
            .err()
            .map(|err| format!("Couldn’t start the recording worker: {err}"));
        let project_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("e.g. Friday Rehearsal"));
        let timing = TimingEditor::new(window, cx);
        cx.subscribe(&project_name, |view, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                view.local_error = None;
                cx.notify();
            }
        })
        .detach();
        let view = Self {
            worker: worker.ok(),
            status: Status::default(),
            sequence: 0,
            pending: None,
            input,
            output,
            screen: Screen::Welcome,
            project_name,
            project_attempt: None,
            folder: None,
            timing,
            show_timing: false,
            appearance,
            appearance_path,
            settings_error,
            show_settings: false,
            record_click: false,
            choosing_path: false,
            local_error,
        };
        if view.worker.is_some() {
            let executor = cx.background_executor().clone();
            cx.spawn_in(window, async move |view, cx| {
                loop {
                    executor.timer(Duration::from_millis(55)).await;
                    if view.update_in(cx, Self::refresh_status).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        let entity = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            entity
                .update(cx, |view, cx| {
                    if view.unsaved_take() {
                        view.local_error =
                            Some("Stop and save the take before closing Demolition.".into());
                        cx.notify();
                        false
                    } else {
                        true
                    }
                })
                .unwrap_or(true)
        });
        cx.on_app_quit(|view, _| {
            // Dock/app Quit can bypass the window guard. Wait for the worker to
            // close the unfinished take instead of letting the process kill it.
            if view.unsaved_take()
                && let Some(worker) = view.worker.take()
            {
                worker.shutdown();
            }
            std::future::ready(())
        })
        .detach();
        view
    }

    fn unsaved_take(&self) -> bool {
        self.status.recording_seconds.is_some()
            || self.pending.as_ref().is_some_and(|job| job.protects_take)
    }

    fn busy(&self) -> bool {
        self.worker.is_none()
            || self.pending.is_some()
            || self.choosing_path
            || self.status.recording_seconds.is_some()
    }

    fn send(&mut self, command: Command, progress: &'static str, cx: &mut Context<Self>) {
        if let Some(worker) = &self.worker {
            let seq = self.sequence + 1;
            let protects_take = matches!(command, Command::Start { .. } | Command::Stop);
            match worker.send(seq, command) {
                Ok(()) => {
                    self.sequence = seq;
                    self.pending = Some(Pending {
                        label: progress,
                        protects_take,
                    });
                    self.local_error = None;
                }
                Err(err) => self.local_error = Some(err),
            }
            cx.notify();
        }
    }

    fn refresh_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worker) = &self.worker else { return };
        let status = worker.snapshot();
        let new_project = status.project.as_ref().map(Arc::as_ptr)
            != self.status.project.as_ref().map(Arc::as_ptr);
        let changed = new_project
            || status.completed != self.status.completed
            || status.meters != self.status.meters
            || status.input_ready != self.status.input_ready
            || status.previewing_click != self.status.previewing_click
            || status.clipped != self.status.clipped
            || status.recording_seconds != self.status.recording_seconds
            || status.error != self.status.error
            || status.notice != self.status.notice;
        if status.inventory.as_ref().map(Arc::as_ptr)
            != self.status.inventory.as_ref().map(Arc::as_ptr)
        {
            for (select, input) in [(&self.input, true), (&self.output, false)] {
                let options = status
                    .inventory
                    .iter()
                    .flat_map(|list| &list.devices)
                    .filter(|device| if input { device.input } else { device.output })
                    .map(|device| DeviceOption {
                        name: device.name.clone().into(),
                        id: device.id.clone(),
                    })
                    .collect::<Vec<_>>();
                select.update(cx, |select, cx| {
                    let suggested = if options.len() == 1 {
                        Some(options[0].id.clone())
                    } else if !input {
                        status.inventory.as_ref().and_then(|list| {
                            let id = list.default_output.as_ref()?;
                            options
                                .iter()
                                .any(|device| &device.id == id)
                                .then(|| id.clone())
                        })
                    } else {
                        None
                    };
                    select.set_items(SearchableVec::new(options), window, cx);
                    if select.selected_value().is_none()
                        && let Some(id) = suggested
                    {
                        select.set_selected_value(&id, window, cx);
                    }
                });
            }
        }
        if status.completed == self.sequence {
            self.pending = None;
        }
        self.status = status;
        if new_project {
            self.screen = Screen::Workspace;
            self.folder = None;
            self.record_click = false;
            self.timing = TimingEditor::new(window, cx);
            self.show_timing = false;
            if self.status.inventory.is_none() {
                self.send(Command::Scan, "Looking for audio devices…", cx);
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn choose_path(&mut self, which: Destination, cx: &mut Context<Self>) {
        let is_guide = matches!(which, Destination::Guide);
        if !is_guide
            && let Err(error) = validate_project_name(self.project_name.read(cx).value().trim())
        {
            self.local_error = Some(error.to_string());
            cx.notify();
            return;
        }
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: is_guide,
            directories: !is_guide,
            multiple: false,
            prompt: Some(
                if is_guide {
                    "Choose guide WAV"
                } else {
                    "Choose the folder for your new project"
                }
                .into(),
            ),
        });
        self.choosing_path = true;
        self.local_error = None;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let picked = selection.await;
            let _ = view.update(cx, |view, cx| {
                view.choosing_path = false;
                match picked {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            match which {
                                Destination::Guide => view.send(
                                    Command::InspectGuide(path),
                                    "Checking guide WAV…",
                                    cx,
                                ),
                                Destination::Folder => view.folder = Some(path),
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => {
                        view.local_error = Some(format!("Couldn’t open the file picker: {err}"))
                    }
                    Err(_) => {
                        view.local_error =
                            Some("The file picker closed unexpectedly. Try again.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_meters(&mut self, cx: &mut Context<Self>) {
        if self.status.meters.is_some() {
            self.send(Command::DisableMeters, "Stopping meters…", cx);
        } else if let Some(input) = self.input.read(cx).selected_value().cloned() {
            let rate = self.status.sample_rate();
            self.send(Command::Meter { input, rate }, "Opening input…", cx);
        }
    }

    fn new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::NewProject;
        self.local_error = None;
        self.project_attempt = None;
        self.folder = None;
        self.project_name
            .update(cx, |name, cx| name.set_value("", window, cx));
        cx.notify();
    }

    fn create_project(&mut self, cx: &mut Context<Self>) {
        let name = self.project_name.read(cx).value().trim().to_owned();
        if let Err(error) = validate_project_name(&name) {
            self.local_error = Some(error.to_string());
            cx.notify();
        } else if let Some(parent) = self.folder.clone() {
            let before = self.sequence;
            self.send(
                Command::NewProject { name, parent },
                "Creating project…",
                cx,
            );
            if self.sequence != before {
                self.project_attempt = Some(self.sequence);
            }
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if let Ok(timing) = self.timing.settings(cx) {
            let output = self.output.read(cx).selected_value().cloned();
            self.send(
                Command::Start {
                    output,
                    timing,
                    click: self.record_click,
                },
                "Starting take…",
                cx,
            );
        }
    }

    fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        if self.status.previewing_click {
            self.send(Command::StopClick, "Stopping click…", cx);
        } else if let (Ok(timing), Some(output)) = (
            self.timing.settings(cx),
            self.output.read(cx).selected_value().cloned(),
        ) {
            self.send(
                Command::PreviewClick { output, timing },
                "Starting click…",
                cx,
            );
        }
    }

    fn set_appearance(&mut self, appearance: Appearance, window: &Window, cx: &mut Context<Self>) {
        if self.appearance == appearance && self.settings_error.is_none() {
            return;
        }
        self.appearance = appearance;
        cx.set_window_appearance(appearance.override_appearance());
        // Cocoa's cached window value may still contain the old forced appearance.
        // Read the app's effective appearance after dropping that override.
        let system = if cfg!(target_os = "macos") {
            cx.window_appearance()
        } else {
            window.appearance()
        };
        appearance.apply(system, cx);
        self.settings_error = self
            .appearance_path
            .as_deref()
            .ok_or_else(|| std::io::Error::other("No user settings directory"))
            .and_then(|path| save_appearance(path, appearance))
            .err()
            .map(|error| {
                format!("Appearance changed for this session but couldn’t be saved: {error}")
            });
        cx.notify();
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let theme = Theme::global(cx).colors;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_5()
            .py_3()
            .bg(palette::panel_surface(cx))
            .border_b_1()
            .border_color(theme.border)
            .child(div().font_semibold().child("Appearance"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(Appearance::ALL.map(|appearance| {
                        let current = self.appearance == appearance;
                        let button = Button::new(appearance.key())
                            .label(appearance.label())
                            .toggled(current);
                        (if current {
                            button.primary()
                        } else {
                            button.secondary()
                        })
                        .on_click(cx.listener(
                            move |view, _, window, cx| view.set_appearance(appearance, window, cx),
                        ))
                    })),
            )
            .child(div().text_sm().text_color(theme.muted_foreground).child(
                "System follows your operating system. This setting applies to all projects.",
            ))
            .children(self.settings_error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone())
            }))
    }

    fn render_wizard(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::global(cx).colors;
        let welcome = self.screen == Screen::Welcome;
        let busy = self.busy();
        let name = self.project_name.read(cx).value();
        let folder = self.folder.as_ref().map_or_else(
            || {
                "Choose a parent folder. Demolition creates a named project folder inside it."
                    .into()
            },
            |path| path.join(name.trim()).display().to_string(),
        );
        let content = div()
            .w_full()
            .max_w(px(520.))
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .bg(palette::panel_surface(cx))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .child(div().text_2xl().font_semibold().child(if welcome {
                "Start a session"
            } else {
                "New project"
            }))
            .children(if welcome {
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_4()
                        .child("Create a project folder for your recordings and media.")
                        .child(
                            Button::new("new-project")
                                .primary()
                                .label("New Project")
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.new_project(window, cx)),
                                ),
                        ),
                )
            } else {
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(div().text_sm().child("Project name"))
                        .child(
                            Input::new(&self.project_name)
                                .aria_label("Project name")
                                .disabled(busy),
                        )
                        .child(div().text_sm().child("Project location"))
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(folder),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    Button::new("project-back")
                                        .ghost()
                                        .label("Back")
                                        .disabled(busy)
                                        .on_click(cx.listener(|view, _, _, cx| {
                                            view.screen = if view.status.project.is_some() {
                                                Screen::Workspace
                                            } else {
                                                Screen::Welcome
                                            };
                                            view.local_error = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            Button::new("project-location")
                                                .label("Choose folder…")
                                                .disabled(busy || name.trim().is_empty())
                                                .on_click(cx.listener(|view, _, _, cx| {
                                                    view.choose_path(Destination::Folder, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("create-project")
                                                .primary()
                                                .label(if self.pending.is_some() {
                                                    "Creating…"
                                                } else {
                                                    "Create Project"
                                                })
                                                .disabled(
                                                    busy || self.folder.is_none()
                                                        || name.trim().is_empty(),
                                                )
                                                .on_click(cx.listener(|view, _, _, cx| {
                                                    view.create_project(cx)
                                                })),
                                        ),
                                ),
                        ),
                )
            })
            .children(
                self.local_error
                    .as_ref()
                    .or(self.status.error.as_ref().filter(|_| {
                        !welcome && self.project_attempt == Some(self.status.completed)
                    }))
                    .map(|error| {
                        div()
                            .text_sm()
                            .text_color(theme.danger)
                            .child(error.clone())
                    }),
            );
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_5()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(content)
    }

    fn render_workspace(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let theme = Theme::global(cx).colors;
        let dark = Theme::global(cx).is_dark();
        let panel = palette::panel_surface(cx);
        let mono_font = Theme::global(cx).mono_font_family.clone();
        let previewing = self.status.previewing_click;
        let busy = self.busy() || previewing;
        let recording = self.status.recording_seconds;
        let project = self
            .status
            .project
            .as_ref()
            .expect("workspace requires a project");
        let output_selected = self.output.read(cx).selected_value().is_some();
        let timing_summary = self.timing.summary(cx);
        let valid_timing = timing_summary.is_ok();
        let guide = self.status.guide.as_ref().map_or_else(
            || "None · mono/stereo WAV at 44.1 or 48 kHz".into(),
            |(path, rate)| {
                format!(
                    "{} · {} kHz",
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy(),
                    *rate as f32 / 1000.0
                )
            },
        );
        let can_record = !busy
            && self.status.input_ready
            && valid_timing
            && ((self.status.guide.is_none() && !self.record_click) || output_selected);
        let system = self.status.inventory.as_ref().map_or_else(
            || "Audio devices".into(),
            |list| {
                if list.devices.is_empty() {
                    format!("{} · No devices found", list.backend)
                } else if list.unavailable > 0 {
                    format!("{} · Some devices couldn’t be read", list.backend)
                } else {
                    format!("{} · Audio devices", list.backend)
                }
            },
        );
        let field = |label, text: String, button| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div().flex_1().min_w_0().child(div().child(label)).child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .text_ellipsis()
                            .child(text),
                    ),
                )
                .child(button)
        };
        let card = || {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .bg(panel)
                .border_1()
                .border_color(theme.border)
                .rounded_lg()
        };
        let mut levels = div().flex().flex_col().gap_2();
        for index in 0..CHANNELS {
            let peak = self.status.meters.map_or(0.0, |peaks| peaks[index]);
            let clipped = self.status.clipped[index];
            let fill = ((20.0 * peak.max(0.001).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
            levels = levels.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w(px(52.))
                            .text_sm()
                            .child(format!("Input {}", index + 1)),
                    )
                    .child(input_meter(
                        fill,
                        theme.muted,
                        if clipped { theme.danger } else { theme.success },
                        dark,
                    ))
                    .child(
                        div()
                            .w(px(68.))
                            .text_right()
                            .text_xs()
                            .text_color(if clipped {
                                theme.danger
                            } else {
                                theme.muted_foreground
                            })
                            .child(if clipped {
                                "CLIP".into()
                            } else if fill == 0.0 {
                                "—".into()
                            } else {
                                format!("{:.0} dBFS", 20.0 * peak.log10())
                            }),
                    ),
            );
        }
        let state = if let Some(pending) = &self.pending {
            pending.label.into()
        } else if let Some(seconds) = recording {
            format!(
                "Recording take {} · {:02}:{:02}",
                self.status.last_take.unwrap_or(1),
                seconds / 60,
                seconds % 60
            )
        } else if previewing {
            "Metronome playing".into()
        } else if let Some(notice) = &self.status.notice {
            notice.clone()
        } else if can_record {
            "Ready to record".into()
        } else {
            "Set up to record".into()
        };
        let meter_state = if self.status.input_ready {
            "Meters live"
        } else if self.status.meters.is_some() {
            "Waiting for input…"
        } else {
            "Meters off"
        };

        let display = div().text_sm().child(
            timing_summary
                .clone()
                .unwrap_or_else(|_| "Timing needs attention".into()),
        );
        let display = if dark {
            display
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .px_2()
                .py_1()
                .font_family(mono_font)
        } else {
            display
        };
        let record_click = Button::new("record-click").toggled(self.record_click);
        let record_click = if dark && self.record_click {
            record_click.primary()
        } else {
            record_click
        };
        let preview = Button::new("preview-click");
        let preview = if dark && !previewing {
            preview.success().outline()
        } else {
            preview
        };

        div().size_full().flex().flex_col().gap_4().p_5().bg(theme.background).text_color(theme.foreground)
            .child(div().flex().items_center().justify_between().gap_3()
                .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                    .child(div().text_2xl().font_semibold().text_ellipsis().child(project.name.clone()))
                    .child(div().text_sm().text_color(theme.muted_foreground).text_ellipsis().child(project.entry.display().to_string())))
                .child(Button::new("workspace-new-project").label("New Project…").disabled(busy)
                    .on_click(cx.listener(|view, _, window, cx| view.new_project(window, cx)))))
            .child(card()
                .child(div().flex().items_center().justify_between().gap_3()
                    .child(div().font_semibold().text_color(if recording.is_some() { theme.danger } else { theme.foreground }).child(state))
                    .child(if recording.is_some() {
                        Button::new("stop-take").secondary().label("Stop & Save").disabled(self.pending.is_some())
                            .on_click(cx.listener(|view, _, _, cx| view.send(Command::Stop, "Saving take…", cx)))
                    } else {
                        Button::new("record-take").danger().label("Record").disabled(!can_record)
                            .on_click(cx.listener(|view, _, _, cx| view.start(cx)))
                    }))
                .child(div().flex().flex_wrap().items_center().gap_3()
                    .child(display)
                    .child(Button::new("timing-settings").label(if self.show_timing { "Hide Timing Settings" } else { "Timing Settings…" })
                        .on_click(cx.listener(|view, _, _, cx| { view.show_timing = !view.show_timing; cx.notify(); })))
                    .child(record_click.label(if self.record_click { "Click while recording: On" } else { "Click while recording: Off" }).disabled(busy)
                        .on_click(cx.listener(|view, _, _, cx| { view.record_click = !view.record_click; cx.notify(); })))
                    .child(preview.label(if previewing { "Stop Click" } else { "Play Click" })
                        .disabled(self.busy() || (!previewing && (!valid_timing || !output_selected)))
                        .on_click(cx.listener(|view, _, _, cx| view.toggle_preview(cx)))))
                .children(timing_summary.err().map(|error| div().text_sm().text_color(theme.danger).child(error))))
            .child(div().id("workspace-body").flex_1().min_h_0().overflow_y_scroll()
                .child(div().flex().flex_col().gap_3().pr_1()
                    .children(self.show_timing.then(|| self.timing.render(busy, cx)))
                    .child(card()
                        .child(div().flex().items_center().justify_between().gap_3()
                            .child(div().font_semibold().child("Inputs 1–4"))
                            .child(div().flex().items_center().gap_3()
                                .child(div().text_sm().text_color(theme.muted_foreground).child(meter_state))
                                .child(Button::new("toggle-meters").label(if self.status.meters.is_some() { "Turn Meters Off" } else { "Enable Meters" })
                                    .disabled(busy || self.input.read(cx).selected_value().is_none())
                                    .on_click(cx.listener(|view, _, _, cx| view.toggle_meters(cx))))))
                        .child(levels))
                    .child(card()
                        .child(div().flex().items_center().justify_between().gap_3().child(system)
                            .child(Button::new("refresh-devices").label("Refresh").disabled(busy)
                                .on_click(cx.listener(|view, _, _, cx| view.send(Command::Scan, "Looking for audio devices…", cx)))))
                        .child(div().flex().gap_3()
                            .child(div().flex_1().min_w_0().child("Input").child(Select::new(&self.input).w_full().accessibility_label("Input device").placeholder("Choose input interface").disabled(busy)))
                            .child(div().flex_1().min_w_0().child("Output").child(Select::new(&self.output).w_full().accessibility_label("Output device").placeholder("Choose output for click or guide").disabled(busy)))))
                    .child(card().child(field("Guide WAV (optional)", guide,
                        div().flex().gap_2()
                            .children(self.status.guide.as_ref().map(|_| Button::new("clear-guide").label("Remove").disabled(busy)
                                .on_click(cx.listener(|view, _, _, cx| view.send(Command::ClearGuide, "Removing guide…", cx)))))
                            .child(Button::new("choose-guide").label("Choose WAV…").disabled(busy)
                                .on_click(cx.listener(|view, _, _, cx| view.choose_path(Destination::Guide, cx)))))))))
            .child(div().flex().flex_col().gap_2()
                .children(self.local_error.as_ref().or(self.status.error.as_ref()).map(|error| {
                    div().text_sm().text_color(theme.danger).child(error.clone())
                }))
                .child(div().text_sm().text_color(theme.muted_foreground)
                    .child(format!("{} saved {} · Input monitoring is off · Click isn’t mixed into the take", self.status.saved_takes,
                        if self.status.saved_takes == 1 { "take" } else { "takes" }))))
    }
}

fn input_meter(fill: f32, track: Hsla, signal: Hsla, segmented: bool) -> Div {
    let bar = div().flex_1().h(px(8.));
    if !segmented {
        return bar
            .rounded_md()
            .overflow_hidden()
            .bg(track)
            .child(div().h_full().w(relative(fill)).bg(signal));
    }
    const CELLS: usize = 24;
    let lit = fill * CELLS as f32;
    bar.flex().gap(px(2.)).children((0..CELLS).map(|cell| {
        let fraction = (lit - cell as f32).clamp(0., 1.);
        div()
            .flex_1()
            .h_full()
            .rounded(px(1.))
            .overflow_hidden()
            .bg(track)
            .child(div().h_full().w(relative(fraction)).bg(signal))
    }))
}

impl Render for RecorderView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_5()
                    .py_2()
                    .bg(palette::panel_surface(cx))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_sm().font_semibold().child("Demolition"))
                    .child(
                        Button::new("app-settings")
                            .ghost()
                            .label(if self.show_settings {
                                "Close Settings"
                            } else {
                                "Settings…"
                            })
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.show_settings = !view.show_settings;
                                cx.notify();
                            })),
                    ),
            )
            .children(self.show_settings.then(|| self.render_settings(cx)))
            .child(div().flex_1().min_h_0().child(match self.screen {
                Screen::Welcome | Screen::NewProject => self.render_wizard(cx),
                Screen::Workspace => self.render_workspace(cx),
            }))
    }
}

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        palette::install_dark(cx);
        let preferences = appearance_path();
        let (appearance, settings_error) = match preferences.as_deref().map(load_appearance) {
            Some(Ok(appearance)) => (appearance, None),
            Some(Err(error)) => (
                Appearance::System,
                Some(format!(
                    "Couldn’t read the saved appearance. Following the system: {error}"
                )),
            ),
            None => (Appearance::System, None),
        };
        if let Some(value) = appearance.override_appearance() {
            cx.set_window_appearance(Some(value));
        }
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(760.), px(690.)), cx);
        if let Err(error) = gpui_kit::open_window(
            WindowOptions {
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some("Demolition".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| RecorderView::new(window, cx, appearance, preferences, settings_error))
            },
        ) {
            eprintln!("Couldn’t open Demolition: {error:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
