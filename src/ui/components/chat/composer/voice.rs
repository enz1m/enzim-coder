use super::*;
use crate::codex_profiles::CodexProfileManager;
use crate::data::VoiceToTextConfig;
use std::path::Path;
use std::process::{Child, Command, Stdio};

const WHISPER_TINY_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin";

#[derive(Default)]
pub(super) struct VoiceCaptureState {
    pub(super) recording_child: Option<Child>,
    pub(super) recording_path: Option<PathBuf>,
    pub(super) transcribing: bool,
}

pub(crate) fn build_settings_page(
    dialog: &gtk::Window,
    db: Rc<AppDb>,
    on_saved: Option<Rc<dyn Fn(VoiceToTextConfig)>>,
    close_on_save: bool,
) -> gtk::Box {
    let dialog = dialog.clone();
    let existing = db.voice_to_text_config().ok().flatten().unwrap_or_default();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_start(14);
    root.set_margin_end(14);
    root.set_margin_top(14);
    root.set_margin_bottom(14);

    let title = gtk::Label::new(Some("Voice to Text Configuration"));
    title.set_xalign(0.0);
    title.add_css_class("chat-profile-selector-title");
    root.append(&title);

    let backend_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let backend_label = gtk::Label::new(Some("Active backend"));
    backend_label.set_xalign(0.0);
    backend_label.set_width_chars(14);
    let backend_model = gtk::StringList::new(&["Local", "Cloud"]);
    let backend_dropdown = gtk::DropDown::new(Some(backend_model), None::<&gtk::Expression>);
    backend_dropdown.set_selected(if existing.provider == "cloud" { 1 } else { 0 });
    backend_row.append(&backend_label);
    backend_row.append(&backend_dropdown);
    root.append(&backend_row);

    let sections = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let sections_scroll = gtk::ScrolledWindow::new();
    sections_scroll.set_hexpand(true);
    sections_scroll.set_vexpand(true);
    sections_scroll.set_has_frame(false);
    sections_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sections_scroll.set_child(Some(&sections));
    root.append(&sections_scroll);

    let local_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    local_box.add_css_class("chat-profile-card");
    let local_title = gtk::Label::new(Some("Local"));
    local_title.set_xalign(0.0);
    local_title.add_css_class("chat-profile-card-title");
    local_box.append(&local_title);

    let whisper_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let whisper_label = gtk::Label::new(Some("Whisper command"));
    whisper_label.set_width_chars(14);
    whisper_label.set_xalign(0.0);
    let whisper_entry = gtk::Entry::new();
    whisper_entry.set_hexpand(true);
    whisper_entry.set_text(&existing.local_whisper_command);
    let whisper_check_button = gtk::Button::with_label("Check");
    whisper_check_button.add_css_class("app-flat-button");
    whisper_row.append(&whisper_label);
    whisper_row.append(&whisper_entry);
    whisper_row.append(&whisper_check_button);
    local_box.append(&whisper_row);

    let whisper_status = gtk::Label::new(Some(""));
    whisper_status.set_xalign(0.0);
    whisper_status.add_css_class("chat-profile-card-hint");
    local_box.append(&whisper_status);

    let local_model_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let local_model_label = gtk::Label::new(Some("Model"));
    local_model_label.set_xalign(0.0);
    local_model_label.set_width_chars(14);
    let local_model_entry = gtk::Entry::new();
    local_model_entry.set_hexpand(true);
    local_model_entry.set_text(existing.local_model_path.as_deref().unwrap_or(""));
    let local_model_browse = gtk::Button::with_label("Browse");
    local_model_browse.add_css_class("app-flat-button");
    local_model_row.append(&local_model_label);
    local_model_row.append(&local_model_entry);
    local_model_row.append(&local_model_browse);
    local_box.append(&local_model_row);

    let local_models_title = gtk::Label::new(Some("Downloaded models"));
    local_models_title.set_xalign(0.0);
    local_models_title.add_css_class("chat-profile-card-hint");
    local_box.append(&local_models_title);

    let local_models_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    local_box.append(&local_models_list);

    let local_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let local_download_tiny = gtk::Button::with_label("Download Whisper Tiny");
    local_download_tiny.add_css_class("app-flat-button");
    local_actions.append(&local_download_tiny);
    local_box.append(&local_actions);

    let local_test_status = gtk::Label::new(Some(""));
    local_test_status.set_xalign(0.0);
    local_test_status.add_css_class("chat-profile-card-hint");
    local_box.append(&local_test_status);
    sections.append(&local_box);

    let cloud_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    cloud_box.add_css_class("chat-profile-card");
    let cloud_title = gtk::Label::new(Some("Cloud"));
    cloud_title.set_xalign(0.0);
    cloud_title.add_css_class("chat-profile-card-title");
    cloud_box.append(&cloud_title);

    let cloud_provider_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cloud_provider_label = gtk::Label::new(Some("Provider"));
    cloud_provider_label.set_width_chars(14);
    cloud_provider_label.set_xalign(0.0);
    let cloud_provider_model = gtk::StringList::new(&["OpenAI", "Azure/AiFoundry"]);
    let cloud_provider_dropdown =
        gtk::DropDown::new(Some(cloud_provider_model), None::<&gtk::Expression>);
    cloud_provider_dropdown.set_selected(if existing.cloud_provider == "azure" {
        1
    } else {
        0
    });
    cloud_provider_row.append(&cloud_provider_label);
    cloud_provider_row.append(&cloud_provider_dropdown);
    cloud_box.append(&cloud_provider_row);

    let cloud_url_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cloud_url_label = gtk::Label::new(Some("Target URL"));
    cloud_url_label.set_width_chars(14);
    cloud_url_label.set_xalign(0.0);
    let cloud_url_entry = gtk::Entry::new();
    cloud_url_entry.set_hexpand(true);
    cloud_url_entry.set_text(existing.cloud_url.as_deref().unwrap_or(""));
    cloud_url_row.append(&cloud_url_label);
    cloud_url_row.append(&cloud_url_entry);
    cloud_box.append(&cloud_url_row);

    let cloud_key_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cloud_key_label = gtk::Label::new(Some("API key"));
    cloud_key_label.set_width_chars(14);
    cloud_key_label.set_xalign(0.0);
    let cloud_key_entry = gtk::PasswordEntry::new();
    cloud_key_entry.set_hexpand(true);
    cloud_key_entry.set_text(existing.cloud_api_key.as_deref().unwrap_or(""));
    cloud_key_row.append(&cloud_key_label);
    cloud_key_row.append(&cloud_key_entry);
    cloud_box.append(&cloud_key_row);

    let cloud_model_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cloud_model_label = gtk::Label::new(Some("Model"));
    cloud_model_label.set_width_chars(14);
    cloud_model_label.set_xalign(0.0);
    let cloud_model_entry = gtk::Entry::new();
    cloud_model_entry.set_hexpand(true);
    cloud_model_entry.set_text(
        existing
            .cloud_model
            .as_deref()
            .unwrap_or("gpt-4o-mini-transcribe"),
    );
    cloud_model_row.append(&cloud_model_label);
    cloud_model_row.append(&cloud_model_entry);
    cloud_box.append(&cloud_model_row);

    let cloud_test_button = gtk::Button::with_label("Test Cloud");
    cloud_test_button.add_css_class("app-flat-button");
    cloud_box.append(&cloud_test_button);

    let cloud_test_status = gtk::Label::new(Some(""));
    cloud_test_status.set_xalign(0.0);
    cloud_test_status.add_css_class("chat-profile-card-hint");
    cloud_box.append(&cloud_test_status);

    sections.append(&cloud_box);

    let update_backend_sections: Rc<dyn Fn()> = {
        let backend_dropdown = backend_dropdown.clone();
        let local_box = local_box.clone();
        let cloud_box = cloud_box.clone();
        Rc::new(move || {
            let cloud_selected = backend_dropdown.selected() == 1;
            local_box.set_visible(!cloud_selected);
            cloud_box.set_visible(cloud_selected);
        })
    };
    (update_backend_sections)();
    {
        let update_backend_sections = update_backend_sections.clone();
        backend_dropdown.connect_selected_notify(move |_| {
            (update_backend_sections)();
        });
    }

    let update_cloud_provider_rows: Rc<dyn Fn()> = {
        let cloud_provider_dropdown = cloud_provider_dropdown.clone();
        let cloud_model_row = cloud_model_row.clone();
        Rc::new(move || {
            let is_azure = cloud_provider_dropdown.selected() == 1;
            cloud_model_row.set_visible(!is_azure);
        })
    };
    (update_cloud_provider_rows)();
    {
        let update_cloud_provider_rows = update_cloud_provider_rows.clone();
        cloud_provider_dropdown.connect_selected_notify(move |_| {
            (update_cloud_provider_rows)();
        });
    }

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_halign(gtk::Align::End);
    let save_button = gtk::Button::with_label("Save");
    save_button.add_css_class("suggested-action");

    let dialog_status = gtk::Label::new(Some(""));
    dialog_status.set_xalign(0.0);
    dialog_status.add_css_class("chat-profile-card-hint");
    root.append(&dialog_status);

    footer.append(&save_button);
    root.append(&footer);

    let refresh_local_models_ui: Rc<dyn Fn()> = {
        let local_models_list = local_models_list.clone();
        let local_model_entry = local_model_entry.clone();
        Rc::new(move || {
            refresh_local_models_list_widget(&local_models_list, &local_model_entry);
        })
    };
    (refresh_local_models_ui)();

    {
        let local_model_entry = local_model_entry.clone();
        let refresh_local_models_ui = refresh_local_models_ui.clone();
        let local_test_status = local_test_status.clone();
        let dialog = dialog.clone();
        local_model_browse.connect_clicked(move |_| {
            let file_dialog = gtk::FileDialog::builder()
                .title("Select Whisper Model")
                .modal(true)
                .build();
            let local_test_status_for_open = local_test_status.clone();
            let local_model_entry_for_open = local_model_entry.clone();
            let refresh_local_models_ui_for_open = refresh_local_models_ui.clone();
            file_dialog.open(
                Some(&dialog),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let file = match result {
                        Ok(file) => file,
                        Err(err) => {
                            if !err.matches(gtk::gio::IOErrorEnum::Cancelled) {
                                local_test_status_for_open
                                    .set_text(&format!("Model selection failed: {err}"));
                            }
                            return;
                        }
                    };
                    if let Some(path) = file.path() {
                        local_model_entry_for_open.set_text(&path.to_string_lossy());
                        (refresh_local_models_ui_for_open)();
                    }
                },
            );
        });
    }

    {
        let local_test_status = local_test_status.clone();
        let local_download_tiny = local_download_tiny.clone();
        let local_model_entry = local_model_entry.clone();
        let refresh_local_models_ui = refresh_local_models_ui.clone();
        local_download_tiny.clone().connect_clicked(move |_| {
            local_test_status.set_text("Downloading whisper tiny model...");
            local_download_tiny.set_sensitive(false);
            let (tx, rx) = std::sync::mpsc::channel::<Result<PathBuf, String>>();
            std::thread::spawn(move || {
                let _ = tx.send(download_whisper_tiny_model());
            });

            let local_test_status = local_test_status.clone();
            let local_download_tiny = local_download_tiny.clone();
            let local_model_entry = local_model_entry.clone();
            let refresh_local_models_ui = refresh_local_models_ui.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
                Ok(Ok(path)) => {
                    local_model_entry.set_text(&path.to_string_lossy());
                    local_test_status.set_text("Whisper tiny downloaded.");
                    local_download_tiny.set_sensitive(true);
                    (refresh_local_models_ui)();
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    local_test_status.set_text(&format!("Download failed: {err}"));
                    local_download_tiny.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    local_test_status.set_text("Download failed: worker disconnected.");
                    local_download_tiny.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
            });
        });
    }

    {
        let whisper_entry = whisper_entry.clone();
        let whisper_status = whisper_status.clone();
        whisper_check_button.connect_clicked(move |_| {
            let command = whisper_entry.text().trim().to_string();
            if command.is_empty() {
                whisper_status.set_text("Whisper command is empty.");
                return;
            }
            whisper_status.set_text("Checking whisper command...");
            let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
            std::thread::spawn(move || {
                let _ = tx.send(check_whisper_command(&command));
            });

            let whisper_status = whisper_status.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
                Ok(Ok(msg)) => {
                    whisper_status.set_text(&msg);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    whisper_status.set_text(&format!("Check failed: {err}"));
                    gtk::glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    whisper_status.set_text("Check failed: worker disconnected.");
                    gtk::glib::ControlFlow::Break
                }
            });
        });
    }

    {
        let cloud_provider_dropdown = cloud_provider_dropdown.clone();
        let cloud_url_entry = cloud_url_entry.clone();
        let cloud_key_entry = cloud_key_entry.clone();
        let cloud_model_entry = cloud_model_entry.clone();
        let cloud_test_status = cloud_test_status.clone();
        cloud_test_button.connect_clicked(move |_| {
            let provider = if cloud_provider_dropdown.selected() == 1 {
                "azure".to_string()
            } else {
                "openai".to_string()
            };
            let url = cloud_url_entry.text().trim().to_string();
            let api_key = cloud_key_entry.text().trim().to_string();
            let model = cloud_model_entry.text().trim().to_string();
            cloud_test_status.set_text("Testing cloud endpoint...");
            let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
            std::thread::spawn(move || {
                let _ = tx.send(test_cloud_config(&provider, &url, &api_key, &model));
            });

            let cloud_test_status = cloud_test_status.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
                Ok(Ok(msg)) => {
                    cloud_test_status.set_text(&msg);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    cloud_test_status.set_text(&format!("Cloud test failed: {err}"));
                    gtk::glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    cloud_test_status.set_text("Cloud test failed: worker disconnected.");
                    gtk::glib::ControlFlow::Break
                }
            });
        });
    }

    {
        let db = db.clone();
        let dialog = dialog.clone();
        let on_saved = on_saved.clone();
        let backend_dropdown = backend_dropdown.clone();
        let whisper_entry = whisper_entry.clone();
        let local_model_entry = local_model_entry.clone();
        let cloud_provider_dropdown = cloud_provider_dropdown.clone();
        let cloud_url_entry = cloud_url_entry.clone();
        let cloud_key_entry = cloud_key_entry.clone();
        let cloud_model_entry = cloud_model_entry.clone();
        let dialog_status = dialog_status.clone();
        let should_close_on_save = close_on_save;
        save_button.connect_clicked(move |_| {
            let provider = if backend_dropdown.selected() == 1 {
                "cloud".to_string()
            } else {
                "local".to_string()
            };
            let local_model_path = local_model_entry.text().trim().to_string();
            let cloud_url = cloud_url_entry.text().trim().to_string();
            let cloud_api_key = cloud_key_entry.text().trim().to_string();
            let cloud_model = cloud_model_entry.text().trim().to_string();
            let is_azure = cloud_provider_dropdown.selected() == 1;
            let config = VoiceToTextConfig {
                provider,
                local_whisper_command: whisper_entry.text().trim().to_string(),
                local_model_path: if local_model_path.is_empty() {
                    None
                } else {
                    Some(local_model_path)
                },
                cloud_provider: if is_azure {
                    "azure".to_string()
                } else {
                    "openai".to_string()
                },
                cloud_url: if cloud_url.is_empty() {
                    None
                } else {
                    Some(cloud_url)
                },
                cloud_api_key: if cloud_api_key.is_empty() {
                    None
                } else {
                    Some(cloud_api_key)
                },
                cloud_model: if is_azure || cloud_model.is_empty() {
                    None
                } else {
                    Some(cloud_model)
                },
                updated_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            };
            if !config.is_valid() {
                dialog_status.set_text(
                    "Configuration is incomplete. Local needs a model path; Cloud needs URL and API key (model is required for OpenAI).",
                );
                return;
            }
            if let Err(err) = db.upsert_voice_to_text_config(&config) {
                dialog_status.set_text(&format!("Failed to save voice settings: {err}"));
                return;
            }
            if let Some(on_saved) = on_saved.as_ref() {
                on_saved(config);
            }
            if should_close_on_save {
                dialog.close();
            } else {
                dialog_status.set_text("Voice settings saved.");
            }
        });
    }

    root
}

pub(super) fn open_voice_settings_dialog(
    parent: Option<gtk::Window>,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
) {
    crate::ui::components::settings_dialog::show(
        parent.as_ref(),
        db,
        manager,
        crate::ui::components::settings_dialog::SettingsPage::VoiceInput,
    );
}

pub(super) fn start_voice_recording(audio_path: &Path) -> Result<Child, String> {
    let parent = audio_path
        .parent()
        .ok_or_else(|| "Invalid recording path.".to_string())?;
    std::fs::create_dir_all(parent).map_err(|err| format!("Failed to create temp dir: {err}"))?;

    Command::new("ffmpeg")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-f")
        .arg("pulse")
        .arg("-i")
        .arg("default")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(audio_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            format!(
                "Failed to start recording. Install ffmpeg and PulseAudio/PipeWire support ({err})"
            )
        })
}

pub(super) fn ensure_ffmpeg_available() -> Result<(), String> {
    let status = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| {
            "ffmpeg is required for voice input. Install ffmpeg and restart the app.".to_string()
        })?;
    if status.success() {
        Ok(())
    } else {
        Err("ffmpeg is required for voice input. Install ffmpeg and restart the app.".to_string())
    }
}

pub(super) fn stop_voice_recording(child: &mut Child) -> Result<(), String> {
    let pid = child.id() as i32;
    let signal_status = unsafe { libc::kill(pid, libc::SIGINT) };
    if signal_status != 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Failed to gracefully stop ffmpeg recording process.".to_string());
    }

    let graceful_deadline = std::time::Instant::now() + Duration::from_millis(1_500);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                if std::time::Instant::now() >= graceful_deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(err) => return Err(format!("Failed while waiting for ffmpeg to stop: {err}")),
        }
    }

    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
    let term_deadline = std::time::Instant::now() + Duration::from_millis(500);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                if std::time::Instant::now() >= term_deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => {
                return Err(format!(
                    "Failed while waiting for ffmpeg to terminate: {err}"
                ));
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Err(
        "ffmpeg recording process had to be force-stopped; recorded audio may be incomplete."
            .to_string(),
    )
}

pub(super) fn transcribe_audio(
    config: &VoiceToTextConfig,
    audio_path: &Path,
) -> Result<String, String> {
    match config.provider.as_str() {
        "cloud" => transcribe_cloud(config, audio_path),
        _ => transcribe_local(config, audio_path),
    }
}

pub(super) fn check_whisper_command(command: &str) -> Result<String, String> {
    let output = Command::new(command)
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| {
            format!(
                "Could not run `{command} --help`. Install Whisper and make sure `{command}` is available in PATH ({err})."
            )
        })?;
    if output.status.success() {
        Ok(format!("`{command}` is available."))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no error details available");
        Err(format!(
            "`{command}` failed the check. Verify it is a Whisper CLI command ({detail})."
        ))
    }
}

fn test_cloud_config(
    provider: &str,
    url: &str,
    api_key: &str,
    model: &str,
) -> Result<String, String> {
    if url.trim().is_empty() || api_key.trim().is_empty() {
        return Err("Cloud URL and API key are required.".to_string());
    }
    if provider != "azure" && model.trim().is_empty() {
        return Err("Model is required for OpenAI cloud transcription.".to_string());
    }
    let header_name = if provider == "azure" {
        "api-key"
    } else {
        "Authorization"
    };
    let header_value = if provider == "azure" {
        api_key.to_string()
    } else {
        format!("Bearer {api_key}")
    };
    let status = Command::new("curl")
        .arg("-sS")
        .arg("-I")
        .arg("--max-time")
        .arg("8")
        .arg("-H")
        .arg(format!("{header_name}: {header_value}"))
        .arg(url)
        .status()
        .map_err(|err| format!("Failed to run curl: {err}"))?;
    if status.success() {
        Ok("Cloud endpoint responded.".to_string())
    } else {
        Err("Cloud endpoint test failed. Check URL/API key.".to_string())
    }
}

fn transcribe_local(config: &VoiceToTextConfig, audio_path: &Path) -> Result<String, String> {
    let command = config.local_whisper_command.trim();
    if command.is_empty() {
        return Err("Local whisper command is empty.".to_string());
    }
    let model_spec = config
        .local_model_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Local model is missing.".to_string())?;

    let out_base = audio_path.with_extension("");
    let out_txt = out_base.with_extension("txt");
    let _ = std::fs::remove_file(&out_txt);

    let output_dir = out_base
        .parent()
        .ok_or_else(|| "Invalid transcription output directory.".to_string())?;

    let flavor = detect_local_whisper_flavor(command);
    let model_path = Path::new(model_spec);
    let model_exists = model_path.exists();
    let model_ext = model_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let looks_like_cpp_model = model_ext == "bin" || model_ext == "gguf";
    let looks_like_python_checkpoint =
        model_ext == "pt" || model_ext == "pth" || model_ext == "ckpt";

    let attempt_order = match flavor {
        LocalWhisperFlavor::Python => {
            if looks_like_cpp_model {
                [LocalWhisperFlavor::Python, LocalWhisperFlavor::Unknown]
            } else {
                [LocalWhisperFlavor::Python, LocalWhisperFlavor::Cpp]
            }
        }
        LocalWhisperFlavor::Cpp => [LocalWhisperFlavor::Cpp, LocalWhisperFlavor::Python],
        LocalWhisperFlavor::Unknown => {
            if looks_like_cpp_model {
                [LocalWhisperFlavor::Cpp, LocalWhisperFlavor::Python]
            } else if !model_exists || looks_like_python_checkpoint {
                [LocalWhisperFlavor::Python, LocalWhisperFlavor::Cpp]
            } else {
                [LocalWhisperFlavor::Cpp, LocalWhisperFlavor::Python]
            }
        }
    };

    let mut python_error: Option<String> = None;
    let mut cpp_error: Option<String> = None;

    for attempt in attempt_order {
        match attempt {
            LocalWhisperFlavor::Python => {
                match run_python_whisper_transcription(
                    command, model_spec, audio_path, output_dir, &out_txt,
                ) {
                    Ok(text) => return Ok(text),
                    Err(err) => python_error = Some(err),
                }
            }
            LocalWhisperFlavor::Cpp => {
                if !model_exists {
                    cpp_error = Some("whisper.cpp requires a local model file path.".to_string());
                    continue;
                }
                match run_cpp_whisper_transcription(
                    command, model_spec, audio_path, &out_base, &out_txt,
                ) {
                    Ok(text) => return Ok(text),
                    Err(err) => cpp_error = Some(err),
                }
            }
            LocalWhisperFlavor::Unknown => {}
        }
    }

    let mut details = Vec::new();
    if let Some(err) = python_error {
        details.push(format!("python whisper attempt: {err}"));
    }
    if let Some(err) = cpp_error {
        details.push(format!("whisper.cpp attempt: {err}"));
    }
    if details.is_empty() {
        return Err("Local whisper command failed.".to_string());
    }
    Err(format!(
        "Local transcription failed: {}",
        details.join(" | ")
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocalWhisperFlavor {
    Python,
    Cpp,
    Unknown,
}

fn detect_local_whisper_flavor(command: &str) -> LocalWhisperFlavor {
    let output = match Command::new(command)
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => output,
        Err(_) => return LocalWhisperFlavor::Unknown,
    };
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.stderr.is_empty() {
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let text = text.to_ascii_lowercase();
    if text.contains("audio [audio") && text.contains("--output_format") {
        return LocalWhisperFlavor::Python;
    }
    if text.contains("whisper.cpp")
        || text.contains("-otxt")
        || text.contains(" -m ")
        || text.contains(" --model ")
    {
        return LocalWhisperFlavor::Cpp;
    }
    LocalWhisperFlavor::Unknown
}

fn run_cpp_whisper_transcription(
    command: &str,
    model_path: &str,
    audio_path: &Path,
    out_base: &Path,
    out_txt: &Path,
) -> Result<String, String> {
    let output = Command::new(command)
        .arg("-m")
        .arg(model_path)
        .arg("-f")
        .arg(audio_path)
        .arg("-otxt")
        .arg("-of")
        .arg(out_base.to_string_lossy().to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("failed to run command: {err}"))?;
    if !output.status.success() || !out_txt.exists() {
        return Err(extract_stderr_summary(&output.stderr));
    }
    let text = std::fs::read_to_string(out_txt)
        .map_err(|err| format!("failed to read transcript: {err}"))?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return Err("transcript was empty".to_string());
    }
    Ok(trimmed)
}

fn run_python_whisper_transcription(
    command: &str,
    model: &str,
    audio_path: &Path,
    output_dir: &Path,
    out_txt: &Path,
) -> Result<String, String> {
    let model_arg = normalize_python_model_name(model);
    let output = Command::new(command)
        .arg(audio_path)
        .arg("--model")
        .arg(&model_arg)
        .arg("--output_format")
        .arg("txt")
        .arg("--output_dir")
        .arg(output_dir)
        .arg("--fp16")
        .arg("False")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("failed to run command: {err}"))?;
    if !output.status.success() || !out_txt.exists() {
        return Err(extract_stderr_summary(&output.stderr));
    }
    let text = std::fs::read_to_string(out_txt)
        .map_err(|err| format!("failed to read transcript: {err}"))?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return Err("transcript was empty".to_string());
    }
    Ok(trimmed)
}

fn normalize_python_model_name(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    let file_name = Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase())
        .unwrap_or_else(|| trimmed.to_ascii_lowercase());

    if file_name.contains("tiny") {
        return "tiny".to_string();
    }
    if file_name.contains("base") {
        return "base".to_string();
    }
    if file_name.contains("small") {
        return "small".to_string();
    }
    if file_name.contains("medium") {
        return "medium".to_string();
    }
    if file_name.contains("large-v3") {
        return "large-v3".to_string();
    }
    if file_name.contains("large-v2") {
        return "large-v2".to_string();
    }
    if file_name.contains("large-v1") {
        return "large-v1".to_string();
    }
    if file_name.contains("large") {
        return "large".to_string();
    }
    trimmed.to_string()
}

fn extract_stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text_lower = text.to_ascii_lowercase();
    if text_lower.contains("weights only load failed") || text_lower.contains("unpicklingerror") {
        return "model format is not compatible with this whisper command".to_string();
    }
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return "command failed".to_string();
    }

    if text_lower.contains("traceback (most recent call last)") {
        for line in lines.iter().rev() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("file ") || lower.starts_with("traceback ") {
                continue;
            }
            if lower.contains("error") || lower.contains("exception") {
                if lower.contains("no module named") {
                    return "python whisper dependencies are missing. Reinstall whisper and required Python packages.".to_string();
                }
                return (*line).to_string();
            }
        }
    }

    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("usage:") {
            continue;
        }
        if lower.contains("no module named") {
            return "python whisper dependencies are missing. Reinstall whisper and required Python packages.".to_string();
        }
        return (*line).to_string();
    }
    "command failed".to_string()
}

fn transcribe_cloud(config: &VoiceToTextConfig, audio_path: &Path) -> Result<String, String> {
    let url = config
        .cloud_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Cloud URL is missing.".to_string())?;
    let api_key = config
        .cloud_api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Cloud API key is missing.".to_string())?;
    let model = if config.cloud_provider == "azure" {
        None
    } else {
        Some(
            config
                .cloud_model
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "Cloud model is missing.".to_string())?,
        )
    };

    let mut cmd = Command::new("curl");
    cmd.arg("-sS").arg("--fail").arg("-X").arg("POST").arg(url);
    if config.cloud_provider == "azure" {
        cmd.arg("-H").arg(format!("api-key: {api_key}"));
    } else {
        cmd.arg("-H")
            .arg(format!("Authorization: Bearer {api_key}"));
    }
    cmd.arg("-F")
        .arg(format!("file=@{}", audio_path.to_string_lossy()));
    if let Some(model) = model {
        cmd.arg("-F").arg(format!("model={model}"));
    }

    let output = cmd
        .output()
        .map_err(|err| format!("Failed to run curl for cloud transcription: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Cloud transcription failed: {}", stderr.trim()));
    }
    let body = String::from_utf8_lossy(&output.stdout).to_string();
    let parsed = serde_json::from_str::<serde_json::Value>(&body).ok();
    let text = parsed
        .as_ref()
        .and_then(|json| json.get("text"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| body.trim().to_string());
    if text.trim().is_empty() {
        return Err("Cloud transcription returned empty text.".to_string());
    }
    Ok(text)
}

fn download_whisper_tiny_model() -> Result<PathBuf, String> {
    let target = tiny_model_path();
    if target.exists() {
        return Ok(target);
    }
    let parent = target
        .parent()
        .ok_or_else(|| "Invalid tiny model path.".to_string())?;
    std::fs::create_dir_all(parent).map_err(|err| format!("Failed to create model dir: {err}"))?;
    let status = Command::new("curl")
        .arg("-L")
        .arg("--fail")
        .arg("-o")
        .arg(&target)
        .arg(WHISPER_TINY_URL)
        .status()
        .map_err(|err| format!("Failed to run curl: {err}"))?;
    if !status.success() {
        return Err("Model download failed.".to_string());
    }
    Ok(target)
}

fn tiny_model_path() -> PathBuf {
    crate::data::default_app_data_dir()
        .join("voice_models")
        .join("ggml-tiny.bin")
}

fn refresh_local_models_list_widget(local_models_list: &gtk::Box, local_model_entry: &gtk::Entry) {
    while let Some(child) = local_models_list.first_child() {
        local_models_list.remove(&child);
    }

    let selected_path = local_model_entry.text().to_string();
    let local_models = list_local_models();
    if local_models.is_empty() {
        let empty = gtk::Label::new(Some("No downloaded local models yet."));
        empty.set_xalign(0.0);
        empty.add_css_class("chat-profile-card-hint");
        local_models_list.append(&empty);
        return;
    }

    for model_path in local_models {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&model_path));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        row.append(&label);

        let select_btn = gtk::Button::with_label(if model_path == selected_path {
            "Selected"
        } else {
            "Use"
        });
        if model_path == selected_path {
            select_btn.set_sensitive(false);
        } else {
            let local_model_entry = local_model_entry.clone();
            let local_models_list = local_models_list.clone();
            let path_for_select = model_path.clone();
            select_btn.connect_clicked(move |_| {
                local_model_entry.set_text(&path_for_select);
                refresh_local_models_list_widget(&local_models_list, &local_model_entry);
            });
        }
        row.append(&select_btn);
        local_models_list.append(&row);
    }
}

fn list_local_models() -> Vec<String> {
    let dir = crate::data::default_app_data_dir().join("voice_models");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| {
                    let ext = ext.to_ascii_lowercase();
                    ext == "bin" || ext == "gguf" || ext == "pt"
                })
                .unwrap_or(false)
        })
        .filter_map(|path| path.to_str().map(|value| value.to_string()))
        .collect();
    out.sort();
    out
}
