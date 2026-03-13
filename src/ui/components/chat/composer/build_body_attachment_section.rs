{
    {
        let add_menu_popover = add_menu_popover.clone();
        add_file.connect_clicked(move |_| {
            if add_menu_popover.is_visible() {
                add_menu_popover.popdown();
            } else {
                add_menu_popover.popup();
            }
        });
    }

    let picker_refresh: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let picker_activate_index: Rc<RefCell<Option<Rc<dyn Fn(usize)>>>> =
        Rc::new(RefCell::new(None));

    {
        let picker_refresh = picker_refresh.clone();
        let add_picker_popover = add_picker_popover.clone();
        let add_picker_entries = add_picker_entries.clone();
        let add_picker_root = add_picker_root.clone();
        let add_picker_current = add_picker_current.clone();
        let input_view = input_view.clone();
        let selected_mentions = selected_mentions.clone();
        let activate_entry: Rc<dyn Fn(usize)> = Rc::new(move |index| {
            let selected = { add_picker_entries.borrow().get(index).cloned() };
            let Some(selected) = selected else {
                return;
            };

            if selected.is_dir {
                add_picker_current.replace(Some(selected.path.clone()));
                if let Some(refresh) = picker_refresh.borrow().as_ref() {
                    refresh();
                }
                return;
            }

            let Some(root) = add_picker_root.borrow().clone() else {
                return;
            };
            let display = mention_display_for_path(&root, &selected.path, false);
            let path = selected.path.to_string_lossy().to_string();

            append_direct_mention(&input_view, &display);
            if !selected_mentions.borrow().iter().any(|m| m.path == path) {
                selected_mentions
                    .borrow_mut()
                    .push(MentionAttachment { display, path });
            }
            add_picker_popover.popdown();
            input_view.grab_focus();
        });
        picker_activate_index.replace(Some(activate_entry));
    }

    {
        let picker_activate_index = picker_activate_index.clone();
        let add_picker_list = add_picker_list.clone();
        let add_picker_back = add_picker_back.clone();
        let add_picker_path = add_picker_path.clone();
        let add_picker_entries = add_picker_entries.clone();
        let add_picker_cached_dir = add_picker_cached_dir.clone();
        let add_picker_cached_entries = add_picker_cached_entries.clone();
        let mention_files_root = mention_files_root.clone();
        let mention_files = mention_files.clone();
        let add_picker_root = add_picker_root.clone();
        let add_picker_current = add_picker_current.clone();
        let add_picker_query = add_picker_query.clone();
        let refresh_picker: Rc<dyn Fn()> = Rc::new(move || {
            let Some(root) = add_picker_root.borrow().clone() else {
                return;
            };
            let Some(current) = add_picker_current.borrow().clone() else {
                return;
            };
            let Some(activate_entry) = picker_activate_index.borrow().as_ref().cloned() else {
                return;
            };
            let query = add_picker_query.borrow().clone();
            refresh_add_picker_browser(
                &add_picker_list,
                &add_picker_path,
                &root,
                &current,
                &query,
                &add_picker_cached_dir,
                &add_picker_cached_entries,
                &mention_files_root,
                &mention_files,
                &add_picker_entries,
                &activate_entry,
            );
            add_picker_back.set_sensitive(current != root);
        });
        picker_refresh.replace(Some(refresh_picker));
    }

    {
        let picker_refresh = picker_refresh.clone();
        let add_menu_popover = add_menu_popover.clone();
        let add_picker_popover = add_picker_popover.clone();
        let add_picker_root = add_picker_root.clone();
        let add_picker_current = add_picker_current.clone();
        let add_picker_query = add_picker_query.clone();
        let add_picker_cached_dir = add_picker_cached_dir.clone();
        let add_picker_search = add_picker_search.clone();
        let mention_files = mention_files.clone();
        let mention_files_root = mention_files_root.clone();
        let active_workspace_path = active_workspace_path.clone();
        add_menu_file_button.connect_clicked(move |_| {
            ensure_mention_files_loaded(
                &active_workspace_path,
                &mention_files_root,
                &mention_files,
            );

            let Some(root) = resolve_workspace_root(&active_workspace_path) else {
                return;
            };
            add_picker_root.replace(Some(root.clone()));
            add_picker_current.replace(Some(root));
            add_picker_query.replace(String::new());
            add_picker_cached_dir.replace(None);
            add_picker_search.set_text("");
            if let Some(refresh) = picker_refresh.borrow().as_ref() {
                refresh();
            }

            add_menu_popover.popdown();
            add_picker_popover.popup();
            add_picker_search.grab_focus();
        });
    }

    {
        let add_menu_popover = add_menu_popover.clone();
        let add_file = add_file.clone();
        let selected_images = selected_images.clone();
        let image_preview_scroll = image_preview_scroll.clone();
        let image_preview_strip = image_preview_strip.clone();
        let send = send.clone();
        let input_view = input_view.clone();
        let thread_locked = thread_locked.clone();
        add_menu_image_button.connect_clicked(move |_| {
            add_menu_popover.popdown();

            let dialog = gtk::FileDialog::builder()
                .title("Attach Images")
                .modal(true)
                .build();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Images"));
            filter.add_mime_type("image/*");
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            dialog.set_default_filter(Some(&filter));

            let parent = add_file
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            let selected_images_for_cb = selected_images.clone();
            let image_preview_scroll_for_cb = image_preview_scroll.clone();
            let image_preview_strip_for_cb = image_preview_strip.clone();
            let send_for_cb = send.clone();
            let input_view_for_cb = input_view.clone();
            let thread_locked_for_cb = thread_locked.clone();
            dialog.open_multiple(
                parent.as_ref(),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let Ok(files) = result else {
                        return;
                    };
                    let mut paths = Vec::new();
                    for idx in 0..files.n_items() {
                        let Some(item) = files.item(idx) else {
                            continue;
                        };
                        let Ok(file) = item.downcast::<gtk::gio::File>() else {
                            continue;
                        };
                        if let Some(path) = file.path() {
                            paths.push(path);
                        }
                    }
                    if add_image_attachments(&selected_images_for_cb, &paths) > 0 {
                        refresh_image_preview_strip(
                            &image_preview_scroll_for_cb,
                            &image_preview_strip_for_cb,
                            &selected_images_for_cb,
                            &send_for_cb,
                            &input_view_for_cb,
                            &thread_locked_for_cb,
                        );
                    }
                },
            );
        });
    }

    {
        let picker_refresh = picker_refresh.clone();
        let add_picker_root = add_picker_root.clone();
        let add_picker_current = add_picker_current.clone();
        add_picker_back.connect_clicked(move |_| {
            let Some(root) = add_picker_root.borrow().clone() else {
                return;
            };
            let Some(current) = add_picker_current.borrow().clone() else {
                return;
            };
            if current == root {
                return;
            }

            let Some(parent) = current.parent().map(Path::to_path_buf) else {
                return;
            };
            if !parent.starts_with(&root) {
                return;
            }

            add_picker_current.replace(Some(parent.clone()));
            if let Some(refresh) = picker_refresh.borrow().as_ref() {
                refresh();
            }
        });
    }

    {
        let add_picker_popover = add_picker_popover.clone();
        let add_picker_root = add_picker_root.clone();
        let add_picker_current = add_picker_current.clone();
        let input_view = input_view.clone();
        let selected_mentions = selected_mentions.clone();
        add_current_folder_button.connect_clicked(move |_| {
            let Some(root) = add_picker_root.borrow().clone() else {
                return;
            };
            let Some(current) = add_picker_current.borrow().clone() else {
                return;
            };

            let display = mention_display_for_path(&root, &current, true);
            let path = current.to_string_lossy().to_string();

            append_direct_mention(&input_view, &display);
            if !selected_mentions.borrow().iter().any(|m| m.path == path) {
                selected_mentions
                    .borrow_mut()
                    .push(MentionAttachment { display, path });
            }
            add_picker_popover.popdown();
            input_view.grab_focus();
        });
    }

    {
        let picker_refresh = picker_refresh.clone();
        let add_picker_query = add_picker_query.clone();
        let search_refresh_source: Rc<RefCell<Option<gtk::glib::SourceId>>> =
            Rc::new(RefCell::new(None));
        let search_refresh_source_for_cb = search_refresh_source.clone();
        add_picker_search.connect_search_changed(move |entry| {
            add_picker_query.replace(entry.text().to_string());
            if let Some(source) = search_refresh_source_for_cb.borrow_mut().take() {
                source.remove();
            }
            let picker_refresh = picker_refresh.clone();
            let search_refresh_source = search_refresh_source_for_cb.clone();
            let source = gtk::glib::timeout_add_local(Duration::from_millis(90), move || {
                if let Some(refresh) = picker_refresh.borrow().as_ref() {
                    refresh();
                }
                search_refresh_source.borrow_mut().take();
                gtk::glib::ControlFlow::Break
            });
            search_refresh_source_for_cb.replace(Some(source));
        });
    }

    {
        let picker_activate_index = picker_activate_index.clone();
        add_picker_search.connect_activate(move |_| {
            if let Some(activate_entry) = picker_activate_index.borrow().as_ref() {
                activate_entry(0);
            }
        });
    }

    {
        let selected_images = selected_images.clone();
        let image_preview_scroll = image_preview_scroll.clone();
        let image_preview_strip = image_preview_strip.clone();
        let send = send.clone();
        let input_view = input_view.clone();
        let input_view_for_drop = input_view.clone();
        let thread_locked = thread_locked.clone();
        let drop_target_files = gtk::DropTarget::new(
            gtk::gdk::FileList::static_type(),
            gtk::gdk::DragAction::COPY,
        );
        drop_target_files.set_preload(true);
        drop_target_files.set_propagation_phase(gtk::PropagationPhase::Capture);
        drop_target_files.connect_drop(move |_, value, _, _| {
            let Ok(file_list) = value.get::<gtk::gdk::FileList>() else {
                return false;
            };
            let mut paths = Vec::new();
            for file in file_list.files() {
                if let Some(path) = file.path() {
                    paths.push(path);
                }
            }
            if add_image_attachments(&selected_images, &paths) > 0 {
                refresh_image_preview_strip(
                    &image_preview_scroll,
                    &image_preview_strip,
                    &selected_images,
                    &send,
                    &input_view_for_drop,
                    &thread_locked,
                );
                true
            } else {
                false
            }
        });
        input_view.add_controller(drop_target_files);
    }

    {
        let selected_images = selected_images.clone();
        let image_preview_scroll = image_preview_scroll.clone();
        let image_preview_strip = image_preview_strip.clone();
        let send = send.clone();
        let input_view = input_view.clone();
        let input_view_for_drop = input_view.clone();
        let thread_locked = thread_locked.clone();
        let drop_target_text =
            gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::COPY);
        drop_target_text.set_preload(true);
        drop_target_text.set_propagation_phase(gtk::PropagationPhase::Capture);
        drop_target_text.connect_drop(move |_, value, _, _| {
            let Ok(raw) = value.get::<String>() else {
                return false;
            };
            let paths = parse_uri_list_paths(&raw);
            if add_image_attachments(&selected_images, &paths) > 0 {
                refresh_image_preview_strip(
                    &image_preview_scroll,
                    &image_preview_strip,
                    &selected_images,
                    &send,
                    &input_view_for_drop,
                    &thread_locked,
                );
                true
            } else {
                false
            }
        });
        input_view.add_controller(drop_target_text);
    }

    {
        let selected_images = selected_images.clone();
        let image_preview_scroll = image_preview_scroll.clone();
        let image_preview_strip = image_preview_strip.clone();
        let send = send.clone();
        let input_view = input_view.clone();
        let thread_locked = thread_locked.clone();
        buffer.connect_paste_done(move |_, clipboard| {
            let selected_images_from_texture = selected_images.clone();
            let image_preview_scroll_from_texture = image_preview_scroll.clone();
            let image_preview_strip_from_texture = image_preview_strip.clone();
            let send_from_texture = send.clone();
            let input_view_from_texture = input_view.clone();
            let thread_locked_from_texture = thread_locked.clone();
            clipboard.read_texture_async(None::<&gtk::gio::Cancellable>, move |result| {
                let Ok(Some(texture)) = result else {
                    return;
                };
                let Ok(dir) = ensure_composer_image_dir() else {
                    return;
                };
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let target = dir.join(format!("pasted-{timestamp}.png"));
                if texture.save_to_png(&target).is_err() {
                    return;
                }
                if add_image_attachments(&selected_images_from_texture, &[target]) > 0 {
                    refresh_image_preview_strip(
                        &image_preview_scroll_from_texture,
                        &image_preview_strip_from_texture,
                        &selected_images_from_texture,
                        &send_from_texture,
                        &input_view_from_texture,
                        &thread_locked_from_texture,
                    );
                }
            });

            let selected_images_from_text = selected_images.clone();
            let image_preview_scroll_from_text = image_preview_scroll.clone();
            let image_preview_strip_from_text = image_preview_strip.clone();
            let send_from_text = send.clone();
            let input_view_from_text = input_view.clone();
            let thread_locked_from_text = thread_locked.clone();
            clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
                let Ok(Some(raw_text)) = result else {
                    return;
                };
                let paths = parse_uri_list_paths(raw_text.as_str());
                if add_image_attachments(&selected_images_from_text, &paths) > 0 {
                    refresh_image_preview_strip(
                        &image_preview_scroll_from_text,
                        &image_preview_strip_from_text,
                        &selected_images_from_text,
                        &send_from_text,
                        &input_view_from_text,
                        &thread_locked_from_text,
                    );
                }
            });
        });
    }

    {
        let input_scroll = input_scroll.clone();
        let input_view = input_view.clone();
        let placeholder = placeholder.clone();
        let messages_box = messages_box.clone();
        let messages_scroll = messages_scroll.clone();
        let send = send.clone();
        let mention_popover = mention_popover.clone();
        let mention_listbox = mention_listbox.clone();
        let filtered_mentions = filtered_mentions.clone();
        let mention_files = mention_files.clone();
        let mention_files_root = mention_files_root.clone();
        let active_workspace_path = active_workspace_path.clone();
        let suggestion_row = suggestion_row.clone();
        let selected_mentions = selected_mentions.clone();
        let selected_images = selected_images.clone();
        let thread_locked = thread_locked.clone();
        buffer.connect_changed(move |buf| {
            let start = buf.start_iter();
            let end = buf.end_iter();
            let text = buf.text(&start, &end, true);
            let is_empty = text.is_empty();
            placeholder.set_visible(is_empty);

            if is_empty {
                selected_mentions.borrow_mut().clear();
            }

            update_send_button_active_state(
                &send,
                &input_view,
                &selected_images,
                *thread_locked.borrow(),
            );

            update_input_height(&input_scroll, &input_view, min_height, max_height);
            let follow_messages_while_typing = suggestion_row.parent().is_some();
            if follow_messages_while_typing && messages_box.first_child().is_some() {
                super::message_render::scroll_to_bottom(&messages_scroll);
            }

            if mention_query_range(&input_view).is_some() {
                ensure_mention_files_loaded(
                    &active_workspace_path,
                    &mention_files_root,
                    &mention_files,
                );
                refresh_mention_popup(
                    &mention_popover,
                    &mention_listbox,
                    &filtered_mentions,
                    &mention_files,
                    &input_view,
                );
            } else {
                mention_popover.popdown();
            }
        });
    }

    let key_controller = gtk::EventControllerKey::new();
    {
        let send = send.clone();
        let mention_popover = mention_popover.clone();
        let mention_listbox = mention_listbox.clone();
        let mention_scroll = mention_scroll.clone();
        let filtered_mentions = filtered_mentions.clone();
        let input_view_for_mentions = input_view.clone();
        let selected_mentions = selected_mentions.clone();
        let selected_images = selected_images.clone();
        let image_preview_scroll = image_preview_scroll.clone();
        let image_preview_strip = image_preview_strip.clone();
        let send_for_paste = send.clone();
        let input_view_for_paste = input_view.clone();
        let thread_locked_for_paste = thread_locked.clone();
        key_controller.connect_key_pressed(move |_, key, _, state| {
            if mention_popover.is_visible() {
                if key == gtk::gdk::Key::Down {
                    move_mention_selection(&mention_listbox, &mention_scroll, 1);
                    return gtk::glib::Propagation::Stop;
                }
                if key == gtk::gdk::Key::Up {
                    move_mention_selection(&mention_listbox, &mention_scroll, -1);
                    return gtk::glib::Propagation::Stop;
                }
                if key == gtk::gdk::Key::Escape {
                    mention_popover.popdown();
                    return gtk::glib::Propagation::Stop;
                }
                let is_enter_popup = key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter;
                if is_enter_popup {
                    let row = mention_listbox
                        .selected_row()
                        .or_else(|| mention_listbox.row_at_index(0));
                    if let Some(row) = row {
                        let index = row.index();
                        let selected = { filtered_mentions.borrow().get(index as usize).cloned() };
                        if let Some((display, path)) = selected {
                            let _ = insert_selected_mention(&input_view_for_mentions, &display);
                            mention_popover.popdown();
                            if !selected_mentions.borrow().iter().any(|m| m.path == path) {
                                selected_mentions
                                    .borrow_mut()
                                    .push(MentionAttachment { display, path });
                            }
                            return gtk::glib::Propagation::Stop;
                        }
                    }
                }
            }

            let is_paste = (key == gtk::gdk::Key::v || key == gtk::gdk::Key::V)
                && state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            if is_paste {
                let clipboard = input_view_for_paste.clipboard();
                let formats = clipboard.formats();
                let has_image_clipboard = formats.contains_type(gtk::gdk::Texture::static_type())
                    || formats.contain_mime_type("image/png")
                    || formats.contain_mime_type("image/jpeg")
                    || formats.contain_mime_type("image/webp")
                    || formats.contain_mime_type("image/gif")
                    || formats.contain_mime_type("image/bmp")
                    || formats.contain_mime_type("image/tiff");
                if has_image_clipboard {
                    let selected_images_for_texture = selected_images.clone();
                    let image_preview_scroll_for_texture = image_preview_scroll.clone();
                    let image_preview_strip_for_texture = image_preview_strip.clone();
                    let send_for_texture = send_for_paste.clone();
                    let input_view_for_texture = input_view_for_paste.clone();
                    let thread_locked_for_texture = thread_locked_for_paste.clone();
                    clipboard.read_texture_async(None::<&gtk::gio::Cancellable>, move |result| {
                        let Ok(Some(texture)) = result else {
                            return;
                        };
                        let Ok(dir) = ensure_composer_image_dir() else {
                            return;
                        };
                        let timestamp = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis();
                        let target = dir.join(format!("pasted-{timestamp}.png"));
                        if texture.save_to_png(&target).is_err() {
                            return;
                        }
                        if add_image_attachments(&selected_images_for_texture, &[target]) > 0 {
                            refresh_image_preview_strip(
                                &image_preview_scroll_for_texture,
                                &image_preview_strip_for_texture,
                                &selected_images_for_texture,
                                &send_for_texture,
                                &input_view_for_texture,
                                &thread_locked_for_texture,
                            );
                        }
                    });
                    return gtk::glib::Propagation::Stop;
                }
            }

            let is_enter = key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter;
            if is_enter && !state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                send.emit_clicked();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
    }
    input_view.add_controller(key_controller);
}
