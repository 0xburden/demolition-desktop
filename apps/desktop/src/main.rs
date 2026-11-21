#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use demolition_audio_io::{DeviceInventory, DiscoveryError, discover_devices};
use gpui_kit::base::Disableable;
use gpui_kit::component::Theme;
use gpui_kit::component::button::Button;
use gpui_kit::{
    App, AppContext, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, size,
};

struct AudioSetup {
    devices: Option<Result<DeviceInventory, DiscoveryError>>,
}

impl AudioSetup {
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.devices = None;
        cx.notify();
        let scan = cx.background_spawn(async { discover_devices() });
        cx.spawn(async move |view, cx| {
            let result = scan.await;
            let _ = view.update(cx, |setup, cx| {
                setup.devices = Some(result);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for AudioSetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &Theme::global(cx).colors;
        let contents = match &self.devices {
            None => div().child("Looking for audio devices…"),
            Some(Err(error)) => div()
                .flex()
                .flex_col()
                .gap_2()
                .child("Couldn’t look up audio devices. Check your connection and refresh.")
                .child(error.to_string()),
            Some(Ok(inventory)) => {
                let mut list = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(format!("Audio system: {}", inventory.backend));
                if inventory.devices.is_empty() {
                    list =
                        list.child("No devices found. Connect your audio interface and refresh.");
                }
                list = list.children(inventory.devices.iter().map(|device| {
                    let direction = match (device.input, device.output) {
                        (true, true) => "Input and output",
                        (true, false) => "Input",
                        (false, true) => "Output",
                        (false, false) => "Direction unknown",
                    };
                    div()
                        .flex()
                        .justify_between()
                        .gap_4()
                        .p_3()
                        .rounded_md()
                        .bg(theme.muted)
                        .child(device.name.clone())
                        .child(direction)
                }));
                if inventory.unavailable > 0 {
                    list = list.child("Some devices couldn’t be read. Try refreshing.");
                }
                list
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_xl().child("Audio setup"))
                    .child(
                        Button::new("refresh-devices")
                            .label("Refresh devices")
                            .disabled(self.devices.is_none())
                            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                    ),
            )
            .child(
                div()
                    .id("devices")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(contents),
            )
    }
}

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(680.), px(480.)), cx);
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
            |_, cx| {
                cx.new(|cx| {
                    let mut setup = AudioSetup { devices: None };
                    setup.refresh(cx);
                    setup
                })
            },
        ) {
            eprintln!("Couldn’t open Demolition: {error:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
