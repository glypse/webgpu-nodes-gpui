use crate::graph::{ColorSpace, GraphModel, GraphProject, ProjectSettings};

#[test]
fn default_when_a_project_has_none() {
    assert_eq!(GraphModel::default().settings(), ProjectSettings::default());
}

#[test]
fn are_saved_and_loaded_with_the_project() {
    let mut model = GraphModel::default();
    let mut settings = model.settings();
    settings.color_space = ColorSpace::DisplayP3;
    assert!(model.update_settings(settings));
    let loaded = GraphProject::from_json(&model.project().to_json()).expect("loads");
    assert_eq!(GraphModel::new(loaded).settings().color_space, ColorSpace::DisplayP3);
}

#[test]
fn values_out_of_range_are_refused() {
    let mut model = GraphModel::default();
    let mut settings = model.settings();
    settings.canvas.width = 0;
    assert!(!model.update_settings(settings));
    settings.canvas.width = 100;
    settings.timing.fps = f64::NAN;
    assert!(!model.update_settings(settings));
    assert_eq!(model.settings(), ProjectSettings::default());
}

#[test]
fn are_part_of_the_undo_history_and_unsaved_changes() {
    let mut model = GraphModel::default();
    let mut settings = model.settings();
    settings.timing.fps = 24.0;
    settings.timing.realtime = false;
    assert!(model.update_settings(settings));
    assert!(model.has_unsaved_changes());
    assert!(model.undo());
    assert_eq!(model.settings().timing, ProjectSettings::default().timing);
    assert!(model.redo());
    assert_eq!(model.settings().timing.fps, 24.0);
}

#[test]
fn unreadable_settings_fall_back_to_the_defaults() {
    let mut project = GraphProject::default();
    project.extra.insert("settings".to_owned(), serde_json::json!({ "canvas": 3 }));
    assert_eq!(GraphModel::new(project).settings(), ProjectSettings::default());
}
