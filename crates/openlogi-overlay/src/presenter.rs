//! Full-display presenter pointer effects.

use std::sync::Arc;

use chrono::Local;
use gpui::{
    Bounds, BoxShadow, Context, FillOptions, FillRule, Hsla, InteractiveElement, IntoElement,
    ParentElement, PathBuilder, PathStyle, Render, Size, Styled, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, canvas, div, img, point,
    prelude::FluentBuilder as _, px, rgba,
};
use openlogi_core::hid::PresenterEffect;
use openlogi_ipc::PresenterOverlay;

use crate::platform;

const DIM: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.02,
    a: 0.62,
};

#[cfg(target_os = "macos")]
use gpui::RenderImage;
#[cfg(target_os = "macos")]
use image::{Frame as ImageFrame, RgbaImage};
#[cfg(target_os = "macos")]
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct CaptureRequest {
    cursor: (f64, f64),
    lens_radius: f32,
    magnification: u16,
}

#[cfg(target_os = "macos")]
struct CapturedFrame {
    image: Arc<RenderImage>,
    cursor: (f64, f64),
}

/// Transparent full-display view for one physical display.
pub(crate) struct PresenterView {
    overlay: PresenterOverlay,
    display: platform::CursorDisplay,
    #[cfg(target_os = "macos")]
    hidden_cursor_display: Option<u32>,
    #[cfg(target_os = "macos")]
    magnifier_frame: Option<CapturedFrame>,
    #[cfg(target_os = "macos")]
    capture_requests: SyncSender<CaptureRequest>,
    #[cfg(target_os = "macos")]
    captured_frames: Receiver<CapturedFrame>,
}

impl PresenterView {
    pub(crate) fn new(overlay: PresenterOverlay, display: platform::CursorDisplay) -> Self {
        #[cfg(target_os = "macos")]
        let (capture_requests, captured_frames) = spawn_capture_worker();
        #[cfg(target_os = "macos")]
        if matches!(overlay.effect, PresenterEffect::Magnify) {
            platform::request_screen_capture_access();
        }
        Self {
            overlay,
            display,
            #[cfg(target_os = "macos")]
            hidden_cursor_display: None,
            #[cfg(target_os = "macos")]
            magnifier_frame: None,
            #[cfg(target_os = "macos")]
            capture_requests,
            #[cfg(target_os = "macos")]
            captured_frames,
        }
    }

    pub(crate) fn update(&mut self, overlay: PresenterOverlay, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if !matches!(self.overlay.effect, PresenterEffect::Magnify)
            && matches!(overlay.effect, PresenterEffect::Magnify)
        {
            platform::request_screen_capture_access();
        }
        self.overlay = overlay;
        cx.notify();
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let (origin_x, origin_y) = self.display.origin;
        let (width, height) = self.display.size;
        x >= origin_x && x < origin_x + width && y >= origin_y && y < origin_y + height
    }

    #[cfg(target_os = "macos")]
    fn sync_cursor_visibility(&mut self, cursor: Option<&openlogi_hook::CursorPosition>) {
        let cursor_here = cursor.is_some_and(|position| self.contains(position.x, position.y));
        if self.overlay.cursor_control || !cursor_here {
            platform::show_cursor(self.hidden_cursor_display.take());
        } else if self.hidden_cursor_display.is_none()
            && let Some(position) = cursor
        {
            self.hidden_cursor_display = platform::hide_cursor_at(position.x, position.y);
        }
    }

    #[cfg(target_os = "macos")]
    fn update_magnifier(&mut self, cursor: Option<&openlogi_hook::CursorPosition>, radius: f32) {
        while let Ok(frame) = self.captured_frames.try_recv() {
            self.magnifier_frame = Some(frame);
        }
        if !matches!(self.overlay.effect, PresenterEffect::Magnify) {
            self.magnifier_frame = None;
            return;
        }
        let Some(cursor) = cursor.filter(|position| self.contains(position.x, position.y)) else {
            return;
        };
        let request = CaptureRequest {
            cursor: (cursor.x, cursor.y),
            lens_radius: radius,
            magnification: self.overlay.magnification,
        };
        match self.capture_requests.try_send(request) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => self.magnifier_frame = None,
        }
    }
}

impl Drop for PresenterView {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        platform::show_cursor(self.hidden_cursor_display.take());
    }
}

impl Render for PresenterView {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "native display coordinates and dimensions fit GPUI pixels"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "declarative GPUI composition for three closely related presenter effects"
    )]
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let cursor = self
            .overlay
            .frozen_position
            .map(|(x, y)| openlogi_hook::CursorPosition {
                x: f64::from(x),
                y: f64::from(y),
            })
            .or_else(openlogi_hook::cursor_position);
        let cursor_here = cursor
            .as_ref()
            .is_some_and(|position| self.contains(position.x, position.y));

        #[cfg(target_os = "macos")]
        self.sync_cursor_visibility(cursor.as_ref());

        let (x, y) = cursor.as_ref().map_or_else(
            || {
                (
                    f32::from(viewport.width) / 2.0,
                    f32::from(viewport.height) / 2.0,
                )
            },
            |position| {
                (
                    (position.x - self.display.origin.0) as f32,
                    (position.y - self.display.origin.1) as f32,
                )
            },
        );
        let base_radius = match self.overlay.effect {
            PresenterEffect::DigitalLaser => 7.0,
            PresenterEffect::Highlight => f32::from(self.overlay.spotlight_radius),
            PresenterEffect::Magnify => f32::from(self.overlay.magnifier_radius),
        };
        let radius = if matches!(self.overlay.effect, PresenterEffect::DigitalLaser) {
            base_radius * f32::from(self.overlay.effect_size) / 100.0
        } else {
            base_radius
        };

        #[cfg(target_os = "macos")]
        self.update_magnifier(cursor.as_ref(), radius);

        let contrast = f32::from(self.overlay.effect_contrast) / 100.0;
        let dim = Hsla {
            a: DIM.a * (0.65 + contrast * 0.35),
            ..DIM
        };
        let laser = rgba((self.overlay.effect_color.packed() << 8) | 0xff);
        let magnifier = rgba((self.overlay.magnifier_color.packed() << 8) | 0xff);
        let laser_hsla: Hsla = laser.into();
        let magnifier_hsla: Hsla = magnifier.into();
        let timer = if self.overlay.timer_current_time {
            Some(Local::now().format("%H:%M:%S").to_string())
        } else {
            self.overlay.timer_remaining_ms.map(|millis| {
                let seconds = millis.div_ceil(1000);
                let minutes = seconds / 60;
                let seconds = seconds % 60;
                format!("{minutes:02}:{seconds:02}")
            })
        };

        let root = div().id("presenter-overlay").size_full();
        if !cursor_here {
            return root;
        }

        let root = root.when(
            matches!(self.overlay.effect, PresenterEffect::Highlight),
            |root| root.child(highlight_mask(x, y, radius, dim)),
        );

        #[cfg(target_os = "macos")]
        let root = if matches!(self.overlay.effect, PresenterEffect::Magnify) {
            // Keep the lens and the pixels it contains in the same capture
            // sample. Moving a stale frame to the newest cursor position is
            // what produced the bright tearing seen during fast motion.
            let (lens_x, lens_y) = self.magnifier_frame.as_ref().map_or((x, y), |frame| {
                (
                    (frame.cursor.0 - self.display.origin.0) as f32,
                    (frame.cursor.1 - self.display.origin.1) as f32,
                )
            });
            let lens = div()
                .absolute()
                .left(px(lens_x - radius))
                .top(px(lens_y - radius))
                .size(px(radius * 2.0))
                .rounded_full()
                .overflow_hidden()
                .border_2()
                .border_color(magnifier)
                .shadow(vec![BoxShadow {
                    color: Hsla {
                        a: 0.38,
                        ..magnifier_hsla
                    },
                    offset: point(px(0.0), px(2.0)),
                    blur_radius: px(12.0),
                    spread_radius: px(1.0),
                    inset: false,
                }])
                .when_some(self.magnifier_frame.as_ref(), |lens, frame| {
                    lens.child(img(frame.image.clone()).size_full())
                });
            root.child(lens)
        } else {
            root
        };

        let root = root.when(
            matches!(self.overlay.effect, PresenterEffect::DigitalLaser),
            |root| {
                root.child(
                    div()
                        .absolute()
                        .left(px(x - radius))
                        .top(px(y - radius))
                        .size(px(radius * 2.0))
                        .rounded_full()
                        .bg(laser)
                        .shadow(vec![
                            BoxShadow {
                                color: Hsla {
                                    a: 0.78,
                                    ..laser_hsla
                                },
                                offset: point(px(0.0), px(0.0)),
                                blur_radius: px(radius * 1.8),
                                spread_radius: px(radius * 0.75),
                                inset: false,
                            },
                            BoxShadow {
                                color: Hsla {
                                    a: 0.34,
                                    ..laser_hsla
                                },
                                offset: point(px(0.0), px(0.0)),
                                blur_radius: px(radius * 3.5),
                                spread_radius: px(radius * 1.4),
                                inset: false,
                            },
                        ]),
                )
            },
        );

        root.when_some(timer, |root, timer| {
            root.child(
                div()
                    .absolute()
                    .left(px((x + radius + 12.0).min(f32::from(viewport.width) - 76.0)))
                    .top(px((y - 18.0).max(8.0)))
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .bg(rgba(0x1414_14d9))
                    .text_lg()
                    .text_color(gpui::white())
                    .child(timer),
            )
        })
    }
}

fn highlight_mask(x: f32, y: f32, radius: f32, color: Hsla) -> impl IntoElement {
    const SEGMENTS: u16 = 160;
    canvas(
        move |bounds, _, _| (bounds, x, y, radius),
        move |_bounds, (bounds, x, y, radius), window, _| {
            let mut builder = PathBuilder::fill().with_style(PathStyle::Fill(
                FillOptions::default().with_fill_rule(FillRule::EvenOdd),
            ));
            let origin = bounds.origin;
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let outer = [
                origin,
                origin + point(px(width), px(0.0)),
                origin + point(px(width), px(height)),
                origin + point(px(0.0), px(height)),
            ];
            builder.add_polygon(&outer, true);

            let circle = (0..SEGMENTS)
                .map(|index| {
                    let angle = std::f32::consts::TAU * f32::from(index) / f32::from(SEGMENTS);
                    origin + point(px(x + radius * angle.cos()), px(y + radius * angle.sin()))
                })
                .collect::<Vec<_>>();
            builder.add_polygon(&circle, true);
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .inset_0()
}

#[cfg(target_os = "macos")]
fn spawn_capture_worker() -> (SyncSender<CaptureRequest>, Receiver<CapturedFrame>) {
    let (request_tx, request_rx) = std::sync::mpsc::sync_channel::<CaptureRequest>(1);
    let (frame_tx, frame_rx) = std::sync::mpsc::channel();
    let result = std::thread::Builder::new()
        .name("openlogi-magnifier-capture".into())
        .spawn(move || {
            while let Ok(mut request) = request_rx.recv() {
                while let Ok(newer) = request_rx.try_recv() {
                    request = newer;
                }
                if let Some(frame) = capture_magnifier(request)
                    && frame_tx.send(frame).is_err()
                {
                    break;
                }
            }
        });
    if let Err(error) = result {
        tracing::warn!(%error, "could not start magnifier capture worker");
    }
    (request_tx, frame_rx)
}

#[cfg(target_os = "macos")]
fn capture_magnifier(request: CaptureRequest) -> Option<CapturedFrame> {
    if !platform::has_screen_capture_access() {
        return None;
    }
    let display = platform::display_containing(request.cursor.0, request.cursor.1)?;
    let magnification = f64::from(request.magnification.clamp(100, 500)) / 100.0;
    let sample = f64::from(request.lens_radius) * 2.0 / magnification;
    let min_x = display.origin.0;
    let min_y = display.origin.1;
    let max_x = (display.origin.0 + display.size.0 - sample).max(min_x);
    let max_y = (display.origin.1 + display.size.1 - sample).max(min_y);
    let x = (request.cursor.0 - sample / 2.0).clamp(min_x, max_x);
    let y = (request.cursor.1 - sample / 2.0).clamp(min_y, max_y);
    let image = core_graphics::display::CGDisplay::screenshot(
        core_graphics::geometry::CGRect::new(
            &core_graphics::geometry::CGPoint::new(x, y),
            &core_graphics::geometry::CGSize::new(sample, sample),
        ),
        core_graphics::window::kCGWindowListOptionOnScreenOnly,
        core_graphics::window::kCGNullWindowID,
        core_graphics::window::kCGWindowImageBestResolution,
    )?;
    let width = image.width();
    let height = image.height();
    let width_u32 = u32::try_from(width).ok()?;
    let height_u32 = u32::try_from(height).ok()?;
    let row_stride = image.bytes_per_row();
    let source = image.data();
    let bytes = source.bytes();
    if image.bits_per_pixel() != 32 || bytes.len() < row_stride.saturating_mul(height) {
        return None;
    }

    // Strip Quartz row padding, convert BGRA to the RGBA layout expected by
    // `image::RgbaImage`, and supply a hard circular alpha mask. No PNG codec
    // or UI-thread work sits between the compositor frame and the GPU texture.
    let mut rgba = vec![0; width.saturating_mul(height).saturating_mul(4)];
    let center_x = f64::from(width_u32) / 2.0;
    let center_y = f64::from(height_u32) / 2.0;
    let radius = f64::from(width_u32.min(height_u32)) / 2.0;
    for (row, row_u32) in (0..height).zip(0..height_u32) {
        let source_row = &bytes[row * row_stride..row * row_stride + width * 4];
        let target_row = &mut rgba[row * width * 4..(row + 1) * width * 4];
        for (column, column_u32) in (0..width).zip(0..width_u32) {
            let source = &source_row[column * 4..column * 4 + 4];
            let target = &mut target_row[column * 4..column * 4 + 4];
            target[0] = source[2];
            target[1] = source[1];
            target[2] = source[0];
            let dx = f64::from(column_u32) + 0.5 - center_x;
            let dy = f64::from(row_u32) + 0.5 - center_y;
            target[3] = u8::from(dx.mul_add(dx, dy * dy) <= radius * radius) * u8::MAX;
        }
    }
    let buffer = RgbaImage::from_raw(width_u32, height_u32, rgba)?;
    Some(CapturedFrame {
        image: Arc::new(RenderImage::new(vec![ImageFrame::new(buffer)])),
        cursor: request.cursor,
    })
}

/// Full-display transparent popups, one for each active display.
#[expect(
    clippy::cast_possible_truncation,
    reason = "native display dimensions fit GPUI pixels"
)]
pub(crate) fn presenter_window_options(
    cx: &mut gpui::App,
) -> Vec<(WindowOptions, platform::CursorDisplay)> {
    let mut displays = platform::active_displays();
    if displays.is_empty() {
        displays = cx
            .displays()
            .into_iter()
            .map(|display| {
                let bounds = display.bounds();
                platform::CursorDisplay {
                    id: u64::from(display.id()),
                    origin: (f64::from(bounds.origin.x), f64::from(bounds.origin.y)),
                    size: (f64::from(bounds.size.width), f64::from(bounds.size.height)),
                }
            })
            .collect();
    }
    displays
        .into_iter()
        .map(|display| {
            let bounds = Bounds::new(
                point(px(0.0), px(0.0)),
                Size::new(px(display.size.0 as f32), px(display.size.1 as f32)),
            );
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: false,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                display_id: Some(gpui::DisplayId::from(display.id)),
                window_background: WindowBackgroundAppearance::Transparent,
                app_id: Some(format!("openlogi-presenter-overlay-{}", display.id)),
                ..WindowOptions::default()
            };
            (options, display)
        })
        .collect()
}
