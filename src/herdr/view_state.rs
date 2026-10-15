use crate::app::App;
use serde_json::{Value, json};

// The sidebar has no branch selector or detail panel, so only history state is
// persisted. OIDs anchor the selection and viewport when newer commits arrive.
pub(super) fn snapshot(app: &App) -> Value {
    json!({
        "selected_oid": app.selected_oid(),
        "selected": app.selected,
        "top_oid": app.repo.commits.get(app.top).map(|commit| commit.oid.as_str()),
        "top": app.top,
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
    let anchored_index = |oid_key: &str, index_key: &str, fallback: usize| {
        state
            .get(oid_key)
            .and_then(Value::as_str)
            .and_then(|oid| app.repo.commits.iter().position(|commit| commit.oid == oid))
            .unwrap_or_else(|| index(index_key).unwrap_or(fallback))
            .min(app.repo.commits.len().saturating_sub(1))
    };
    let selected = anchored_index("selected_oid", "selected", app.selected);
    let top = anchored_index("top_oid", "top", app.top);
    if app.selected != selected {
        app.selected = selected;
        app.selection_changed();
    }
    // Let ui::draw enforce visibility using the actual terminal height.
    app.top = top;
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
            original.selected = 7;
            original.top = 5;
            original.graph_offset = 3;
            original.query = "한글 author".into();
            original.searching = searching;
            original.selection_changed();
            let state = snapshot(&original);
            assert_eq!(state["selected_oid"], original.repo.commits[7].oid);
            assert_eq!(state["top_oid"], original.repo.commits[5].oid);
            assert!(state.get("reference").is_none());

            let mut restored = demo();
            restore(&mut restored, &state);
            assert_eq!(restored.selected_oid(), original.selected_oid());
            assert_eq!(restored.top, original.top);
            assert_eq!(restored.graph_offset, original.graph_offset);
            assert_eq!(restored.query, original.query);
            assert_eq!(restored.searching, searching);
            assert_eq!(restored.detail, original.detail);
            assert_eq!(snapshot(&restored), state);
        }
    }

    #[test]
    fn inserted_commits_preserve_selection_and_top_oid_even_at_the_start() {
        for top in [0, 3] {
            let mut original = demo();
            original.selected = top + 2;
            original.top = top;
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
            assert_eq!(restored.selected, original.selected + 2);
            assert_eq!(restored.top, top + 2);
            assert_eq!(restored.selected - restored.top, 2);
            assert_eq!(restored.repo.commits[restored.top].oid, state["top_oid"]);
        }
    }

    #[test]
    fn removed_selection_and_top_fall_back_to_clamped_indices() {
        for selected in [5, 9] {
            let mut original = demo();
            original.selected = selected;
            original.top = selected;
            let state = snapshot(&original);
            let mut repo = original.repo.clone();
            repo.commits.remove(selected);
            let mut restored = App::new(repo, 2000, true).with_sidebar(true);
            restore(&mut restored, &state);
            assert_eq!(restored.selected, selected.min(8));
            assert_eq!(restored.top, selected.min(8));
            assert_ne!(restored.selected_oid(), original.selected_oid());
        }
    }

    #[test]
    fn empty_history_has_no_anchors_and_restores_safely() {
        let mut original = demo();
        original.selected = 7;
        original.top = 5;
        original.graph_offset = 3;
        original.query = "unfinished query".into();
        original.searching = true;
        let mut repo = git::demo();
        repo.commits.clear();
        let mut empty = App::new(repo, 2000, true).with_sidebar(true);
        restore(&mut empty, &snapshot(&original));
        assert_eq!(empty.selected, 0);
        assert_eq!(empty.top, 0);
        assert_eq!(empty.graph_offset, 0);
        assert_eq!(empty.query, original.query);
        assert!(empty.searching);
        let state = snapshot(&empty);
        assert!(state["selected_oid"].is_null());
        assert!(state["top_oid"].is_null());
        let mut restored = demo();
        restore(&mut restored, &state);
        assert_eq!(restored.selected, 0);
        assert_eq!(restored.top, 0);
        assert_eq!(restored.query, empty.query);
        assert!(restored.searching);
    }

    #[test]
    fn malformed_fields_are_ignored_and_large_indices_are_clamped() {
        let mut app = demo();
        app.selected = 5;
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
                "top_oid": false, "top": 1.5,
                "graph_offset": "3", "query": {}, "searching": "false",
                "unknown": "ignored",
            }),
        );
        assert_eq!(snapshot(&app), original);
        restore(
            &mut app,
            &json!({
                "selected_oid": "missing", "selected": u64::MAX,
                "top_oid": "missing", "top": u64::MAX,
                "graph_offset": u64::MAX,
            }),
        );
        // Oversized integers are either clamped or ignored on narrower targets.
        let expected = if usize::try_from(u64::MAX).is_ok() {
            9
        } else {
            5
        };
        assert_eq!(app.selected, expected);
        assert_eq!(app.top, if expected == 9 { 9 } else { 3 });
        assert!(app.graph_offset <= app.graph.width.saturating_sub(1) * 3);
        assert_eq!(app.query, "keep me");
        assert!(app.searching);
    }

    #[test]
    fn draw_not_restore_adjusts_selection_visibility() {
        let mut app = demo();
        restore(&mut app, &json!({"selected": 1, "top": 8}));
        assert_eq!(app.selected, 1);
        assert_eq!(app.top, 8);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(32, 12)).unwrap();
        terminal
            .draw(|frame| {
                crate::ui::draw(frame, &mut app, false);
            })
            .unwrap();
        assert_eq!(app.top, 1);
    }
}
