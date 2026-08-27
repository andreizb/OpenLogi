//! Logitech Spotlight configuration surface.
//!
//! The presenter deserves its own interaction model: three physical buttons,
//! ordered pointer effects, timer and haptic settings, plus per-application
//! profiles. Keeping this as a purpose-built view also prevents internal capture
//! actions from leaking into the generic mouse-button editor.

use std::collections::BTreeMap;

use gpui::{
    Anchor, AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement, Render, StatefulInteractiveElement as _, Styled, Subscription,
    Window, div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::{
    Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    popover::Popover,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};
use openlogi_core::binding::{Action, ButtonId};
use openlogi_core::color::Rgb;
use openlogi_core::hid::{PointerSpeed, PresenterEffect, PresenterSettings, PresenterTimerMode};

use crate::features::mouse::picker::presenter_action_picker;
use crate::state::{AppState, DeviceKey, DeviceRecord, Load};
use crate::ui::theme::{self, Palette, Typography as _};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PresenterSection {
    #[default]
    Buttons,
    Timer,
    Vibration,
}

#[derive(Clone, Copy, Debug)]
enum SettingsSliderKind {
    HighlightContrast,
    HighlightRadius,
    MagnifierRadius,
    Magnification,
    LaserSize,
    VibrationIntensity,
}

struct SettingsSliders {
    highlight_contrast: Entity<SliderState>,
    highlight_radius: Entity<SliderState>,
    magnifier_radius: Entity<SliderState>,
    magnification: Entity<SliderState>,
    laser_size: Entity<SliderState>,
    vibration_intensity: Entity<SliderState>,
}

impl SettingsSliders {
    fn get(&self, kind: SettingsSliderKind) -> &Entity<SliderState> {
        match kind {
            SettingsSliderKind::HighlightContrast => &self.highlight_contrast,
            SettingsSliderKind::HighlightRadius => &self.highlight_radius,
            SettingsSliderKind::MagnifierRadius => &self.magnifier_radius,
            SettingsSliderKind::Magnification => &self.magnification,
            SettingsSliderKind::LaserSize => &self.laser_size,
            SettingsSliderKind::VibrationIntensity => &self.vibration_intensity,
        }
    }
}

/// Complete Spotlight editor for one selected presenter.
pub struct PresenterControlsView {
    #[expect(dead_code, reason = "held to keep the AppState observer alive")]
    state_obs: Subscription,
    section: PresenterSection,
    speed_slider: Option<Entity<SliderState>>,
    speed_sub: Option<Subscription>,
    speed_key: Option<DeviceKey>,
    settings_key: Option<(DeviceKey, Option<String>)>,
    settings_sliders: Option<SettingsSliders>,
    settings_subs: Vec<Subscription>,
    color_input: Option<Entity<InputState>>,
    timer_input: Option<Entity<InputState>>,
}

impl PresenterControlsView {
    /// Construct a Spotlight editor that follows inventory and config changes.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let state_obs = cx.subscribe(&state, |_, _, event, cx| {
            if matches!(
                event,
                crate::state::StateEvent::InventoryChanged
                    | crate::state::StateEvent::DeviceSelected(_)
                    | crate::state::StateEvent::ForegroundChanged
                    | crate::state::StateEvent::BindingsChanged(_)
                    | crate::state::StateEvent::PresenterChanged(_)
            ) {
                cx.notify();
            }
        });
        Self {
            state_obs,
            section: PresenterSection::Buttons,
            speed_slider: None,
            speed_sub: None,
            speed_key: None,
            settings_key: None,
            settings_sliders: None,
            settings_subs: Vec::new(),
            color_input: None,
            timer_input: None,
        }
    }

    fn ensure_speed_slider(
        &mut self,
        key: &DeviceKey,
        speed: PointerSpeed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.speed_key.as_ref() == Some(key) {
            if let Some(slider) = &self.speed_slider {
                let target = f32::from(speed.get());
                slider.update(cx, |state, cx| {
                    if (state.value().start() - target).abs() > f32::EPSILON {
                        state.set_value(target, window, cx);
                    }
                });
            }
            return;
        }
        let slider = cx.new(|_| {
            SliderState::new()
                .max(f32::from(PointerSpeed::MAX.get()))
                .min(f32::from(PointerSpeed::MIN.get()))
                .step(1.)
                .default_value(f32::from(speed.get()))
        });
        let sub = cx.subscribe(&slider, |_panel, _slider, event: &SliderEvent, cx| {
            if let SliderEvent::Release(value) = event
                && let Some(speed) = speed_from_slider(value.start())
            {
                AppState::update(cx, |state, cx| {
                    let key = state.current_record().map(DeviceRecord::device_key);
                    state.commit_pointer_speed(speed);
                    if let Some(key) = key {
                        cx.emit(crate::state::StateEvent::PresenterChanged(key));
                    }
                });
            }
            cx.notify();
        });
        self.speed_slider = Some(slider);
        self.speed_sub = Some(sub);
        self.speed_key = Some(key.clone());
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "slider values are rounded and clamped to each persisted integer range"
    )]
    fn create_settings_slider(
        kind: SettingsSliderKind,
        profile: Option<String>,
        min: f32,
        max: f32,
        step: f32,
        value: f32,
        cx: &mut Context<Self>,
    ) -> (Entity<SliderState>, Subscription) {
        let slider = cx.new(|_| {
            SliderState::new()
                .max(max)
                .min(min)
                .step(step)
                .default_value(value)
        });
        let sub = cx.subscribe(&slider, move |_panel, _slider, event: &SliderEvent, cx| {
            let SliderEvent::Release(value) = event else {
                return;
            };
            let value = value.start().round();
            AppState::update(cx, |state, cx| {
                let mut settings = state.presenter_settings_for(profile.as_deref());
                match kind {
                    SettingsSliderKind::HighlightContrast => {
                        settings.effect_contrast = value.clamp(10., 100.) as u8;
                    }
                    SettingsSliderKind::HighlightRadius => {
                        settings.spotlight_radius = value.clamp(50., 320.) as u16;
                    }
                    SettingsSliderKind::MagnifierRadius => {
                        settings.magnifier_radius = value.clamp(50., 240.) as u16;
                    }
                    SettingsSliderKind::Magnification => {
                        settings.magnification = value.clamp(125., 500.) as u16;
                    }
                    SettingsSliderKind::LaserSize => {
                        settings.effect_size = value.clamp(50., 200.) as u8;
                    }
                    SettingsSliderKind::VibrationIntensity => {
                        settings.vibration_intensity = value.clamp(0., 100.) as u8;
                    }
                }
                state.commit_presenter_settings_for(profile.as_deref(), settings);
                if let Some(key) = state.current_record().map(DeviceRecord::device_key) {
                    cx.emit(crate::state::StateEvent::PresenterChanged(key));
                }
            });
            cx.notify();
        });
        (slider, sub)
    }

    fn ensure_settings_sliders(
        &mut self,
        key: DeviceKey,
        profile: Option<String>,
        settings: PresenterSettings,
        cx: &mut Context<Self>,
    ) {
        let settings_key = (key, profile);
        if self.settings_key.as_ref() == Some(&settings_key) {
            return;
        }
        let specs = [
            (
                SettingsSliderKind::HighlightContrast,
                10.,
                100.,
                1.,
                f32::from(settings.effect_contrast),
            ),
            (
                SettingsSliderKind::HighlightRadius,
                50.,
                320.,
                1.,
                f32::from(settings.spotlight_radius),
            ),
            (
                SettingsSliderKind::MagnifierRadius,
                50.,
                240.,
                1.,
                f32::from(settings.magnifier_radius),
            ),
            (
                SettingsSliderKind::Magnification,
                125.,
                500.,
                5.,
                f32::from(settings.normalized_magnification()),
            ),
            (
                SettingsSliderKind::LaserSize,
                50.,
                200.,
                1.,
                f32::from(settings.effect_size),
            ),
            (
                SettingsSliderKind::VibrationIntensity,
                0.,
                100.,
                1.,
                f32::from(settings.vibration_intensity),
            ),
        ];
        let mut entities = Vec::new();
        let mut subscriptions = Vec::new();
        for (kind, min, max, step, value) in specs {
            let (entity, subscription) = Self::create_settings_slider(
                kind,
                settings_key.1.clone(),
                min,
                max,
                step,
                value,
                cx,
            );
            entities.push(entity);
            subscriptions.push(subscription);
        }
        let Ok(
            [
                highlight_contrast,
                highlight_radius,
                magnifier_radius,
                magnification,
                laser_size,
                vibration_intensity,
            ],
        ) = <[Entity<SliderState>; 6]>::try_from(entities)
        else {
            return;
        };
        self.settings_sliders = Some(SettingsSliders {
            highlight_contrast,
            highlight_radius,
            magnifier_radius,
            magnification,
            laser_size,
            vibration_intensity,
        });
        self.settings_subs = subscriptions;
        self.settings_key = Some(settings_key);
    }

    fn ensure_color_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        self.color_input
            .get_or_insert_with(|| cx.new(|cx| InputState::new(window, cx).placeholder("#RRGGBB")))
            .clone()
    }

    fn ensure_timer_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        self.timer_input
            .get_or_insert_with(|| cx.new(|cx| InputState::new(window, cx).placeholder("minutes")))
            .clone()
    }
}

impl Render for PresenterControlsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        AppState::load_current_device_reads(cx);
        let profile = AppState::try_read(cx)
            .and_then(AppState::editing_app)
            .map(str::to_string);
        let settings = AppState::try_read(cx)
            .map_or_else(PresenterSettings::default, AppState::presenter_settings);
        let bindings = AppState::try_read(cx).map_or_else(BTreeMap::new, |state| {
            state.presenter_bindings_for_profile(profile.as_deref())
        });
        if let Some(record) = AppState::try_read(cx).and_then(AppState::current_record) {
            let key = record.device_key();
            self.ensure_settings_sliders(key, profile.clone(), settings, cx);
        }
        let color_input = self.ensure_color_input(window, cx);
        let timer_input = self.ensure_timer_input(window, cx);
        let pal = theme::palette(cx);
        let view = cx.entity();

        let speed_panel = match (
            AppState::try_read(cx).map_or(Load::Unknown, AppState::pointer_speed_status),
            AppState::try_read(cx)
                .and_then(AppState::current_record)
                .map(DeviceRecord::device_key),
        ) {
            (Load::Ready(speed), Some(key)) => {
                self.ensure_speed_slider(&key, *speed, window, cx);
                speed_control(self.speed_slider.as_ref(), *speed, pal).into_any_element()
            }
            (Load::Failed(error), Some(key)) => pointer_speed_failure(error, key, &view, pal),
            (Load::Unsupported(error), _) => muted(error, pal),
            (Load::Loading, _) => muted("Reading pointer speed…", pal),
            _ => muted("Pointer speed unavailable", pal),
        };

        let inspector = match self.section {
            PresenterSection::Buttons => buttons_inspector(
                settings,
                &bindings,
                profile.clone(),
                self.settings_sliders.as_ref(),
                speed_panel,
                &color_input,
                &view,
                pal,
            ),
            PresenterSection::Timer => {
                timer_inspector(settings, profile.clone(), &timer_input, &view, pal)
            }
            PresenterSection::Vibration => vibration_inspector(
                settings,
                profile.clone(),
                self.settings_sliders.as_ref(),
                &view,
                pal,
            ),
        };

        v_flex().w_full().gap_3().child(
            h_flex()
                .w_full()
                .items_start()
                .flex_wrap()
                .gap_3()
                .child(section_nav(self.section, &view, pal))
                .child(div().flex_1().min_w(px(250.)).child(presenter_stage(
                    self.section,
                    settings,
                    &bindings,
                    pal,
                )))
                .child(
                    v_flex()
                        .w(px(410.))
                        .flex_shrink_0()
                        .rounded(pal.card_radius)
                        .border_1()
                        .border_color(pal.border)
                        .bg(pal.panel)
                        .child(inspector),
                ),
        )
    }
}

fn section_nav(
    selected: PresenterSection,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> impl IntoElement {
    v_flex()
        .w(px(126.))
        .flex_shrink_0()
        .gap_1()
        .child(nav_button(
            0,
            "Buttons",
            IconName::Settings,
            PresenterSection::Buttons,
            selected,
            view,
        ))
        .child(nav_button(
            1,
            "Timer",
            IconName::Calendar,
            PresenterSection::Timer,
            selected,
            view,
        ))
        .child(nav_button(
            2,
            "Vibration",
            IconName::Bell,
            PresenterSection::Vibration,
            selected,
            view,
        ))
        .child(
            div()
                .mt_3()
                .text_caption()
                .text_color(pal.text_muted)
                .child("Spotlight 1"),
        )
}

fn nav_button(
    index: usize,
    label: &'static str,
    icon: IconName,
    section: PresenterSection,
    selected: PresenterSection,
    view: &Entity<PresenterControlsView>,
) -> impl IntoElement {
    let view = view.clone();
    Button::new(("presenter-nav", index))
        .w_full()
        .label(label)
        .icon(icon)
        .when(section == selected, ButtonVariants::primary)
        .on_click(move |_, _, cx| {
            view.update(cx, |view, cx| {
                view.section = section;
                cx.notify();
            });
        })
}

fn presenter_stage(
    section: PresenterSection,
    settings: PresenterSettings,
    bindings: &std::collections::BTreeMap<ButtonId, Action>,
    pal: Palette,
) -> impl IntoElement {
    let pointer_label = match section {
        PresenterSection::Buttons => {
            format!("{} pointer effects", settings.enabled_effects.count_ones())
        }
        PresenterSection::Timer => presenter_timer_label(settings),
        PresenterSection::Vibration => format!("Intensity {}%", settings.vibration_intensity),
    };
    let next = bindings
        .get(&ButtonId::PresenterNextHold)
        .map_or_else(|| "Start Presentation".to_string(), Action::label);
    let back = bindings
        .get(&ButtonId::PresenterBackHold)
        .map_or_else(|| "Blank Screen".to_string(), Action::label);

    v_flex()
        .w_full()
        .items_center()
        .gap_4()
        .py_4()
        .child(div().text_heading().text_color(pal.text_primary).child("Spotlight"))
        .child(
            h_flex()
                .items_center()
                .gap_3()
                .child(
                    v_flex()
                        .items_end()
                        .gap_8()
                        .w(px(150.))
                        .child(callout("Pointer", &pointer_label, pal))
                        .child(callout("Hold Back", &back, pal)),
                )
                .child(
                    v_flex()
                        .items_center()
                        .gap_3()
                        .w(px(112.))
                        .h(px(390.))
                        .py_5()
                        .rounded(px(48.))
                        .border_1()
                        .border_color(pal.border)
                        .bg(pal.control_hover)
                        .shadow_md()
                        .child(device_button(40., "•", pal))
                        .child(device_button(68., "›", pal))
                        .child(device_button(42., "‹", pal))
                        .child(div().flex_1())
                        .child(div().w(px(32.)).h(px(3.)).rounded_full().bg(pal.border)),
                )
                .child(
                    v_flex()
                        .w(px(150.))
                        .pt(px(128.))
                        .child(callout("Hold Next", &next, pal)),
                ),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_caption()
                .text_color(pal.text_muted)
                .child(match section {
                    PresenterSection::Buttons => "Hold Pointer to aim. Double-click cycles effects. While Pointer is held, Next / Back cycles forward / backward.",
                    PresenterSection::Timer => "The timer starts on the first slide navigation and remains visible while presenting.",
                    PresenterSection::Vibration => "Spotlight vibrates for timer and low-battery alerts without interrupting the slide show.",
                }),
        )
}

fn callout(title: &str, value: &str, pal: Palette) -> impl IntoElement {
    v_flex()
        .w_full()
        .rounded(pal.control_radius)
        .border_1()
        .border_color(pal.border)
        .bg(pal.panel)
        .px_3()
        .py_2()
        .child(
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(title.to_string()),
        )
        .child(div().text_body().child(value.to_string()))
}

fn device_button(size: f32, label: &'static str, pal: Palette) -> impl IntoElement {
    div()
        .w(px(size))
        .h(px(size))
        .rounded_full()
        .border_1()
        .border_color(pal.border)
        .bg(pal.panel)
        .flex()
        .items_center()
        .justify_center()
        .text_color(pal.text_primary)
        .child(label)
}

#[expect(
    clippy::too_many_arguments,
    clippy::needless_pass_by_value,
    reason = "one cohesive Spotlight inspector whose owned profile feeds static callbacks"
)]
fn buttons_inspector(
    settings: PresenterSettings,
    bindings: &std::collections::BTreeMap<ButtonId, Action>,
    profile: Option<String>,
    sliders: Option<&SettingsSliders>,
    speed_panel: AnyElement,
    color_input: &Entity<InputState>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> AnyElement {
    let effect_order = settings.normalized_effect_order();
    v_flex()
        .w_full()
        .p_4()
        .gap_4()
        .child(inspector_title(
            "Buttons & pointer",
            "Configure motion, ordered pointer effects, and every physical press.",
            pal,
        ))
        .child(speed_panel)
        .child(section_rule(pal))
        .child(
            v_flex()
                .gap_1()
                .child(div().text_subheading().child("Pointer effects"))
                .child(
                    div()
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child("Double-click cycles enabled effects in this order."),
                ),
        )
        .children(effect_order.into_iter().enumerate().map(|(index, effect)| {
            effect_card(index, effect, settings, profile.clone(), sliders, view, pal)
        }))
        .child(color_editor(
            color_input,
            settings,
            profile.clone(),
            view,
            pal,
        ))
        .child(section_rule(pal))
        .child(options_panel(settings, profile.clone(), view, pal))
        .child(section_rule(pal))
        .child(div().text_subheading().child("Button actions"))
        .child(div().text_caption().text_color(pal.text_muted).child(
            "Assign globally or per app. Action Palette exposes the full action set from one hold.",
        ))
        .children(
            [
                ButtonId::PresenterNext,
                ButtonId::PresenterNextHold,
                ButtonId::PresenterBack,
                ButtonId::PresenterBackHold,
            ]
            .into_iter()
            .enumerate()
            .map(|(index, button)| {
                presenter_control_row(
                    index,
                    button,
                    bindings.get(&button),
                    profile.clone(),
                    view,
                    pal,
                )
            }),
        )
        .into_any_element()
}

fn inspector_title(title: &str, subtitle: &str, pal: Palette) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_heading().child(title.to_string()))
        .child(
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(subtitle.to_string()),
        )
}

fn section_rule(pal: Palette) -> impl IntoElement {
    div().w_full().h(px(1.)).bg(pal.border)
}

#[expect(
    clippy::too_many_lines,
    reason = "one declarative effect card with effect-specific controls"
)]
fn effect_card(
    index: usize,
    effect: PresenterEffect,
    settings: PresenterSettings,
    profile: Option<String>,
    sliders: Option<&SettingsSliders>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> impl IntoElement {
    let enabled = settings.effect_enabled(effect);
    let selected = settings.effect == effect;
    let title = match effect {
        PresenterEffect::Highlight => "Highlight",
        PresenterEffect::Magnify => "Magnify",
        PresenterEffect::DigitalLaser => "Digital laser",
    };
    let select_view = view.clone();
    let select_profile = profile.clone();
    let toggle_view = view.clone();
    let toggle_profile = profile.clone();
    let up_view = view.clone();
    let up_profile = profile.clone();
    let down_view = view.clone();
    let down_profile = profile.clone();

    v_flex()
        .w_full()
        .gap_3()
        .p_3()
        .rounded(pal.control_radius)
        .border_1()
        .border_color(if selected {
            theme::accent()
        } else {
            pal.border
        })
        .bg(if selected {
            theme::accent_tint()
        } else {
            pal.control_hover
        })
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    Button::new(("presenter-select-effect", index))
                        .ghost()
                        .label(format!("{}. {title}", index + 1))
                        .on_click(move |_, _, cx| {
                            AppState::update(cx, |state, _| {
                                let mut settings =
                                    state.presenter_settings_for(select_profile.as_deref());
                                settings.effect = effect;
                                settings.enabled_effects |= effect.bit();
                                state.commit_presenter_settings_for(
                                    select_profile.as_deref(),
                                    settings,
                                );
                            });
                            select_view.update(cx, |_, cx| cx.notify());
                        }),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(("presenter-effect-up", index))
                                .compact()
                                .ghost()
                                .icon(IconName::ChevronUp)
                                .disabled(index == 0)
                                .on_click(move |_, _, cx| {
                                    AppState::update(cx, |state, _| {
                                        let mut settings =
                                            state.presenter_settings_for(up_profile.as_deref());
                                        if index > 0 {
                                            settings.effect_order.swap(index, index - 1);
                                        }
                                        state.commit_presenter_settings_for(
                                            up_profile.as_deref(),
                                            settings,
                                        );
                                    });
                                    up_view.update(cx, |_, cx| cx.notify());
                                }),
                        )
                        .child(
                            Button::new(("presenter-effect-down", index))
                                .compact()
                                .ghost()
                                .icon(IconName::ChevronDown)
                                .disabled(index == 2)
                                .on_click(move |_, _, cx| {
                                    AppState::update(cx, |state, _| {
                                        let mut settings =
                                            state.presenter_settings_for(down_profile.as_deref());
                                        if index < 2 {
                                            settings.effect_order.swap(index, index + 1);
                                        }
                                        state.commit_presenter_settings_for(
                                            down_profile.as_deref(),
                                            settings,
                                        );
                                    });
                                    down_view.update(cx, |_, cx| cx.notify());
                                }),
                        )
                        .child(
                            Switch::new(("presenter-effect-enabled", index))
                                .checked(enabled)
                                .on_click(move |checked, _, cx| {
                                    let enabled = *checked;
                                    AppState::update(cx, |state, _| {
                                        let mut settings =
                                            state.presenter_settings_for(toggle_profile.as_deref());
                                        if enabled {
                                            settings.enabled_effects |= effect.bit();
                                        } else if settings.enabled_effects.count_ones() > 1 {
                                            settings.enabled_effects &= !effect.bit();
                                        }
                                        state.commit_presenter_settings_for(
                                            toggle_profile.as_deref(),
                                            settings,
                                        );
                                    });
                                    toggle_view.update(cx, |_, cx| cx.notify());
                                }),
                        ),
                ),
        )
        .when(effect == PresenterEffect::Highlight, |card| {
            card.child(setting_slider(
                "Contrast",
                format!("{}%", settings.effect_contrast),
                sliders.map(|sliders| sliders.get(SettingsSliderKind::HighlightContrast)),
                pal,
            ))
            .child(setting_slider(
                "Spotlight size",
                format!("{} pt", settings.spotlight_radius),
                sliders.map(|sliders| sliders.get(SettingsSliderKind::HighlightRadius)),
                pal,
            ))
            .child(highlight_preview(settings, pal))
        })
        .when(effect == PresenterEffect::Magnify, |card| {
            card.child(setting_slider(
                "Lens size",
                format!("{} pt", settings.magnifier_radius),
                sliders.map(|sliders| sliders.get(SettingsSliderKind::MagnifierRadius)),
                pal,
            ))
            .child(setting_slider(
                "Magnification",
                format!(
                    "{:.2}×",
                    f32::from(settings.normalized_magnification()) / 100.
                ),
                sliders.map(|sliders| sliders.get(SettingsSliderKind::Magnification)),
                pal,
            ))
        })
        .when(effect == PresenterEffect::DigitalLaser, |card| {
            card.child(setting_slider(
                "Laser size",
                format!("{}%", settings.effect_size),
                sliders.map(|sliders| sliders.get(SettingsSliderKind::LaserSize)),
                pal,
            ))
            .child(color_swatches(
                "Laser color",
                settings.effect_color,
                false,
                profile,
                view,
                pal,
            ))
        })
}

fn highlight_preview(settings: PresenterSettings, pal: Palette) -> impl IntoElement {
    div()
        .w_full()
        .h(px(72.))
        .rounded(pal.control_radius)
        .bg(pal
            .text_primary
            .opacity(f32::from(settings.effect_contrast) / 125.))
        .flex()
        .items_center()
        .justify_center()
        .child(div().w(px(58.)).h(px(58.)).rounded_full().bg(pal.panel))
}

fn setting_slider(
    label: &str,
    value: String,
    slider: Option<&Entity<SliderState>>,
    pal: Palette,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .child(div().text_caption().child(label.to_string()))
                .child(div().text_caption().text_color(pal.text_muted).child(value)),
        )
        .child(slider.map_or_else(
            || div().into_any_element(),
            |slider| Slider::new(slider).horizontal().into_any_element(),
        ))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the owned profile is cloned into static GPUI callbacks"
)]
fn color_swatches(
    label: &'static str,
    selected: Rgb,
    magnifier: bool,
    profile: Option<String>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> impl IntoElement {
    const COLORS: [Rgb; 8] = [
        Rgb::new(0xff, 0x3b, 0x30),
        Rgb::new(0xff, 0x2d, 0x95),
        Rgb::new(0xaf, 0x52, 0xde),
        Rgb::new(0x00, 0x7a, 0xff),
        Rgb::new(0x32, 0xad, 0xe6),
        Rgb::new(0x34, 0xc7, 0x59),
        Rgb::new(0xff, 0xcc, 0x00),
        Rgb::new(0xff, 0xff, 0xff),
    ];
    v_flex()
        .gap_2()
        .child(div().text_caption().child(label))
        .child(
            h_flex()
                .gap_2()
                .children(COLORS.into_iter().enumerate().map(|(index, color)| {
                    let swatch_view = view.clone();
                    let swatch_profile = profile.clone();
                    let (red, green, blue) = color.components();
                    let packed = (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue);
                    div()
                        .id((
                            if magnifier {
                                "presenter-lens-color-swatch"
                            } else {
                                "presenter-laser-color-swatch"
                            },
                            index,
                        ))
                        .w(px(24.))
                        .h(px(24.))
                        .rounded_full()
                        .border_2()
                        .border_color(if selected == color {
                            theme::accent()
                        } else {
                            pal.border
                        })
                        .bg(rgb(packed))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            AppState::update(cx, |state, _| {
                                let mut settings =
                                    state.presenter_settings_for(swatch_profile.as_deref());
                                if magnifier {
                                    settings.magnifier_color = color;
                                } else {
                                    settings.effect_color = color;
                                }
                                state.commit_presenter_settings_for(
                                    swatch_profile.as_deref(),
                                    settings,
                                );
                            });
                            swatch_view.update(cx, |_, cx| cx.notify());
                        })
                })),
        )
}

fn color_editor(
    input: &Entity<InputState>,
    settings: PresenterSettings,
    profile: Option<String>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> impl IntoElement {
    let laser_input = input.clone();
    let lens_input = input.clone();
    let laser_view = view.clone();
    let lens_view = view.clone();
    let laser_profile = profile.clone();
    let lens_profile = profile.clone();
    v_flex()
        .gap_2()
        .child(color_swatches(
            "Magnifier rim color",
            settings.magnifier_color,
            true,
            profile,
            view,
            pal,
        ))
        .child(
            h_flex()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .child(Input::new(input).small().cleanable(true)),
                )
                .child(
                    Button::new("presenter-apply-laser-color")
                        .compact()
                        .label("Laser")
                        .on_click(move |_, _, cx| {
                            let value = laser_input.read(cx).value().to_string();
                            let Some(color) = parse_rgb(&value) else {
                                return;
                            };
                            AppState::update(cx, |state, _| {
                                let mut settings =
                                    state.presenter_settings_for(laser_profile.as_deref());
                                settings.effect_color = color;
                                state.commit_presenter_settings_for(
                                    laser_profile.as_deref(),
                                    settings,
                                );
                            });
                            laser_view.update(cx, |_, cx| cx.notify());
                        }),
                )
                .child(
                    Button::new("presenter-apply-lens-color")
                        .compact()
                        .label("Lens")
                        .on_click(move |_, _, cx| {
                            let value = lens_input.read(cx).value().to_string();
                            let Some(color) = parse_rgb(&value) else {
                                return;
                            };
                            AppState::update(cx, |state, _| {
                                let mut settings =
                                    state.presenter_settings_for(lens_profile.as_deref());
                                settings.magnifier_color = color;
                                state.commit_presenter_settings_for(
                                    lens_profile.as_deref(),
                                    settings,
                                );
                            });
                            lens_view.update(cx, |_, cx| cx.notify());
                        }),
                ),
        )
}

fn options_panel(
    settings: PresenterSettings,
    profile: Option<String>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> impl IntoElement {
    v_flex()
        .gap_3()
        .child(div().text_subheading().child("Pointer behavior"))
        .child(setting_switch(
            "Cursor control",
            "Interact with links and video controls while pointing.",
            settings.cursor_control,
            "presenter-cursor-control",
            profile.clone(),
            view,
            pal,
            |settings, checked| settings.cursor_control = checked,
        ))
        .child(setting_switch(
            "Recenter effects",
            "Move the effect to the display center when slides change.",
            settings.recenter_pointer,
            "presenter-recenter",
            profile.clone(),
            view,
            pal,
            |settings, checked| settings.recenter_pointer = checked,
        ))
        .child(setting_switch(
            "Freeze after release",
            "Keep the active effect visible after releasing Pointer.",
            settings.freeze_effect,
            "presenter-freeze",
            profile,
            view,
            pal,
            |settings, checked| settings.freeze_effect = checked,
        ))
}

#[expect(clippy::too_many_arguments, reason = "shared labeled setting switch")]
fn setting_switch(
    title: &'static str,
    subtitle: &'static str,
    checked: bool,
    id: &'static str,
    profile: Option<String>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
    update: fn(&mut PresenterSettings, bool),
) -> impl IntoElement {
    let view = view.clone();
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_3()
        .child(
            v_flex()
                .flex_1()
                .child(div().text_body().child(title))
                .child(
                    div()
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child(subtitle),
                ),
        )
        .child(
            Switch::new(id)
                .checked(checked)
                .on_click(move |checked, _, cx| {
                    let checked = *checked;
                    AppState::update(cx, |state, _| {
                        let mut settings = state.presenter_settings_for(profile.as_deref());
                        update(&mut settings, checked);
                        state.commit_presenter_settings_for(profile.as_deref(), settings);
                    });
                    view.update(cx, |_, cx| cx.notify());
                }),
        )
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the owned profile is cloned into static GPUI callbacks"
)]
fn timer_inspector(
    settings: PresenterSettings,
    profile: Option<String>,
    timer_input: &Entity<InputState>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> AnyElement {
    let modes = [
        (PresenterTimerMode::Off, "Off"),
        (PresenterTimerMode::CurrentTime, "Clock"),
        (PresenterTimerMode::Countdown, "Countdown"),
    ];
    let presets = [(15_u16, "15 min"), (30, "30 min"), (60, "60 min")];
    let input = timer_input.clone();
    let input_profile = profile.clone();
    let input_view = view.clone();
    v_flex()
        .w_full().p_4().gap_4()
        .child(inspector_title("Presentation timer", "Track elapsed context or count down to a vibration alert.", pal))
        .child(h_flex().gap_2().children(modes.into_iter().enumerate().map(|(index, (mode, label))| {
            let mode_profile = profile.clone();
            let mode_view = view.clone();
            Button::new(("presenter-timer-mode", index)).label(label)
                .when(settings.normalized_timer_mode() == mode, ButtonVariants::primary)
                .on_click(move |_, _, cx| {
                    AppState::update(cx, |state, _| {
                        let mut settings = state.presenter_settings_for(mode_profile.as_deref());
                        settings.timer_mode = mode;
                        if mode == PresenterTimerMode::Countdown && settings.timer_seconds == 0 { settings.timer_seconds = 15 * 60; }
                        state.commit_presenter_settings_for(mode_profile.as_deref(), settings);
                    });
                    mode_view.update(cx, |_, cx| cx.notify());
                })
        })))
        .when(settings.normalized_timer_mode() == PresenterTimerMode::Countdown, |panel| {
            panel
                .child(div().text_subheading().child("Presentation duration"))
                .child(h_flex().gap_2().children(presets.into_iter().enumerate().map(|(index, (minutes, label))| {
                    let preset_profile = profile.clone();
                    let preset_view = view.clone();
                    Button::new(("presenter-timer-preset", index)).label(label)
                        .when(settings.timer_seconds == minutes * 60, ButtonVariants::primary)
                        .on_click(move |_, _, cx| {
                            AppState::update(cx, |state, _| {
                                let mut settings = state.presenter_settings_for(preset_profile.as_deref());
                                settings.timer_mode = PresenterTimerMode::Countdown;
                                settings.timer_seconds = minutes * 60;
                                state.commit_presenter_settings_for(preset_profile.as_deref(), settings);
                            });
                            preset_view.update(cx, |_, cx| cx.notify());
                        })
                })))
                .child(h_flex().gap_2()
                    .child(div().flex_1().child(Input::new(timer_input).small().cleanable(true)))
                    .child(Button::new("presenter-custom-timer").label("Set minutes").on_click(move |_, _, cx| {
                        let value = input.read(cx).value().to_string();
                        let Ok(minutes) = value.trim().parse::<u16>() else { return; };
                        AppState::update(cx, |state, _| {
                            let mut settings = state.presenter_settings_for(input_profile.as_deref());
                            settings.timer_mode = PresenterTimerMode::Countdown;
                            settings.timer_seconds = minutes.clamp(1, u16::MAX / 60).saturating_mul(60);
                            state.commit_presenter_settings_for(input_profile.as_deref(), settings);
                        });
                        input_view.update(cx, |_, cx| cx.notify());
                    })))
        })
        .child(section_rule(pal))
        .child(v_flex().gap_1().child(div().text_subheading().child("Behavior")).child(div().text_caption().text_color(pal.text_muted).child("Countdown begins on the first Next / Back press. Clock shows local time. Timer alerts are configured under Vibration.")))
        .into_any_element()
}

fn vibration_inspector(
    settings: PresenterSettings,
    profile: Option<String>,
    sliders: Option<&SettingsSliders>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> AnyElement {
    v_flex()
        .w_full()
        .p_4()
        .gap_4()
        .child(inspector_title(
            "Vibration",
            "Use the Spotlight motor for quiet presentation alerts.",
            pal,
        ))
        .child(setting_slider(
            "Vibration intensity",
            format!("{}%", settings.vibration_intensity),
            sliders.map(|sliders| sliders.get(SettingsSliderKind::VibrationIntensity)),
            pal,
        ))
        .child(section_rule(pal))
        .child(setting_switch(
            "Low battery alert",
            "Vibrate once when the presenter reaches low charge.",
            settings.low_battery_alert,
            "presenter-low-battery-alert",
            profile.clone(),
            view,
            pal,
            |settings, checked| settings.low_battery_alert = checked,
        ))
        .child(setting_switch(
            "Timer alert",
            "Vibrate when the configured countdown finishes.",
            settings.haptic_alerts,
            "presenter-timer-alert",
            profile,
            view,
            pal,
            |settings, checked| settings.haptic_alerts = checked,
        ))
        .into_any_element()
}

fn speed_control(
    slider: Option<&Entity<SliderState>>,
    speed: PointerSpeed,
    pal: Palette,
) -> impl IntoElement {
    setting_slider(
        "Pointer speed",
        format!("{} / {}", speed.get() + 1, PointerSpeed::COUNT),
        slider,
        pal,
    )
}

fn pointer_speed_failure(
    error: String,
    key: DeviceKey,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> AnyElement {
    let view = view.clone();
    h_flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .text_caption()
                .text_color(pal.text_muted)
                .child(error),
        )
        .child(
            Button::new("presenter-pointer-speed-retry")
                .compact()
                .outline()
                .label("Retry")
                .on_click(move |_, _, cx| {
                    AppState::retry_pointer_speed_read(cx, key.clone());
                    view.update(cx, |_, cx| cx.notify());
                }),
        )
        .into_any_element()
}

fn presenter_control_row(
    index: usize,
    button: ButtonId,
    action: Option<&Action>,
    profile: Option<String>,
    view: &Entity<PresenterControlsView>,
    pal: Palette,
) -> impl IntoElement {
    let name = tr!(button.label()).to_string();
    let value = action.map_or_else(|| "Device default".to_string(), Action::label);
    let picker_view = view.clone();
    h_flex()
        .w_full()
        .items_center()
        .rounded(pal.control_radius)
        .bg(pal.control_hover)
        .p(px(2.))
        .child(
            Popover::new(("presenter-control", index))
                .appearance(false)
                .anchor(Anchor::TopLeft)
                .mouse_button(MouseButton::Left)
                .trigger(
                    Button::new(("presenter-control-trigger", index))
                        .outline()
                        .w_full()
                        .label(format!("{name} — {value}"))
                        .icon(IconName::ChevronRight),
                )
                .content(move |_state, _window, cx| {
                    presenter_action_picker(button, profile.clone(), &picker_view, cx)
                }),
        )
}

fn presenter_timer_label(settings: PresenterSettings) -> String {
    match settings.normalized_timer_mode() {
        PresenterTimerMode::Off => "Timer disabled".to_string(),
        PresenterTimerMode::CurrentTime => "Current time".to_string(),
        PresenterTimerMode::Countdown => format!("Countdown {} min", settings.timer_seconds / 60),
    }
}

fn parse_rgb(value: &str) -> Option<Rgb> {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 {
        return None;
    }
    let rgb = u32::from_str_radix(value, 16).ok()?;
    Some(Rgb::new(
        u8::try_from((rgb >> 16) & 0xff).ok()?,
        u8::try_from((rgb >> 8) & 0xff).ok()?,
        u8::try_from(rgb & 0xff).ok()?,
    ))
}

fn speed_from_slider(value: f32) -> Option<PointerSpeed> {
    let value = value.round().clamp(0., f32::from(PointerSpeed::MAX.get()));
    (0..PointerSpeed::COUNT)
        .find(|level| (f32::from(*level) - value).abs() < f32::EPSILON)
        .and_then(PointerSpeed::new)
}

fn muted(message: impl Into<String>, pal: Palette) -> AnyElement {
    div()
        .text_caption()
        .text_color(pal.text_muted)
        .child(message.into())
        .into_any_element()
}
