use super::RecorderView;
use demolition_audio_io::{ClickSettings, Meter, TEMPO_RANGE, TimingChange};
use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::Theme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    App, AppContext, Context, Div, Entity, ParentElement, SharedString, Styled, Window, div, px,
};

pub(super) struct TimingEditor {
    tempo: Entity<InputState>,
    meter: Entity<InputState>,
    level: Entity<InputState>,
    change_bar: Entity<InputState>,
    change_tempo: Entity<InputState>,
    change_meter: Entity<InputState>,
    accent: bool,
    changes: Vec<TimingChange>,
    editing_bar: Option<u32>,
    change_error: Option<&'static str>,
}

impl TimingEditor {
    pub(super) fn new(window: &mut Window, cx: &mut Context<RecorderView>) -> Self {
        let defaults = ClickSettings::default();
        let mut field = |value: String, placeholder| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value)
                    .placeholder(placeholder)
            });
            cx.subscribe(&input, |view, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    view.timing.change_error = None;
                    cx.notify();
                }
            })
            .detach();
            input
        };
        Self {
            tempo: field(defaults.bpm.to_string(), "120"),
            meter: field(meter_label(defaults.meter), "4/4"),
            level: field(defaults.level.to_string(), "70"),
            change_bar: field(String::new(), "e.g. 5"),
            change_tempo: field(String::new(), "Optional"),
            change_meter: field(String::new(), "Optional"),
            accent: defaults.accent,
            changes: Vec::new(),
            editing_bar: None,
            change_error: None,
        }
    }

    fn starting_values(&self, cx: &App) -> Result<(u16, Meter, u8), &'static str> {
        let bpm = parse_bpm(&self.tempo.read(cx).value())?;
        let meter = parse_meter(&self.meter.read(cx).value())?;
        let level = self
            .level
            .read(cx)
            .value()
            .trim()
            .parse::<u8>()
            .ok()
            .filter(|level| *level <= 100)
            .ok_or("Enter a click level between 0 and 100 percent.")?;
        Ok((bpm, meter, level))
    }

    pub(super) fn settings(&self, cx: &App) -> Result<ClickSettings, &'static str> {
        let (bpm, meter, level) = self.starting_values(cx)?;
        Ok(ClickSettings {
            bpm,
            meter,
            level,
            accent: self.accent,
            changes: self.changes.clone(),
        })
    }

    pub(super) fn summary(&self, cx: &App) -> Result<String, &'static str> {
        let (bpm, meter, _) = self.starting_values(cx)?;
        let change_count = self.changes.len();
        Ok(if change_count == 0 {
            format!("{bpm} BPM · {}", meter_label(meter))
        } else {
            format!(
                "{bpm} BPM · {} · {change_count} {}",
                meter_label(meter),
                if change_count == 1 {
                    "change"
                } else {
                    "changes"
                }
            )
        })
    }

    fn draft(&self, cx: &App) -> Result<TimingChange, &'static str> {
        let bar = self
            .change_bar
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|bar| *bar >= 2)
            .ok_or("Enter a bar number of 2 or later. Starting timing applies at bar 1.")?;
        let tempo_text = self.change_tempo.read(cx).value();
        let meter_text = self.change_meter.read(cx).value();
        let bpm = (!tempo_text.trim().is_empty())
            .then(|| parse_bpm(&tempo_text))
            .transpose()?;
        let meter = (!meter_text.trim().is_empty())
            .then(|| parse_meter(&meter_text))
            .transpose()?;
        if bpm.is_none() && meter.is_none() {
            return Err("Enter a new tempo, time signature, or both.");
        }
        Ok(TimingChange { bar, bpm, meter })
    }

    fn save_change(&mut self, window: &mut Window, cx: &mut Context<RecorderView>) {
        match self.draft(cx) {
            Ok(change) => {
                if self.editing_bar != Some(change.bar)
                    && self
                        .changes
                        .binary_search_by_key(&change.bar, |item| item.bar)
                        .is_ok()
                {
                    self.change_error = Some(
                        "That bar already has a change. Choose Edit on that row to update it.",
                    );
                } else {
                    self.changes
                        .retain(|item| Some(item.bar) != self.editing_bar);
                    let index = self.changes.partition_point(|item| item.bar < change.bar);
                    self.changes.insert(index, change);
                    self.fill_draft(None, window, cx);
                }
            }
            Err(error) => self.change_error = Some(error),
        }
        cx.notify();
    }

    fn fill_draft(
        &mut self,
        change: Option<TimingChange>,
        window: &mut Window,
        cx: &mut Context<RecorderView>,
    ) {
        self.change_error = None;
        self.editing_bar = change.map(|change| change.bar);
        // set_value focuses each field; finish on the bar field.
        self.change_meter.update(cx, |state, cx| {
            state.set_value(
                change
                    .and_then(|change| change.meter)
                    .map(meter_label)
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        self.change_tempo.update(cx, |state, cx| {
            state.set_value(
                change
                    .and_then(|change| change.bpm)
                    .map(|bpm| bpm.to_string())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        self.change_bar.update(cx, |state, cx| {
            state.set_value(
                change
                    .map(|change| change.bar.to_string())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        cx.notify();
    }

    pub(super) fn render(&self, busy: bool, cx: &mut Context<RecorderView>) -> Div {
        let theme = Theme::global(cx).colors;
        let field = |label, state, width| {
            div()
                .w(px(width))
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().child(label))
                .child(Input::new(state).aria_label(label).disabled(busy))
        };
        let updating = self.editing_bar.is_some();
        div().flex().flex_col().gap_3().p_4().bg(super::palette::panel_surface(cx)).border_1().border_color(theme.border).rounded_lg()
            .child(div().font_semibold().child("Metronome Settings"))
            .child(div().flex().flex_wrap().items_end().gap_3()
                .child(field("Starting BPM", &self.tempo, 108.))
                .child(field("Time signature", &self.meter, 108.))
                .child(field("Click level (%)", &self.level, 108.))
                .child(Button::new("accent-downbeat").label(if self.accent { "Accent beat 1: On" } else { "Accent beat 1: Off" }).disabled(busy)
                    .on_click(cx.listener(|view, _, _, cx| { view.timing.accent = !view.timing.accent; cx.notify(); }))))
            .child(div().text_sm().text_color(theme.muted_foreground).child("BPM counts quarter notes. The click follows the time signature: 6/8 gives six eighth-note clicks per bar."))
            .child(div().font_semibold().child("Timing Changes"))
            .child(div().text_sm().text_color(theme.muted_foreground)
                .child("Changes take effect at the start of the chosen bar. Leave BPM or time signature blank to keep it unchanged."))
            .child(div().flex().flex_wrap().items_end().gap_3()
                .child(field("Bar number", &self.change_bar, 108.))
                .child(field("New BPM", &self.change_tempo, 108.))
                .child(field("New time signature", &self.change_meter, 132.))
                .child(Button::new("save-timing-change").secondary().label(if updating { "Update Change" } else { "Add Change" }).disabled(busy)
                    .on_click(cx.listener(|view, _, window, cx| view.timing.save_change(window, cx))))
                .children(updating.then(|| Button::new("cancel-timing-change").ghost().label("Cancel").disabled(busy)
                    .on_click(cx.listener(|view, _, window, cx| view.timing.fill_draft(None, window, cx))))))
            .children(self.change_error.map(|error| div().text_sm().text_color(theme.danger).child(error)))
            .children(self.changes.iter().copied().map(|change| {
                let tempo = change.bpm.map_or_else(|| "Keep tempo".into(), |bpm| format!("{bpm} BPM"));
                let meter = change.meter.map_or_else(|| "Keep time signature".into(), meter_label);
                div().flex().items_center().justify_between().gap_2().py_1().border_t_1().border_color(theme.border)
                    .child(div().text_sm().child(format!("Bar {} · {tempo} · {meter}", change.bar)))
                    .child(div().flex().gap_2()
                        .child(Button::new(SharedString::from(format!("edit-timing-{}", change.bar))).ghost().label("Edit").disabled(busy)
                            .on_click(cx.listener(move |view, _, window, cx| view.timing.fill_draft(Some(change), window, cx))))
                        .child(Button::new(SharedString::from(format!("remove-timing-{}", change.bar))).ghost().label("Remove").disabled(busy)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.timing.changes.retain(|item| item.bar != change.bar);
                                if view.timing.editing_bar == Some(change.bar) {
                                    view.timing.fill_draft(None, window, cx);
                                }
                                cx.notify();
                            }))))
            }))
    }
}

fn meter_label(meter: Meter) -> String {
    format!("{}/{}", meter.beats, meter.unit)
}

fn parse_bpm(text: &str) -> Result<u16, &'static str> {
    text.trim()
        .parse()
        .ok()
        .filter(|bpm| TEMPO_RANGE.contains(bpm))
        .ok_or("Enter a whole-number tempo between 30 and 300 BPM.")
}

fn parse_meter(text: &str) -> Result<Meter, &'static str> {
    let error = "Enter a time signature like 4/4 or 7/8: 1–32 beats over 2, 4, 8, or 16.";
    let (beats, unit) = text.split_once('/').ok_or(error)?;
    let beats = beats.trim().parse::<u8>().map_err(|_| error)?;
    let unit = unit.trim().parse::<u8>().map_err(|_| error)?;
    if !(1..=32).contains(&beats) || !matches!(unit, 2 | 4 | 8 | 16) {
        return Err(error);
    }
    Ok(Meter { beats, unit })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meter_and_bpm_from_editor_fields() {
        assert_eq!(parse_meter(" 7 / 8 "), Ok(Meter { beats: 7, unit: 8 }));
        for invalid in ["4", "4/3", "0/4", "33/4", "4/4/4"] {
            assert!(parse_meter(invalid).is_err(), "{invalid}");
        }
        assert_eq!(parse_bpm(" 137 "), Ok(137));
        assert!(parse_bpm("301").is_err());
    }
}
