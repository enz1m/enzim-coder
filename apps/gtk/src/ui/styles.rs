use gtk::gdk;

const PALETTE_CSS: &str = include_str!("styles/palette.css");

const GLOBAL_CSS: &str = r#"
    scrollbar slider,
    scale slider {
      min-width: 6px;
      min-height: 6px;
    }

    /* Keep muted helper labels consistent across host themes/Flatpak runtimes. */
    label.dim-label,
    .dim-label {
      color: alpha(@window_fg_color, 0.66);
      opacity: 1;
    }

    window:backdrop label.dim-label,
    window:backdrop .dim-label {
      color: alpha(@window_fg_color, 0.58);
      opacity: 1;
    }

    /* Ensure placeholder text stays readable on custom surfaces in Flatpak. */
    entry placeholder,
    entry text placeholder {
      color: alpha(@window_fg_color, 0.52);
      opacity: 1;
    }

    /* Keep DropDown labels/menu items readable on custom dark surfaces. */
    dropdown,
    dropdown > button,
    dropdown > button > box,
    dropdown > button > box > label,
    dropdown > button > box > arrow {
      color: alpha(@window_fg_color, 0.94);
    }

    popover.menu modelbutton,
    popover.menu modelbutton > box > label,
    popover.menu listview row,
    popover.menu listview row label,
    popover.menu menuitem,
    popover.menu menuitem label,
    popover listview row,
    popover listview row label {
      color: alpha(@window_fg_color, 0.94);
    }
"#;

const BACKDROP_FALLBACK_CSS: &str = r#"
    .no-backdrop-blur .composer-floating,
    .no-backdrop-blur .chat-worktree-overlay,
    .no-backdrop-blur .chat-queued-card,
    .no-backdrop-blur .file-preview-card,
    .no-backdrop-blur .sidebar-action-button,
    .no-backdrop-blur .scroll-down-button,
    .no-backdrop-blur .remote-mode-overlay,
    .no-backdrop-blur .welcome-overlay {
      backdrop-filter: none;
    }

    .no-backdrop-blur .composer-floating {
      background: alpha(@view_bg_color, 0.82);
    }

    .no-backdrop-blur .chat-worktree-overlay {
      background: alpha(@view_bg_color, 0.86);
    }

    .no-backdrop-blur .chat-queued-card {
      background: alpha(@view_bg_color, 0.8);
    }

    .no-backdrop-blur .file-preview-card {
      background: alpha(@view_bg_color, 0.94);
    }

    .no-backdrop-blur .sidebar-action-button {
      background-color: alpha(@window_fg_color, 0.14);
    }

    .no-backdrop-blur .sidebar-action-button:hover {
      background-color: alpha(@window_fg_color, 0.18);
    }

    .no-backdrop-blur .sidebar-action-button:active {
      background-color: alpha(@window_fg_color, 0.22);
    }

    .no-backdrop-blur .scroll-down-button {
      background: alpha(@view_bg_color, 0.2);
    }

    .no-backdrop-blur .scroll-down-button:hover {
      background: alpha(@view_bg_color, 0.26);
    }

    .no-backdrop-blur .remote-mode-overlay {
      background: alpha(#2a5ea8, 0.56);
    }

    .no-backdrop-blur .welcome-overlay {
      background: alpha(#757b86, 0.34);
    }

    .no-backdrop-blur .welcome-overlay.guide-mode {
      background: alpha(#757b86, 0.22);
    }

    .no-backdrop-blur .welcome-overlay.is-dismissing {
      background: alpha(#757b86, 0.0);
    }
"#;

const USER_OVERRIDE_CSS: &str = r#"
    #workspace-thread-list,
    #workspace-thread-list > *,
    #workspace-thread-listbox,
    #workspace-thread-listbox > row.thread-row,
    #workspace-thread-listbox > row.thread-row:hover,
    #workspace-thread-listbox > row.thread-row:selected,
    #workspace-thread-listbox > row.thread-row.thread-row-selected,
    #workspace-thread-listbox > row.thread-row:active,
    #workspace-thread-listbox > row.thread-row:focus,
    #workspace-thread-listbox > row.thread-row:focus-visible {
      background: transparent;
      background-color: transparent;
      background-image: none;
      border: none;
      box-shadow: none;
      outline: none;
    }

    #workspace-thread-listbox > row.thread-row,
    #workspace-thread-listbox > row.thread-row:hover,
    #workspace-thread-listbox > row.thread-row:selected,
    #workspace-thread-listbox > row.thread-row.thread-row-selected,
    #workspace-thread-listbox > row.thread-row:active {
      margin: 2px 0;
      padding: 0;
      border-radius: 0;
      min-height: 0;
    }

    #workspace-thread-listbox > row.thread-row > box.thread-row-content {
      margin: 0;
    }

    button#top-window-close-button,
    button#top-window-close-button:hover,
    button#top-window-close-button:active,
    button#top-window-close-button:focus,
    button#top-window-close-button:focus-visible {
      min-height: 24px;
      min-width: 24px;
      padding: 0;
      margin: 0;
      border: none;
      border-radius: 999px;
      background: alpha(@window_fg_color, 0.08);
      background-color: alpha(@window_fg_color, 0.08);
      background-image: none;
      box-shadow: none;
      outline: none;
      color: alpha(@window_fg_color, 0.78);
    }

    button#top-window-close-button:hover,
    button#top-window-close-button:active,
    button#top-window-close-button:focus,
    button#top-window-close-button:focus-visible {
      background: alpha(@window_fg_color, 0.16);
      background-color: alpha(@window_fg_color, 0.16);
      color: alpha(@window_fg_color, 0.96);
    }

    image#top-window-close-icon,
    button#top-window-close-button image {
      color: inherit;
      -gtk-icon-style: symbolic;
      opacity: 1;
    }
"#;

const COMPONENT_STYLES: &[&str] = &[
    include_str!("styles/components/actions_popover.css"),
    include_str!("styles/components/buttons.css"),
    include_str!("styles/components/chat.css"),
    include_str!("styles/components/chat_messages.css"),
    include_str!("styles/components/composer.css"),
    include_str!("styles/components/bottom_bar.css"),
    include_str!("styles/components/file_browser.css"),
    include_str!("styles/components/git_tab.css"),
    include_str!("styles/components/multi_chat.css"),
    include_str!("styles/components/profile_selector.css"),
    include_str!("styles/components/remote.css"),
    include_str!("styles/components/restore_preview.css"),
    include_str!("styles/components/settings.css"),
    include_str!("styles/components/skills_mcp_editor.css"),
    include_str!("styles/components/sidebar.css"),
    include_str!("styles/components/style_picker.css"),
    include_str!("styles/components/top_bar.css"),
    include_str!("styles/components/thread_list.css"),
    include_str!("styles/components/welcome.css"),
];

fn register_css(display: &gdk::Display, css: &str, priority: u32) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(css);
    gtk::style_context_add_provider_for_display(display, &provider, priority);
}

pub fn install_css() {
    if let Some(display) = gdk::Display::default() {
        let mut combined_css = String::from(PALETTE_CSS);
        combined_css.push('\n');
        combined_css.push_str(GLOBAL_CSS);
        combined_css.push('\n');
        for component_css in COMPONENT_STYLES {
            combined_css.push_str(component_css);
            combined_css.push('\n');
        }
        register_css(
            &display,
            &combined_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        register_css(
            &display,
            BACKDROP_FALLBACK_CSS,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        register_css(
            &display,
            USER_OVERRIDE_CSS,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}
