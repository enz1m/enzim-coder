{
    match method {
                "serverRequest/resolved" => {
                    if let Some(request_id) = event.params.get("requestId").and_then(Value::as_i64)
                    {
                        if let Some(pending) = pending_server_requests_by_id
                            .borrow_mut()
                            .remove(&request_id)
                        {
                            remove_request_card(&turn_uis, &pending.turn_id, &pending.card);
                            remove_persisted_pending_request(
                                &db,
                                &cached_pending_requests_for_thread,
                                &pending.thread_id,
                                request_id,
                            );
                            if let Some(turn_ui) = turn_uis.borrow_mut().get_mut(&pending.turn_id) {
                                if turn_ui.in_progress {
                                    super::refresh_turn_status(turn_ui);
                                }
                            }
                        }
                        if let Some(thread_id) = pending_request_thread_by_id
                            .borrow_mut()
                            .remove(&request_id)
                        {
                            remove_persisted_pending_request(
                                &db,
                                &cached_pending_requests_for_thread,
                                &thread_id,
                                request_id,
                            );
                        }
                    }
                }
        _ => {}
    }
}
