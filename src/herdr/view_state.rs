use crate::app::App;
use serde_json::{Value, json};

// Selection also identifies the sidebar's inline expanded file list.
// OIDs anchor the selection and viewport when newer commits arrive.
pub(super) fn snapshot(app: &App) -> Value {
    json!({
        "selected_oid": app.selected_oid(),
        "selected": app.selected,
        "top_oid": app.repo.commits.get(app.top).map(|commit| commit.oid.as_str()),
        "top": app.top,
        "top_offset": app.saved_row_offset(),
        "graph_offset": app.graph_offset,
        "query": app.query,
        "searching": app.searching,
    })
}

pub(super) fn restore(app: &mut App, state: &Value) {
    if !state.is_object() {
        return;
    }
    let index = |key: &str| {
        state
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
    };
    let anchored_index = |oid_key: &str, index_key: &str| {
        state
            .get(oid_key)
            .and_then(Value::as_str)
            .and_then(|oid| app.repo.commits.iter().position(|commit| commit.oid == oid))
            .or_else(|| index(index_key))
    };
    let last = app.repo.commits.len().checked_sub(1);
    let selected = if state.get("selected").is_some_and(Value::is_null) {
        None
    } else {
        anchored_index("selected_oid", "selected")
            .or(app.selected)
            .and_then(|selected| last.map(|last| selected.min(last)))
    };
    let top = anchored_index("top_oid", "top")
        .unwrap_or(app.top)
        .min(app.repo.commits.len().saturating_sub(1));
    if app.selected != selected || selected.is_some() {
        app.selected = selected;
        // Rebuild the selected commit's file list asynchronously, including when
        // restoring over the same selection; file contents are not persisted.
        app.selection_changed();
    }
    if app.sidebar {
        app.show_details = false;
    }
    // Restore an offset inside a file list after its asynchronous load completes.
    app.top = top;
    app.restore_row_offset(index("top_offset").unwrap_or(0));
    app.graph_offset = index("graph_offset")
        .unwrap_or(app.graph_offset)
        .min(app.graph.width.saturating_sub(1).saturating_mul(3));
    if let Some(query) = state.get("query").and_then(Value::as_str) {
        app.query = query.to_owned();
    }
    if let Some(searching) = state.get("searching").and_then(Value::as_bool) {
        app.searching = searching;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;

    fn demo() -> App {
        App::new(git::demo(), 2000, true).with_sidebar(true)
    }

    #[test]
    fn demo_selection_viewport_search_and_pan_roundtrip() {
        for searching in [false, true] {
            let mut original = demo();
            original.selected = Some(7);
            original.top = 5;
            original.top_offset = 1;
            original.graph_offset = 3;
            original.query = "한글 author".into();
            original.searching = searching;
            original.selection_changed();
            let state = snapshot(&original);
            assert_eq!(state["selected_oid"], original.repo.commits[7].oid);
            assert_eq!(state["top_oid"], original.repo.commits[5].oid);
            assert_eq!(state["top_offset"], 1);
            assert!(state.get("reference").is_none());
            assert!(state.get("show_details").is_none());

            let mut restored = demo();
            restore(&mut restored, &state);
            assert_eq!(restored.selected_oid(), original.selected_oid());
            assert_eq!(restored.top, original.top);
            assert_eq!(restored.top_offset, original.top_offset);
            assert_eq!(restored.graph_offset, original.graph_offset);
            assert_eq!(restored.query, original.query);
            assert_eq!(restored.searching, searching);
            assert!(!restored.show_details);
            assert_eq!(snapshot(&restored), state);
        }
    }

    #[test]
    fn unselected_sidebar_roundtrips_its_independent_viewport() {
        let mut original = demo();
        assert_eq!(original.selected, None);
        original.top = 6;
        original.top_offset = 1;
        original.graph_offset = 3;
        let state = snapshot(&original);
        assert!(state["selected"].is_null());
        assert!(state["selected_oid"].is_null());
        assert_eq!(state["top_oid"], original.repo.commits[6].oid);

        let mut restored = demo();
        restored.selected = Some(2);
        restored.selection_changed();
        restore(&mut restored, &state);
        assert_eq!(restored.selected, None);
        assert_eq!(restored.selected_oid(), None);
        assert!(restored.files.is_none());
        assert!(!restored.show_details);
        assert_eq!(restored.top, 6);
        assert_eq!(restored.top_offset, 1);
        assert_eq!(snapshot(&restored), state);
    }

    #[test]
    fn explicit_null_selection_overrides_a_stale_oid_without_moving_the_viewport() {
        let mut app = demo();
        app.selected = Some(3);
        app.selection_changed();
        let oid = app.repo.commits[3].oid.clone();
        restore(
            &mut app,
            &json!({"selected": null, "selected_oid": oid, "top": 8}),
        );
        assert_eq!(app.selected, None);
        assert_eq!(app.top, 8);
    }

    #[test]
    fn legacy_integer_and_oid_only_selection_states_are_supported() {
        let mut app = demo();
        let oid = app.repo.commits[4].oid.clone();
        for state in [
            json!({"selected": 4, "top": 8}),
            json!({"selected_oid": oid, "top": 8}),
        ] {
            app.selected = None;
            app.top_offset = 7;
            restore(&mut app, &state);
            assert_eq!(app.selected, Some(4));
            assert_eq!(app.top, 8);
            assert_eq!(app.top_offset, 0);
        }
    }

    #[test]
    fn legacy_details_visibility_never_opens_a_sidebar_inspector() {
        let mut app = demo();
        assert!(snapshot(&app).get("show_details").is_none());
        restore(&mut app, &json!({"selected": 3, "show_details": true}));
        assert_eq!(app.selected, Some(3));
        assert!(!app.show_details);
    }

    #[test]
    fn restoring_the_same_selection_marks_its_file_list_for_async_reload() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        app.selected = Some(4);
        app.top = 4;
        app.top_offset = 27;
        app.files = Some(Err("stale file list".into()));
        let state = snapshot(&app);
        assert!(state.get("files").is_none());

        restore(&mut app, &state);
        assert_eq!(app.selected, Some(4));
        assert_eq!(app.top, 4);
        assert_eq!(app.top_offset, 0);
        assert!(app.files.is_none());
        assert!(!app.show_details);
        assert_eq!(snapshot(&app), state);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(32, 12)).unwrap();
        terminal
            .draw(|frame| {
                crate::ui::draw(frame, &mut app, false);
            })
            .unwrap();
        assert_eq!(snapshot(&app), state);
        let files = (0..40)
            .map(|index| git::ChangedFile {
                status: "M".into(),
                path: format!("file-{index}.txt"),
                original_path: None,
            })
            .collect();
        app.apply(crate::app::Response::Files {
            oid: app.selected_oid().unwrap().to_owned(),
            id: 0,
            result: Ok(files),
        });
        terminal
            .draw(|frame| {
                crate::ui::draw(frame, &mut app, false);
            })
            .unwrap();
        assert_eq!(app.top, 4);
        assert_eq!(app.top_offset, 27);
        assert_eq!(snapshot(&app), state);
    }

    #[test]
    fn missing_or_invalid_row_offsets_default_to_zero() {
        let mut app = demo();
        for state in [
            json!({}),
            json!({"top_offset": null}),
            json!({"top_offset": -1}),
            json!({"top_offset": 1.5}),
            json!({"top_offset": "3"}),
        ] {
            app.top_offset = 7;
            restore(&mut app, &state);
            assert_eq!(app.top_offset, 0);
        }
    }

    #[test]
    fn inserted_commits_preserve_selection_and_top_oid_even_at_the_start() {
        for top in [0, 3] {
            let mut original = demo();
            original.selected = Some(top + 2);
            original.top = top;
            original.top_offset = 1;
            let state = snapshot(&original);
            let mut repo = original.repo.clone();
            for oid in ["new-first", "new-second"] {
                let mut commit = repo.commits[0].clone();
                commit.parents = vec![commit.oid.clone()];
                commit.oid = oid.into();
                repo.commits.insert(0, commit);
            }
            let mut restored = App::new(repo, 2000, true).with_sidebar(true);
            restore(&mut restored, &state);
            assert_eq!(restored.selected_oid(), original.selected_oid());
            assert_eq!(restored.selected, original.selected.map(|index| index + 2));
            assert_eq!(restored.top, top + 2);
            assert_eq!(restored.top_offset, 1);
            assert_eq!(restored.selected, Some(restored.top + 2));
            assert_eq!(restored.repo.commits[restored.top].oid, state["top_oid"]);
        }
    }

    #[test]
    fn removed_selection_and_top_fall_back_to_clamped_indices() {
        for selected in [5, 9] {
            let mut original = demo();
            original.selected = Some(selected);
            original.top = selected;
            let state = snapshot(&original);
            let mut repo = original.repo.clone();
            repo.commits.remove(selected);
            let mut restored = App::new(repo, 2000, true).with_sidebar(true);
            restore(&mut restored, &state);
            assert_eq!(restored.selected, Some(selected.min(8)));
            assert_eq!(restored.top, selected.min(8));
            assert_ne!(restored.selected_oid(), original.selected_oid());
        }
    }

    #[test]
    fn empty_history_has_no_anchors_and_restores_safely() {
        let mut original = demo();
        original.selected = Some(7);
        original.top = 5;
        original.graph_offset = 3;
        original.query = "unfinished query".into();
        original.searching = true;
        let mut repo = git::demo();
        repo.commits.clear();
        let mut empty = App::new(repo, 2000, true).with_sidebar(true);
        restore(&mut empty, &snapshot(&original));
        assert_eq!(empty.selected, None);
        assert_eq!(empty.top, 0);
        assert_eq!(empty.graph_offset, 0);
        assert_eq!(empty.query, original.query);
        assert!(empty.searching);
        let state = snapshot(&empty);
        assert!(state["selected_oid"].is_null());
        assert!(state["top_oid"].is_null());
        let mut restored = demo();
        restore(&mut restored, &state);
        assert_eq!(restored.selected, None);
        assert_eq!(restored.top, 0);
        assert_eq!(restored.query, empty.query);
        assert!(restored.searching);
    }

    #[test]
    fn malformed_fields_are_ignored_and_large_indices_are_clamped() {
        let mut app = demo();
        app.selected = Some(5);
        app.top = 3;
        app.graph_offset = 3;
        app.query = "keep me".into();
        app.searching = true;
        let original = snapshot(&app);
        for state in [Value::Null, json!([]), json!(true), json!(12), json!("bad")] {
            restore(&mut app, &state);
            assert_eq!(snapshot(&app), original);
        }
        restore(
            &mut app,
            &json!({
                "selected_oid": [], "selected": -1,
                "top_oid": false, "top": 1.5, "top_offset": -1,
                "graph_offset": "3", "query": {}, "searching": "false",
                "unknown": "ignored",
            }),
        );
        assert_eq!(snapshot(&app), original);
        restore(
            &mut app,
            &json!({
                "selected_oid": "missing", "selected": u64::MAX,
                "top_oid": "missing", "top": u64::MAX, "top_offset": u64::MAX,
                "graph_offset": u64::MAX,
            }),
        );
        // Oversized integers are either clamped or ignored on narrower targets.
        let expected = if usize::try_from(u64::MAX).is_ok() {
            9
        } else {
            5
        };
        assert_eq!(app.selected, Some(expected));
        assert_eq!(app.top, if expected == 9 { 9 } else { 3 });
        assert_eq!(app.top_offset, usize::try_from(u64::MAX).unwrap_or(0));
        assert!(app.graph_offset <= app.graph.width.saturating_sub(1) * 3);
        assert_eq!(app.query, "keep me");
        assert!(app.searching);
    }

    #[test]
    fn sidebar_draw_preserves_viewport_independently_of_selection() {
        for selected in [None, Some(1), Some(9)] {
            let mut app = demo();
            restore(&mut app, &json!({"selected": selected, "top": 5}));
            assert_eq!(app.selected, selected);
            assert_eq!(app.top, 5);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(32, 12)).unwrap();
            terminal
                .draw(|frame| {
                    crate::ui::draw(frame, &mut app, false);
                })
                .unwrap();
            assert_eq!(app.selected, selected);
            assert_eq!(app.top, 5);
        }
    }
}
