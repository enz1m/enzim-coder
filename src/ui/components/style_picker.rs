use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::data::AppDb;

thread_local! {
    static THEME_PROVIDER: RefCell<Option<gtk::CssProvider>> = RefCell::new(None);
    static THEME_CSS_CACHE: RefCell<Option<String>> = RefCell::new(None);
    static STYLE_PICKER_POPOVER: RefCell<Option<gtk::Popover>> = RefCell::new(None);
    static STYLE_PICKER_PREVIEW_PROVIDER: RefCell<Option<gtk::CssProvider>> = RefCell::new(None);
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeConfig {
    pub color: String,
    pub accent_auto: bool,
    pub accent_color: String,
    pub texture: String,
    pub texture_intensity: f64,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            color: "default".to_string(),
            accent_auto: false,
            accent_color: default_manual_accent_color(),
            texture: "none".to_string(),
            texture_intensity: 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WheelTarget {
    Base,
    Accent,
}

const BASE_MAX_BRIGHTNESS: f64 = 0.30;
const ACCENT_MAX_BRIGHTNESS: f64 = 1.0;

fn max_brightness_for(target: WheelTarget) -> f64 {
    match target {
        WheelTarget::Base => BASE_MAX_BRIGHTNESS,
        WheelTarget::Accent => ACCENT_MAX_BRIGHTNESS,
    }
}

pub fn create_style_picker_button(db: Rc<AppDb>) -> gtk::Box {
    let button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    button.add_css_class("style-picker-button");
    button.set_width_request(18);
    button.set_height_request(18);
    button.set_hexpand(false);
    button.set_vexpand(false);
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
    button.set_can_focus(false);
    let icon = gtk::Image::from_icon_name("color-symbolic");
    icon.set_pixel_size(15);
    icon.add_css_class("bottom-icon-image");
    button.append(&icon);
    button.set_tooltip_text(Some("Customize appearance"));

    let theme_config = Rc::new(RefCell::new(load_theme_config(&db)));

    {
        let hover_target = button.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            hover_target.add_css_class("is-hover");
        });
        let hover_target = button.clone();
        motion.connect_leave(move |_| {
            hover_target.remove_css_class("is-hover");
        });
        button.add_controller(motion);
    }

    {
        let db = db.clone();
        let theme_config = theme_config.clone();
        let active_target = button.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_pressed(move |_, _, _, _| {
            active_target.add_css_class("is-active");
        });

        let db = db.clone();
        let theme_config = theme_config.clone();
        let active_target = button.clone();
        click.connect_released(move |_, _, _, _| {
            active_target.remove_css_class("is-active");
            show_style_picker_popup(&active_target, db.clone(), theme_config.clone());
        });
        button.add_controller(click);
    }

    button
}

fn load_theme_config(db: &AppDb) -> ThemeConfig {
    let color = db
        .get_setting("theme_color")
        .ok()
        .flatten()
        .map(|raw| {
            if raw.trim().eq_ignore_ascii_case("system") {
                "default".to_string()
            } else {
                raw
            }
        })
        .unwrap_or_else(|| "default".to_string());
    let accent_auto = db
        .get_setting("theme_accent_auto")
        .ok()
        .flatten()
        .map(|raw| {
            let lowered = raw.trim().to_ascii_lowercase();
            !(lowered == "0" || lowered == "false" || lowered == "off")
        })
        .unwrap_or(false);
    let accent_color = db
        .get_setting("theme_accent_color")
        .ok()
        .flatten()
        .map(|raw| legacy_accent_preset_value(&raw).unwrap_or(raw))
        .unwrap_or_else(default_manual_accent_color);
    let texture = db
        .get_setting("theme_texture")
        .ok()
        .flatten()
        .unwrap_or_else(|| "none".to_string());
    let texture_intensity = db
        .get_setting("theme_texture_intensity")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.5);

    ThemeConfig {
        color,
        accent_auto,
        accent_color,
        texture,
        texture_intensity,
    }
}

fn show_style_picker_popup(
    relative_to: &impl IsA<gtk::Widget>,
    db: Rc<AppDb>,
    theme_config: Rc<RefCell<ThemeConfig>>,
) {
    let relative_widget: gtk::Widget = relative_to.as_ref().clone();
    if let Some(existing) = STYLE_PICKER_POPOVER.with(|cell| cell.borrow().clone()) {
        if let Some(parent) = existing.parent() {
            if parent.as_ptr() != relative_widget.as_ptr() {
                existing.unparent();
                existing.set_parent(&relative_widget);
            }
        } else {
            existing.set_parent(&relative_widget);
        }
        existing.popup();
        return;
    }

    let syncing_picker_controls = Rc::new(RefCell::new(false));
    let wheel_target = Rc::new(RefCell::new(WheelTarget::Base));
    let popover = gtk::Popover::new();
    popover.add_css_class("style-picker-popover");
    popover.set_parent(&relative_widget);
    popover.set_autohide(true);
    popover.set_position(gtk::PositionType::Top);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_start(20);
    content.set_margin_end(20);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.add_css_class("style-picker-content");

    let title = gtk::Label::new(Some("Customize Appearance"));
    title.add_css_class("style-picker-title");
    title.set_xalign(0.0);

    let page_stack = gtk::Stack::new();
    page_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    page_stack.set_transition_duration(180);
    page_stack.set_hhomogeneous(true);
    page_stack.set_vhomogeneous(false);

    let main_page = gtk::Box::new(gtk::Orientation::Vertical, 16);
    let wheel_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    wheel_page.set_halign(gtk::Align::Center);
    wheel_page.set_valign(gtk::Align::Center);
    wheel_page.set_hexpand(true);
    wheel_page.set_vexpand(false);
    main_page.append(&title);

    let color_wheel = gtk::DrawingArea::new();
    color_wheel.set_width_request(200);
    color_wheel.set_height_request(200);
    color_wheel.set_halign(gtk::Align::Center);
    color_wheel.add_css_class("style-picker-color-wheel");

    let selected_pos = Rc::new(RefCell::new((100.0, 100.0)));
    let brightness_value = Rc::new(RefCell::new(BASE_MAX_BRIGHTNESS));

    {
        let selected_pos = selected_pos.clone();
        let brightness_value = brightness_value.clone();
        color_wheel.set_draw_func(move |_area, cr, width, height| {
            let center_x = width as f64 / 2.0;
            let center_y = height as f64 / 2.0;
            let radius = center_x.min(center_y) - 10.0;
            let current_brightness = *brightness_value.borrow();

            for angle in 0..360 {
                let rad = (angle as f64).to_radians();
                let hue = angle as f64 / 360.0;

                for r in 0..100 {
                    let ratio = r as f64 / 100.0;
                    let current_radius = radius * ratio;

                    let saturation = ratio;
                    let value = current_brightness;

                    let (red, green, blue) = hsv_to_rgb(hue, saturation, value);
                    cr.set_source_rgb(red, green, blue);

                    let x = center_x + current_radius * rad.cos();
                    let y = center_y + current_radius * rad.sin();

                    cr.arc(x, y, 1.5, 0.0, 2.0 * std::f64::consts::PI);
                    let _ = cr.fill();
                }
            }

            let center_color = current_brightness;
            cr.set_source_rgb(center_color, center_color, center_color);
            cr.arc(
                center_x,
                center_y,
                radius * 0.15,
                0.0,
                2.0 * std::f64::consts::PI,
            );
            let _ = cr.fill();

            let (sel_x, sel_y) = *selected_pos.borrow();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(2.0);
            cr.arc(sel_x, sel_y, 6.0, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.stroke();

            cr.set_source_rgb(0.0, 0.0, 0.0);
            cr.set_line_width(1.0);
            cr.arc(sel_x, sel_y, 7.0, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.stroke();
        });
    }

    let brightness_slider = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        0.0,
        BASE_MAX_BRIGHTNESS,
        0.005,
    );
    brightness_slider.set_value(BASE_MAX_BRIGHTNESS);
    brightness_slider.set_draw_value(false);
    brightness_slider.set_width_request(200);
    brightness_slider.set_halign(gtk::Align::Center);
    brightness_slider.add_css_class("style-picker-brightness-slider");

    let hex_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    hex_box.set_halign(gtk::Align::Center);

    let hex_label = gtk::Label::new(Some("#"));
    hex_label.add_css_class("style-picker-hex-label");

    let current_hex = resolved_theme_hex(&theme_config.borrow().color.clone());
    let hex_entry = gtk::Entry::new();
    hex_entry.set_placeholder_text(Some("RRGGBB"));
    hex_entry.set_max_length(6);
    hex_entry.set_width_chars(8);
    hex_entry.add_css_class("style-picker-hex-entry");
    hex_entry.set_text(current_hex.trim_start_matches('#'));

    hex_box.append(&hex_label);
    hex_box.append(&hex_entry);

    let apply_wheel_hex: Rc<dyn Fn(&str)> = {
        let db = db.clone();
        let theme_config = theme_config.clone();
        let wheel_target = wheel_target.clone();
        Rc::new(move |hex: &str| {
            let next_hex = hex.trim().to_ascii_lowercase();
            if next_hex.len() != 7 || !next_hex.starts_with('#') {
                return;
            }
            match *wheel_target.borrow() {
                WheelTarget::Base => {
                    theme_config.borrow_mut().color = next_hex.clone();
                    let _ = db.set_setting("theme_color", &next_hex);
                }
                WheelTarget::Accent => {
                    {
                        let mut config = theme_config.borrow_mut();
                        config.accent_auto = false;
                        config.accent_color = next_hex.clone();
                    }
                    let _ = db.set_setting("theme_accent_auto", "0");
                    let _ = db.set_setting("theme_accent_color", &next_hex);
                }
            }
            apply_theme(&theme_config.borrow());
        })
    };

    {
        let hex_entry = hex_entry.clone();
        let color_wheel_ref = color_wheel.clone();
        let selected_pos = selected_pos.clone();
        let brightness_value = brightness_value.clone();
        let apply_wheel_hex = apply_wheel_hex.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_gesture, _n, x, y| {
            let width = color_wheel_ref.width() as f64;
            let height = color_wheel_ref.height() as f64;
            let center_x = width / 2.0;
            let center_y = height / 2.0;
            let radius = center_x.min(center_y) - 10.0;

            let dx = x - center_x;
            let dy = y - center_y;
            let distance = (dx * dx + dy * dy).sqrt();

            if distance <= radius {
                *selected_pos.borrow_mut() = (x, y);

                let angle = dy.atan2(dx);
                let hue = (angle.to_degrees() + 360.0) % 360.0 / 360.0;
                let saturation = (distance / radius).min(1.0);
                let value = *brightness_value.borrow();

                let (r, g, b) = hsv_to_rgb(hue, saturation, value);
                let hex = format!(
                    "{:02x}{:02x}{:02x}",
                    (r * 255.0) as u8,
                    (g * 255.0) as u8,
                    (b * 255.0) as u8
                );
                let next_hex = format!("#{hex}");

                hex_entry.set_text(&hex);
                apply_wheel_hex(&next_hex);
                color_wheel_ref.queue_draw();
            }
        });
        color_wheel.add_controller(gesture);
    }

    {
        let selected_pos = selected_pos.clone();
        let brightness_value = brightness_value.clone();
        let color_wheel = color_wheel.clone();
        let brightness_slider = brightness_slider.clone();
        let syncing_picker_controls = syncing_picker_controls.clone();
        let apply_wheel_hex = apply_wheel_hex.clone();
        let wheel_target = wheel_target.clone();
        hex_entry.connect_activate(move |entry| {
            let text = entry.text();
            let clean = text.trim().trim_start_matches('#').to_ascii_lowercase();
            if clean.len() != 6 || u32::from_str_radix(&clean, 16).is_err() {
                return;
            }
            let next_hex = format!("#{clean}");
            let max_brightness = max_brightness_for(*wheel_target.borrow());
            sync_picker_controls_from_hex(
                &next_hex,
                &selected_pos,
                &brightness_value,
                &brightness_slider,
                &color_wheel,
                &syncing_picker_controls,
                max_brightness,
            );
            apply_wheel_hex(&next_hex);
        });
    }
    {
        let hex_entry = hex_entry.clone();
        let selected_pos = selected_pos.clone();
        let brightness_value = brightness_value.clone();
        let color_wheel = color_wheel.clone();
        let syncing_picker_controls = syncing_picker_controls.clone();
        let apply_wheel_hex = apply_wheel_hex.clone();
        brightness_slider.connect_value_changed(move |slider| {
            if *syncing_picker_controls.borrow() {
                return;
            }

            let value = slider.value();
            *brightness_value.borrow_mut() = value;

            let (x, y) = *selected_pos.borrow();
            let width = if color_wheel.width() > 0 {
                color_wheel.width() as f64
            } else if color_wheel.width_request() > 0 {
                color_wheel.width_request() as f64
            } else {
                200.0
            };
            let height = if color_wheel.height() > 0 {
                color_wheel.height() as f64
            } else if color_wheel.height_request() > 0 {
                color_wheel.height_request() as f64
            } else {
                200.0
            };
            let center_x = width / 2.0;
            let center_y = height / 2.0;
            let radius = center_x.min(center_y) - 10.0;
            let dx = x - center_x;
            let dy = y - center_y;
            let distance = (dx * dx + dy * dy).sqrt();
            let angle = dy.atan2(dx);
            let hue = (angle.to_degrees() + 360.0) % 360.0 / 360.0;
            let saturation = if radius > 0.0 {
                (distance / radius).clamp(0.0, 1.0)
            } else {
                0.0
            };

            let (r, g, b) = hsv_to_rgb(hue, saturation, value);
            let hex = format!(
                "#{:02x}{:02x}{:02x}",
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8
            );
            hex_entry.set_text(hex.trim_start_matches('#'));
            apply_wheel_hex(&hex);
            color_wheel.queue_draw();
        });
    }

    let presets_label = gtk::Label::new(Some("Presets"));
    presets_label.add_css_class("style-picker-presets-label");
    presets_label.set_xalign(0.0);
    presets_label.set_margin_top(4);
    let color_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    color_section.append(&presets_label);

    let open_wheel_for_target: Rc<dyn Fn(WheelTarget)> = {
        let page_stack = page_stack.clone();
        let theme_config = theme_config.clone();
        let wheel_target = wheel_target.clone();
        let hex_entry = hex_entry.clone();
        let selected_pos = selected_pos.clone();
        let brightness_value = brightness_value.clone();
        let color_wheel = color_wheel.clone();
        let brightness_slider = brightness_slider.clone();
        let syncing_picker_controls = syncing_picker_controls.clone();
        Rc::new(move |target: WheelTarget| {
            wheel_target.replace(target);
            let max_brightness = max_brightness_for(target);
            brightness_slider.set_range(0.0, max_brightness);
            let current_hex = {
                let config = theme_config.borrow();
                let raw = match target {
                    WheelTarget::Base => config.color.clone(),
                    WheelTarget::Accent => config.accent_color.clone(),
                };
                resolved_theme_hex(&raw)
            };
            hex_entry.set_text(current_hex.trim_start_matches('#'));
            sync_picker_controls_from_hex(
                &current_hex,
                &selected_pos,
                &brightness_value,
                &brightness_slider,
                &color_wheel,
                &syncing_picker_controls,
                max_brightness,
            );
            page_stack.set_visible_child_name("wheel");
        })
    };

    let color_grid = create_color_grid(
        db.clone(),
        theme_config.clone(),
        hex_entry.clone(),
        selected_pos.clone(),
        color_wheel.clone(),
        brightness_slider.clone(),
        brightness_value.clone(),
        syncing_picker_controls.clone(),
        BASE_MAX_BRIGHTNESS,
        {
            let open_wheel_for_target = open_wheel_for_target.clone();
            Rc::new(move || {
                open_wheel_for_target(WheelTarget::Base);
            })
        },
    );
    color_section.append(&color_grid);

    let accent_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let accent_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    accent_header.set_halign(gtk::Align::Fill);
    let accent_title = gtk::Label::new(Some("Accent color"));
    accent_title.add_css_class("style-picker-presets-label");
    accent_title.set_xalign(0.0);
    accent_title.set_hexpand(true);
    let accent_auto_label = gtk::Label::new(Some("Auto"));
    accent_auto_label.add_css_class("style-picker-presets-label");
    let accent_auto_switch = gtk::Switch::new();
    accent_auto_switch.set_active(theme_config.borrow().accent_auto);
    accent_header.append(&accent_title);
    accent_header.append(&accent_auto_label);
    accent_header.append(&accent_auto_switch);
    accent_section.append(&accent_header);

    let accent_grid = create_accent_color_grid(db.clone(), theme_config.clone(), {
        let open_wheel_for_target = open_wheel_for_target.clone();
        Rc::new(move || {
            open_wheel_for_target(WheelTarget::Accent);
        })
    });
    accent_grid.set_visible(!theme_config.borrow().accent_auto);
    accent_section.append(&accent_grid);
    color_section.append(&accent_section);

    main_page.append(&color_section);

    let texture_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let texture_label = gtk::Label::new(Some("Textures"));
    texture_label.add_css_class("style-picker-presets-label");
    texture_label.set_xalign(0.0);
    texture_section.append(&texture_label);

    let intensity_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    intensity_box.set_halign(gtk::Align::Start);
    intensity_box.set_margin_top(2);

    let intensity_label = gtk::Label::new(Some("Intensity"));
    intensity_label.add_css_class("style-picker-presets-label");
    intensity_label.add_css_class("style-picker-intensity-label");
    intensity_label.set_xalign(0.0);

    let intensity_slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.05);
    intensity_slider.set_value(theme_config.borrow().texture_intensity);
    intensity_slider.set_width_request(150);
    intensity_slider.set_height_request(24);
    intensity_slider.set_draw_value(false);
    intensity_slider.add_css_class("style-picker-intensity-slider");

    let current_texture = theme_config.borrow().texture.clone();
    if current_texture == "none" {
        intensity_slider.set_sensitive(false);
        intensity_label.set_sensitive(false);
    }

    intensity_box.append(&intensity_label);
    intensity_box.append(&intensity_slider);

    let texture_grid = create_texture_grid(
        db.clone(),
        theme_config.clone(),
        intensity_slider.clone(),
        intensity_label.clone(),
    );
    texture_section.append(&texture_grid);
    texture_section.append(&intensity_box);
    main_page.append(&texture_section);

    {
        let db = db.clone();
        let theme_config = theme_config.clone();
        intensity_slider.connect_value_changed(move |slider| {
            let new_intensity = slider.value();
            theme_config.borrow_mut().texture_intensity = new_intensity;
            let _ = db.set_setting("theme_texture_intensity", &new_intensity.to_string());
            apply_theme(&theme_config.borrow());
        });
    }

    {
        let db = db.clone();
        let theme_config = theme_config.clone();
        let accent_grid = accent_grid.clone();
        accent_auto_switch.connect_active_notify(move |switch| {
            let is_auto = switch.is_active();
            theme_config.borrow_mut().accent_auto = is_auto;
            let _ = db.set_setting("theme_accent_auto", if is_auto { "1" } else { "0" });
            accent_grid.set_visible(!is_auto);
            apply_theme(&theme_config.borrow());
        });
    }

    wheel_page.append(&color_wheel);
    wheel_page.append(&brightness_slider);
    wheel_page.append(&hex_box);

    let wheel_apply = gtk::Button::with_label("Apply");
    wheel_apply.add_css_class("app-flat-button");
    wheel_apply.add_css_class("actions-add-button");
    wheel_apply.set_halign(gtk::Align::Center);
    {
        let page_stack = page_stack.clone();
        wheel_apply.connect_clicked(move |_| {
            page_stack.set_visible_child_name("main");
        });
    }
    wheel_page.append(&wheel_apply);

    page_stack.add_named(&main_page, Some("main"));
    page_stack.add_named(&wheel_page, Some("wheel"));
    page_stack.set_visible_child_name("main");

    content.append(&page_stack);

    sync_picker_controls_from_hex(
        &current_hex,
        &selected_pos,
        &brightness_value,
        &brightness_slider,
        &color_wheel,
        &syncing_picker_controls,
        BASE_MAX_BRIGHTNESS,
    );

    popover.set_child(Some(&content));
    popover.connect_closed(|p| {
        if p.child().is_some() {
            p.set_child(Option::<&gtk::Widget>::None);
        }
        if p.parent().is_some() {
            p.unparent();
        }
        STYLE_PICKER_POPOVER.with(|cell| {
            cell.borrow_mut().take();
        });
        STYLE_PICKER_PREVIEW_PROVIDER.with(|cell| {
            if let Some(provider) = cell.borrow_mut().take() {
                if let Some(display) = gtk::gdk::Display::default() {
                    gtk::style_context_remove_provider_for_display(&display, &provider);
                }
            }
        });
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        unsafe {
            libc::malloc_trim(0);
        }
    });
    popover.connect_destroy(|_| {
        STYLE_PICKER_POPOVER.with(|cell| {
            cell.borrow_mut().take();
        });
    });
    STYLE_PICKER_POPOVER.with(|cell| {
        cell.borrow_mut().replace(popover.clone());
    });
    popover.popup();
}

fn ensure_style_picker_preview_provider() {
    STYLE_PICKER_PREVIEW_PROVIDER.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }

        let display = gtk::gdk::Display::default().expect("Could not get default display");
        let provider = gtk::CssProvider::new();
        let mut css = String::new();
        for (id, _name, hex) in base_preset_colors() {
            css.push_str(&format!(".preset-color-{id} {{ background: {hex}; }}\n"));
        }
        for (id, _name, hex) in accent_preset_colors() {
            css.push_str(&format!(".preset-accent-{id} {{ background: {hex}; }}\n"));
        }
        css.push_str(
            r#"
            .texture-preview-none {
                background: alpha(@window_fg_color, 0.08);
                border-radius: 6px;
            }
            .texture-preview-dots {
                background-image:
                    radial-gradient(circle, alpha(@window_fg_color, 0.15) 2px, transparent 2px),
                    linear-gradient(135deg, alpha(@window_fg_color, 0.08) 0%, alpha(@window_fg_color, 0.08) 100%);
                background-size: 12px 12px, 100% 100%;
                border-radius: 6px;
            }
            .texture-preview-grid {
                background-image:
                    linear-gradient(alpha(@window_fg_color, 0.12) 1px, transparent 1px),
                    linear-gradient(90deg, alpha(@window_fg_color, 0.12) 1px, transparent 1px),
                    linear-gradient(135deg, alpha(@window_fg_color, 0.08) 0%, alpha(@window_fg_color, 0.08) 100%);
                background-size: 12px 12px, 12px 12px, 100% 100%;
                border-radius: 6px;
            }
            .texture-preview-grain {
                background-image:
                    repeating-radial-gradient(145% 108% at -18% 118%,
                        transparent 0 7px,
                        alpha(white, 0.16) 7px 8.4px,
                        transparent 8.4px 14px),
                    repeating-radial-gradient(132% 100% at 116% -16%,
                        transparent 0 9px,
                        alpha(white, 0.09) 9px 10.1px,
                        transparent 10.1px 17px),
                    radial-gradient(52% 44% at 52% 50%, alpha(black, 0.10) 0%, transparent 74%),
                    linear-gradient(135deg, alpha(@window_fg_color, 0.10) 0%, alpha(@window_fg_color, 0.06) 100%);
                background-size: 100% 100%, 100% 100%, 100% 100%, 100% 100%;
                background-position: 0 0, 0 0, 0 0, 0 0;
                border-radius: 6px;
            }
            "#,
        );
        provider.load_from_string(&css);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        cell.borrow_mut().replace(provider);
    });
}

fn base_preset_colors() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("default", "Default", "#181616"),
        ("blue", "Blue", "#1e3a5f"),
        ("purple", "Purple", "#3a2a4f"),
        ("green", "Green", "#2a3f2a"),
        ("red", "Red", "#4f2a2a"),
        ("orange", "Orange", "#4f3a2a"),
    ]
}

fn accent_preset_colors() -> Vec<(&'static str, &'static str, String)> {
    base_preset_colors()
        .into_iter()
        .map(|(id, name, hex)| {
            let (accent_bg_hex, _accent_hex) = complementary_accent_for(hex);
            (id, name, accent_bg_hex)
        })
        .collect()
}

fn default_manual_accent_color() -> String {
    "#519a95".to_string()
}

fn legacy_accent_preset_value(raw: &str) -> Option<String> {
    let lowered = raw.trim().to_ascii_lowercase();
    if lowered.starts_with('#') {
        return None;
    }
    base_preset_colors()
        .into_iter()
        .find(|(id, _, _)| *id == lowered)
        .map(|(_, _, hex)| {
            let (accent_bg_hex, _accent_hex) = complementary_accent_for(hex);
            accent_bg_hex
        })
}

fn create_color_grid(
    db: Rc<AppDb>,
    theme_config: Rc<RefCell<ThemeConfig>>,
    hex_entry: gtk::Entry,
    selected_pos: Rc<RefCell<(f64, f64)>>,
    color_wheel: gtk::DrawingArea,
    brightness_slider: gtk::Scale,
    brightness_value: Rc<RefCell<f64>>,
    syncing_picker_controls: Rc<RefCell<bool>>,
    max_brightness: f64,
    open_wheel: Rc<dyn Fn()>,
) -> gtk::Box {
    ensure_style_picker_preview_provider();

    let grid = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    grid.add_css_class("style-picker-grid");
    grid.set_halign(gtk::Align::Center);

    let colors = base_preset_colors();
    let buttons: Rc<RefCell<Vec<(String, gtk::Button)>>> = Rc::new(RefCell::new(Vec::new()));

    for (id, name, hex) in colors {
        let button = gtk::Button::new();
        button.add_css_class("style-picker-color-button");
        button.set_tooltip_text(Some(name));

        let color_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        color_box.set_width_request(36);
        color_box.set_height_request(36);
        color_box.add_css_class("style-picker-color-preview");

        let css_class = format!("preset-color-{}", id);
        color_box.add_css_class(&css_class);

        button.set_child(Some(&color_box));

        let current_color = theme_config.borrow().color.to_ascii_lowercase();
        if current_color == id || (!hex.is_empty() && current_color == hex.to_ascii_lowercase()) {
            button.add_css_class("style-picker-selected");
        }

        {
            let db = db.clone();
            let theme_config = theme_config.clone();
            let hex_entry = hex_entry.clone();
            let stored_value = if hex.is_empty() {
                id.to_string()
            } else {
                hex.to_string()
            };
            let selected_pos = selected_pos.clone();
            let color_wheel = color_wheel.clone();
            let brightness_slider = brightness_slider.clone();
            let brightness_value = brightness_value.clone();
            let syncing_picker_controls = syncing_picker_controls.clone();
            let buttons = buttons.clone();
            button.connect_clicked(move |_| {
                let next_hex = resolved_theme_hex(&stored_value);
                theme_config.borrow_mut().color = stored_value.clone();
                let _ = db.set_setting("theme_color", &stored_value);

                hex_entry.set_text(next_hex.trim_start_matches('#'));
                sync_picker_controls_from_hex(
                    &next_hex,
                    &selected_pos,
                    &brightness_value,
                    &brightness_slider,
                    &color_wheel,
                    &syncing_picker_controls,
                    max_brightness,
                );

                apply_theme(&theme_config.borrow());
                for (btn_id, btn) in buttons.borrow().iter() {
                    if btn_id == &stored_value {
                        btn.add_css_class("style-picker-selected");
                    } else {
                        btn.remove_css_class("style-picker-selected");
                    }
                }
            });
        }

        buttons.borrow_mut().push((hex.to_string(), button.clone()));
        grid.append(&button);
    }

    let wheel_button = create_wheel_button("Color wheel");
    let current_color = theme_config.borrow().color.to_ascii_lowercase();
    let has_preset = base_preset_colors().into_iter().any(|(id, _, hex)| {
        current_color == id || (!hex.is_empty() && current_color == hex.to_ascii_lowercase())
    });
    if current_color.starts_with('#') && !has_preset {
        wheel_button.add_css_class("style-picker-selected");
    }
    {
        let open_wheel = open_wheel.clone();
        let buttons = buttons.clone();
        wheel_button.connect_clicked(move |_| {
            for (btn_id, btn) in buttons.borrow().iter() {
                if btn_id == "__wheel__" {
                    btn.add_css_class("style-picker-selected");
                } else {
                    btn.remove_css_class("style-picker-selected");
                }
            }
            open_wheel();
        });
    }
    buttons
        .borrow_mut()
        .push(("__wheel__".to_string(), wheel_button.clone()));
    grid.append(&wheel_button);

    grid
}

fn create_accent_color_grid(
    db: Rc<AppDb>,
    theme_config: Rc<RefCell<ThemeConfig>>,
    open_wheel: Rc<dyn Fn()>,
) -> gtk::Box {
    ensure_style_picker_preview_provider();

    let grid = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    grid.add_css_class("style-picker-grid");
    grid.set_halign(gtk::Align::Center);

    let colors = accent_preset_colors();
    let buttons: Rc<RefCell<Vec<(String, gtk::Button)>>> = Rc::new(RefCell::new(Vec::new()));

    for (id, name, hex) in colors {
        let button = gtk::Button::new();
        button.add_css_class("style-picker-color-button");
        button.set_tooltip_text(Some(name));

        let color_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        color_box.set_width_request(36);
        color_box.set_height_request(36);
        color_box.add_css_class("style-picker-color-preview");
        let css_class = format!("preset-accent-{}", id);
        color_box.add_css_class(&css_class);
        button.set_child(Some(&color_box));

        let current_color = theme_config.borrow().accent_color.to_ascii_lowercase();
        if current_color == hex.to_ascii_lowercase() {
            button.add_css_class("style-picker-selected");
        }

        {
            let db = db.clone();
            let theme_config = theme_config.clone();
            let buttons = buttons.clone();
            let stored_value = hex.clone();
            button.connect_clicked(move |_| {
                {
                    let mut config = theme_config.borrow_mut();
                    config.accent_auto = false;
                    config.accent_color = stored_value.clone();
                }
                let _ = db.set_setting("theme_accent_auto", "0");
                let _ = db.set_setting("theme_accent_color", &stored_value);
                apply_theme(&theme_config.borrow());

                for (btn_id, btn) in buttons.borrow().iter() {
                    if btn_id == &stored_value {
                        btn.add_css_class("style-picker-selected");
                    } else {
                        btn.remove_css_class("style-picker-selected");
                    }
                }
            });
        }

        buttons.borrow_mut().push((hex.clone(), button.clone()));
        grid.append(&button);
    }

    let wheel_button = create_wheel_button("Accent wheel");
    let current_color = theme_config.borrow().accent_color.to_ascii_lowercase();
    let has_preset = accent_preset_colors()
        .into_iter()
        .any(|(_id, _, hex)| current_color == hex.to_ascii_lowercase());
    if current_color.starts_with('#') && !has_preset {
        wheel_button.add_css_class("style-picker-selected");
    }
    {
        let db = db.clone();
        let theme_config = theme_config.clone();
        let open_wheel = open_wheel.clone();
        let buttons = buttons.clone();
        wheel_button.connect_clicked(move |_| {
            theme_config.borrow_mut().accent_auto = false;
            let _ = db.set_setting("theme_accent_auto", "0");
            for (btn_id, btn) in buttons.borrow().iter() {
                if btn_id == "__wheel__" {
                    btn.add_css_class("style-picker-selected");
                } else {
                    btn.remove_css_class("style-picker-selected");
                }
            }
            open_wheel();
        });
    }
    buttons
        .borrow_mut()
        .push(("__wheel__".to_string(), wheel_button.clone()));
    grid.append(&wheel_button);

    grid
}

fn create_texture_grid(
    db: Rc<AppDb>,
    theme_config: Rc<RefCell<ThemeConfig>>,
    intensity_slider: gtk::Scale,
    intensity_label: gtk::Label,
) -> gtk::Box {
    ensure_style_picker_preview_provider();

    let grid = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    grid.add_css_class("style-picker-grid");

    let textures = vec![
        ("none", "None"),
        ("dots", "Dots"),
        ("grid", "Grid"),
        ("grain", "Waves"),
    ];

    let buttons: Rc<RefCell<Vec<(String, gtk::Button)>>> = Rc::new(RefCell::new(Vec::new()));

    for (id, name) in textures {
        let button = gtk::Button::new();
        button.add_css_class("style-picker-texture-button");
        button.set_tooltip_text(Some(name));

        let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        preview_box.set_width_request(48);
        preview_box.set_height_request(48);
        preview_box.add_css_class("style-picker-texture-preview");

        let pattern_class = format!("texture-preview-{}", id);
        preview_box.add_css_class(&pattern_class);

        button.set_child(Some(&preview_box));

        let current_texture = theme_config.borrow().texture.clone();
        if current_texture == id {
            button.add_css_class("style-picker-selected");
        }

        {
            let db = db.clone();
            let theme_config = theme_config.clone();
            let id = id.to_string();
            let buttons = buttons.clone();
            let intensity_slider = intensity_slider.clone();
            let intensity_label = intensity_label.clone();
            button.connect_clicked(move |_| {
                theme_config.borrow_mut().texture = id.clone();
                let _ = db.set_setting("theme_texture", &id);
                apply_theme(&theme_config.borrow());

                let is_none = id == "none";
                intensity_slider.set_sensitive(!is_none);
                intensity_label.set_sensitive(!is_none);

                for (btn_id, btn) in buttons.borrow().iter() {
                    if btn_id == &id {
                        btn.add_css_class("style-picker-selected");
                    } else {
                        btn.remove_css_class("style-picker-selected");
                    }
                }
            });
        }

        buttons.borrow_mut().push((id.to_string(), button.clone()));
        grid.append(&button);
    }

    grid
}

fn create_wheel_button(tooltip: &str) -> gtk::Button {
    let wheel_button = gtk::Button::new();
    wheel_button.add_css_class("style-picker-color-button");
    wheel_button.add_css_class("style-picker-wheel-button");
    wheel_button.set_tooltip_text(Some(tooltip));

    let wheel_preview = gtk::Overlay::new();
    wheel_preview.set_width_request(36);
    wheel_preview.set_height_request(36);
    wheel_preview.add_css_class("style-picker-color-preview");
    wheel_preview.add_css_class("style-picker-wheel-preview");

    let wheel_canvas = gtk::DrawingArea::new();
    wheel_canvas.set_content_width(36);
    wheel_canvas.set_content_height(36);
    wheel_canvas.set_draw_func(move |_area, cr, width, height| {
        let center_x = width as f64 / 2.0;
        let center_y = height as f64 / 2.0;
        let outer_radius = center_x.min(center_y) - 2.0;
        let inner_radius = outer_radius * 0.42;

        for angle in 0..360 {
            let start = (angle as f64).to_radians();
            let end = ((angle + 1) as f64).to_radians();
            let (r, g, b) = hsv_to_rgb(angle as f64 / 360.0, 0.78, 0.30);
            cr.set_source_rgb(r, g, b);
            cr.move_to(
                center_x + inner_radius * start.cos(),
                center_y + inner_radius * start.sin(),
            );
            cr.arc(center_x, center_y, outer_radius, start, end);
            cr.arc_negative(center_x, center_y, inner_radius, end, start);
            cr.close_path();
            let _ = cr.fill();
        }

        cr.set_source_rgba(0.95, 0.95, 0.95, 0.16);
        cr.arc(
            center_x,
            center_y,
            inner_radius - 1.0,
            0.0,
            2.0 * std::f64::consts::PI,
        );
        let _ = cr.fill();
    });
    wheel_preview.set_child(Some(&wheel_canvas));

    let wheel_icon = gtk::Image::from_icon_name("color-symbolic");
    wheel_icon.set_pixel_size(12);
    wheel_icon.set_halign(gtk::Align::Center);
    wheel_icon.set_valign(gtk::Align::Center);
    wheel_icon.add_css_class("style-picker-wheel-icon");
    wheel_preview.add_overlay(&wheel_icon);

    wheel_button.set_child(Some(&wheel_preview));
    wheel_button
}

fn resolved_theme_hex(raw: &str) -> String {
    if raw.starts_with('#') && raw.len() == 7 {
        return raw.to_string();
    }
    match raw {
        "blue" => "#1e3a5f".to_string(),
        "purple" => "#3a2a4f".to_string(),
        "green" => "#2a3f2a".to_string(),
        "red" => "#4f2a2a".to_string(),
        "orange" => "#4f3a2a".to_string(),
        _ => "#181616".to_string(),
    }
}

fn lighten_hex_color(hex: &str, amount: f64) -> String {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() != 6 {
        return hex.to_string();
    }

    let Ok(r) = u8::from_str_radix(&clean[0..2], 16) else {
        return hex.to_string();
    };
    let Ok(g) = u8::from_str_radix(&clean[2..4], 16) else {
        return hex.to_string();
    };
    let Ok(b) = u8::from_str_radix(&clean[4..6], 16) else {
        return hex.to_string();
    };

    let amount = amount.clamp(0.0, 1.0);
    let lift = |value: u8| -> u8 {
        let current = value as f64;
        (current + (255.0 - current) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };

    format!("#{:02x}{:02x}{:02x}", lift(r), lift(g), lift(b))
}

fn hex_to_rgb_unit(hex: &str) -> Option<(f64, f64, f64)> {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&clean[0..2], 16).ok()?;
    let g = u8::from_str_radix(&clean[2..4], 16).ok()?;
    let b = u8::from_str_radix(&clean[4..6], 16).ok()?;
    Some((r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0))
}

fn rgb_unit_to_hex(r: f64, g: f64, b: f64) -> String {
    let to_u8 = |value: f64| -> u8 { (value.clamp(0.0, 1.0) * 255.0).round() as u8 };
    format!("#{:02x}{:02x}{:02x}", to_u8(r), to_u8(g), to_u8(b))
}

fn complementary_accent_for(base_hex: &str) -> (String, String) {
    let Some((r, g, b)) = hex_to_rgb_unit(base_hex) else {
        return ("#b78354".to_string(), "#d6a173".to_string());
    };

    let (h, s, v) = rgb_to_hsv(r, g, b);
    let accent_h = (h + 0.5).fract();
    let accent_s = (0.45 + s * 0.45).clamp(0.45, 0.88);
    let accent_v = if v < 0.28 {
        0.82
    } else if v < 0.48 {
        0.76
    } else {
        0.70
    };
    let (accent_bg_r, accent_bg_g, accent_bg_b) = hsv_to_rgb(accent_h, accent_s, accent_v);
    let accent_bg_hex = rgb_unit_to_hex(accent_bg_r, accent_bg_g, accent_bg_b);

    let accent_fg_s = (accent_s * 0.82).clamp(0.36, 0.78);
    let accent_fg_v = (accent_v + 0.14).clamp(0.0, 0.95);
    let (accent_r, accent_g, accent_b) = hsv_to_rgb(accent_h, accent_fg_s, accent_fg_v);
    let accent_hex = rgb_unit_to_hex(accent_r, accent_g, accent_b);

    (accent_bg_hex, accent_hex)
}

fn accent_from_manual_color(accent_raw: &str) -> (String, String) {
    let accent_bg_hex = resolved_theme_hex(accent_raw);
    let accent_hex = lighten_hex_color(&accent_bg_hex, 0.18);
    (accent_bg_hex, accent_hex)
}

fn sync_picker_controls_from_hex(
    hex: &str,
    selected_pos: &Rc<RefCell<(f64, f64)>>,
    brightness_value: &Rc<RefCell<f64>>,
    brightness_slider: &gtk::Scale,
    color_wheel: &gtk::DrawingArea,
    syncing_picker_controls: &Rc<RefCell<bool>>,
    max_brightness: f64,
) {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() != 6 {
        return;
    }
    let Ok(r) = u8::from_str_radix(&clean[0..2], 16) else {
        return;
    };
    let Ok(g) = u8::from_str_radix(&clean[2..4], 16) else {
        return;
    };
    let Ok(b) = u8::from_str_radix(&clean[4..6], 16) else {
        return;
    };

    let (h, s, v) = rgb_to_hsv(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let v = v.clamp(0.0, max_brightness);

    let width = if color_wheel.width() > 0 {
        color_wheel.width() as f64
    } else if color_wheel.width_request() > 0 {
        color_wheel.width_request() as f64
    } else {
        200.0
    };
    let height = if color_wheel.height() > 0 {
        color_wheel.height() as f64
    } else if color_wheel.height_request() > 0 {
        color_wheel.height_request() as f64
    } else {
        200.0
    };
    let center_x = width / 2.0;
    let center_y = height / 2.0;
    let radius = center_x.min(center_y) - 10.0;
    let angle = h * 2.0 * std::f64::consts::PI;
    let x = center_x + (radius * s * angle.cos());
    let y = center_y + (radius * s * angle.sin());

    *selected_pos.borrow_mut() = (x, y);
    *brightness_value.borrow_mut() = v;
    syncing_picker_controls.replace(true);
    brightness_slider.set_value(v);
    syncing_picker_controls.replace(false);
    color_wheel.queue_draw();
}

fn apply_theme(config: &ThemeConfig) {
    let display = gtk::gdk::Display::default().expect("Could not get default display");

    THEME_PROVIDER.with(|provider_cell| {
        let mut provider_opt = provider_cell.borrow_mut();

        let css_provider = provider_opt.get_or_insert_with(|| {
            let provider = gtk::CssProvider::new();
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
            provider
        });

        let mut css = String::new();

        let base_color = if config.color.starts_with('#') {
            config.color.as_str()
        } else {
            match config.color.as_str() {
                "system" => "#181616",
                "blue" => "#1e3a5f",
                "purple" => "#3a2a4f",
                "green" => "#2a3f2a",
                "red" => "#4f2a2a",
                "orange" => "#4f3a2a",
                _ => "#181616",
            }
        };
        let popup_color = lighten_hex_color(base_color, 0.10);
        let (accent_bg_color, accent_color) = if config.accent_auto {
            complementary_accent_for(base_color)
        } else {
            accent_from_manual_color(&config.accent_color)
        };

        let combined_css = match config.texture.as_str() {
            "dots" => {
                let size = 10.0 + (config.texture_intensity * 30.0);
                let opacity = 0.01 + (config.texture_intensity * 0.05);
                format!(r#"
                    .main-container {{
                        background-image:
                            radial-gradient(circle, alpha(white, {}) 2px, transparent 2px),
                            linear-gradient(135deg, {} 0%, {} 100%);
                        background-size: {}px {}px, 100% 100%;
                    }}
                "#, opacity, base_color, base_color, size, size)
            },
            "grid" => {
                let size = 10.0 + (config.texture_intensity * 30.0);
                let opacity = 0.01 + (config.texture_intensity * 0.04);
                format!(r#"
                    .main-container {{
                        background-image:
                            linear-gradient(alpha(white, {}) 1px, transparent 1px),
                            linear-gradient(90deg, alpha(white, {}) 1px, transparent 1px),
                            linear-gradient(135deg, {} 0%, {} 100%);
                        background-size: {}px {}px, {}px {}px, 100% 100%;
                    }}
                "#, opacity, opacity, base_color, base_color, size, size, size, size)
            },
            "grain" => {
                let line_bright = 0.09 + (config.texture_intensity * 0.12);
                let line_soft = 0.045 + (config.texture_intensity * 0.07);
                let band_gap_a = 8.0 + (config.texture_intensity * 8.0);
                let band_gap_b = 10.0 + (config.texture_intensity * 10.0);
                let band_width_a = 1.1 + (config.texture_intensity * 1.0);
                let band_width_b = 0.9 + (config.texture_intensity * 0.9);
                let shadow_strength = 0.05 + (config.texture_intensity * 0.05);
                format!(r#"
                    .main-container {{
                        background-image:
                            repeating-radial-gradient(146% 110% at -18% 118%,
                                transparent 0 {}px,
                                alpha(white, {}) {}px calc({}px + {}px),
                                transparent calc({}px + {}px) calc({}px + 6px)),
                            repeating-radial-gradient(132% 100% at 116% -16%,
                                transparent 0 {}px,
                                alpha(white, {}) {}px calc({}px + {}px),
                                transparent calc({}px + {}px) calc({}px + 7px)),
                            radial-gradient(50% 42% at 52% 50%, alpha(black, {}) 0%, transparent 76%),
                            linear-gradient(135deg, {} 0%, {} 100%);
                        background-size: 100% 100%, 100% 100%, 100% 100%, 100% 100%;
                        background-position: 0 0, 0 0, 0 0, 0 0, 0 0;
                    }}
                "#,
                    band_gap_a,
                    line_bright,
                    band_gap_a,
                    band_gap_a,
                    band_width_a,
                    band_gap_a,
                    band_width_a,
                    band_gap_a,
                    band_gap_b,
                    line_soft,
                    band_gap_b,
                    band_gap_b,
                    band_width_b,
                    band_gap_b,
                    band_width_b,
                    band_gap_b,
                    shadow_strength,
                    base_color,
                    base_color
                )
            },
            _ => format!(r#"
                .main-container {{
                    background: linear-gradient(135deg, {} 0%, {} 100%);
                }}
            "#, base_color, base_color),
        };

        css.push_str(&combined_css);
        css.push_str(
            format!(
                r#"
                @define-color enzim_window_bg {};
                @define-color enzim_popup_bg {};
                @define-color enzim_accent_bg {};
                @define-color enzim_accent {};
                @define-color window_bg_color @enzim_window_bg;
                @define-color accent_bg_color @enzim_accent_bg;
                @define-color accent_color @enzim_accent;
                @define-color accent_fg_color #ffffff;

                window,
                window.background,
                dialog,
                popover > contents,
                popover.background > contents,
                popover.menu > contents {{
                    background-color: alpha(@window_bg_color, 0.98);
                    background-image: none;
                }}

                popover > arrow,
                popover.background > arrow,
                popover.menu > arrow {{
                    background-color: alpha(@window_bg_color, 0.98);
                    background-image: none;
                    border: none;
                    border-color: transparent;
                    box-shadow: none;
                }}

                popover.actions-popover > contents,
                popover.actions-popover.background > contents,
                popover.style-picker-popover > contents,
                popover.style-picker-popover.background > contents,
                popover.composer-worktree-popover > contents,
                popover.composer-worktree-popover.background > contents,
                popover.compact-selector-popover > contents,
                popover.compact-selector-popover.background > contents {{
                    background-color: alpha(@enzim_popup_bg, 0.98);
                    background-image: none;
                }}

                popover.style-picker-popover > contents,
                popover.style-picker-popover.background > contents,
                popover.composer-worktree-popover > contents,
                popover.composer-worktree-popover.background > contents {{
                    border: 1px solid alpha(@window_fg_color, 0.14);
                    box-shadow: 0 10px 26px alpha(black, 0.28);
                }}

                popover.actions-popover > arrow,
                popover.actions-popover.background > arrow,
                popover.style-picker-popover > arrow,
                popover.style-picker-popover.background > arrow {{
                    background-color: alpha(@enzim_popup_bg, 0.98);
                    background-image: none;
                    border: 1px solid alpha(@window_fg_color, 0.14);
                    box-shadow: none;
                }}

                popover.compact-selector-popover > arrow,
                popover.compact-selector-popover.background > arrow,
                popover.composer-worktree-popover > arrow,
                popover.composer-worktree-popover.background > arrow,
                popover.composer-attach-popover > arrow,
                popover.composer-attach-popover.background > arrow,
                popover.composer-attach-picker-popover > arrow,
                popover.composer-attach-picker-popover.background > arrow {{
                    background-color: alpha(@enzim_popup_bg, 0.98);
                    background-image: none;
                    border: 1px solid alpha(@window_fg_color, 0.14);
                    box-shadow: none;
                }}

                window.settings-window,
                window.thread-settings-window,
                window.file-preview-window,
                window.git-dialog,
                window.restore-preview-dialog {{
                    background-color: alpha(@window_bg_color, 0.98);
                    background-image: none;
                }}

                .sidebar-scroll-fade-top {{
                    background-image: linear-gradient(to bottom,
                        alpha({}, 0.90) 0%,
                        alpha({}, 0.38) 48%,
                        alpha({}, 0.0) 100%);
                }}

                .sidebar-scroll-fade-bottom {{
                    background-image: linear-gradient(to top,
                        alpha({}, 0.90) 0%,
                        alpha({}, 0.38) 48%,
                        alpha({}, 0.0) 100%);
                }}
            "#,
                base_color,
                popup_color,
                accent_bg_color,
                accent_color,
                base_color, base_color, base_color, base_color, base_color, base_color
            )
            .as_str(),
        );
        let should_reload = THEME_CSS_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.as_deref() == Some(css.as_str()) {
                false
            } else {
                cache.replace(css.clone());
                true
            }
        });
        if should_reload {
            css_provider.load_from_string(&css);
        }
    });
}

pub fn initialize_theme(db: &AppDb) {
    let config = load_theme_config(db);
    apply_theme(&config);
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let c = v * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r, g, b) = if h < 1.0 / 6.0 {
        (c, x, 0.0)
    } else if h < 2.0 / 6.0 {
        (x, c, 0.0)
    } else if h < 3.0 / 6.0 {
        (0.0, c, x)
    } else if h < 4.0 / 6.0 {
        (0.0, x, c)
    } else if h < 5.0 / 6.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (r + m, g + m, b + m)
}

fn rgb_to_hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let h = if delta == 0.0 {
        0.0
    } else if (max - r).abs() < f64::EPSILON {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if (max - g).abs() < f64::EPSILON {
        (((b - r) / delta) + 2.0) / 6.0
    } else {
        (((r - g) / delta) + 4.0) / 6.0
    };

    let s = if max == 0.0 { 0.0 } else { delta / max };
    (h, s, max)
}
