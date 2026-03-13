fn build_panes_scroll(panes_row: &gtk::Box) -> gtk::ScrolledWindow {
    let panes_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(panes_row)
        .build();
    panes_scroll.set_widget_name("multi-chat-panes-scroll");
    panes_scroll.set_has_frame(false);
    panes_scroll.set_overlay_scrolling(false);
    panes_scroll.set_overflow(gtk::Overflow::Hidden);
    {
        let hadj = panes_scroll.hadjustment();
        let start_value = Rc::new(RefCell::new(0.0_f64));
        let drag = gtk::GestureDrag::builder().button(2).build();
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let hadj = hadj.clone();
            let start_value = start_value.clone();
            drag.connect_drag_begin(move |gesture, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                start_value.replace(hadj.value());
            });
        }
        {
            let hadj = hadj.clone();
            let start_value = start_value.clone();
            drag.connect_drag_update(move |_, offset_x, _| {
                let upper = hadj.upper() - hadj.page_size();
                let lower = hadj.lower();
                let next = (*start_value.borrow() - offset_x).clamp(lower, upper.max(lower));
                hadj.set_value(next);
            });
        }
        panes_scroll.add_controller(drag);
    }
    panes_scroll
}

fn build_drop_slot() -> gtk::Box {
    let drop_slot = gtk::Box::new(gtk::Orientation::Vertical, 8);
    drop_slot.add_css_class("multi-chat-drop-slot");
    drop_slot.set_halign(gtk::Align::Fill);
    drop_slot.set_valign(gtk::Align::Fill);
    drop_slot.set_hexpand(false);
    drop_slot.set_vexpand(true);
    drop_slot.set_size_request(220, -1);
    drop_slot.set_visible(false);

    let drop_plus = gtk::Label::new(Some("+"));
    drop_plus.add_css_class("multi-chat-drop-plus");
    drop_slot.append(&drop_plus);
    drop_slot
}

fn build_shared_composer_holder() -> gtk::Box {
    let composer_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    composer_holder.add_css_class("multi-chat-shared-composer");
    composer_holder.set_halign(gtk::Align::Fill);
    composer_holder.set_valign(gtk::Align::End);
    composer_holder.set_hexpand(true);
    composer_holder.set_vexpand(false);
    composer_holder.set_margin_start(0);
    composer_holder.set_margin_end(0);
    composer_holder.set_margin_top(0);
    composer_holder.set_margin_bottom(0);
    composer_holder
}
