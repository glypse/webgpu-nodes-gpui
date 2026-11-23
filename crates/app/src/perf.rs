//! What editing costs on the benchmark scene with a cloned library group: regression tests for
//! the work an edit may skip, and an ignored timing run (`cargo test -p webgpu-nodes perf -- --ignored --nocapture`).

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Entity, TestAppContext};

    use crate::editor::{Editor, EditorEvent};

    use crate::shell::{Shell, init};

    fn timed<T>(label: &str, cx: &mut gpui::VisualTestContext, f: impl FnOnce(&mut gpui::VisualTestContext) -> T) -> T {
        let t = Instant::now();
        let r = f(cx);
        let edit = t.elapsed();
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        eprintln!("{label}: edit {edit:?}, edit + frame {:?}", t.elapsed());
        r
    }

    fn open_scene(cx: &mut TestAppContext) -> (Entity<Editor>, Rc<RefCell<Vec<&'static str>>>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            theme::init(cx);
            crate::preferences::init(cx);
        });
        let (editor, cx) = cx.add_window_view(Editor::new);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes/voronoi.wgnodes");
        let loaded = crate::project::read(&path).expect("the scene loads");
        editor.update(cx, |e, cx| e.load_project(Some(loaded.project), None, cx));
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &EditorEvent, _| {
                seen.borrow_mut().push(match event {
                    EditorEvent::Shader(_) => "shader",
                    EditorEvent::Uniforms(_) => "uniforms",
                    EditorEvent::Settings(_) => "settings",
                    EditorEvent::Edited => "edited",
                    EditorEvent::ExportGroup => "export",
                });
            })
            .detach();
        });
        cx.run_until_parked();
        (editor, events, cx)
    }

    #[gpui::test]
    fn moving_between_groups_leaves_the_program_alone(cx: &mut TestAppContext) {
        let (editor, events, cx) = open_scene(cx);
        let group = editor.read_with(cx, |e, _| {
            e.model().root().nodes.iter().find(|n| n.definition_ref().is_some()).map(|n| n.id.clone())
        });
        let group = group.expect("a group");
        editor.update(cx, |e, cx| e.enter_group(&group, cx));
        assert!(editor.read_with(cx, |e, _| !e.model().path().is_empty()), "entered the group");
        editor.update(cx, |e, cx| e.jump_to(0, cx));
        assert!(editor.read_with(cx, |e, _| e.model().path().is_empty()), "left it again");
        assert!(events.borrow().is_empty(), "the program didn't change, but {:?} were emitted", events.borrow());
    }

    #[gpui::test]
    fn a_setting_is_announced_once_and_changes_no_program(cx: &mut TestAppContext) {
        let (editor, events, cx) = open_scene(cx);
        let changes: [fn(&mut shader_graph::graph::ProjectSettings); 2] =
            [|s| s.timing.fps = 30.0, |s| s.canvas.width += 1];
        for change in changes {
            events.borrow_mut().clear();
            editor.update(cx, |e, cx| {
                let mut settings = e.settings();
                change(&mut settings);
                e.update_settings(settings, cx);
            });
            assert_eq!(*events.borrow(), ["settings"]);
        }
    }

    #[test]
    #[ignore]
    fn codegen_loop() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes/voronoi.wgnodes");
        let loaded = crate::project::read(&path).expect("the scene loads");
        let model = shader_graph::graph::GraphModel::new(loaded.project);
        let t = Instant::now();
        for _ in 0..100 {
            model.generate_hoisted().expect("generates");
        }
        eprintln!("generate x100: {:?}", t.elapsed());
    }

    #[gpui::test]
    #[ignore]
    fn editing_timings(cx: &mut TestAppContext) {
        crate::probe::install(None).ok();
        cx.update(|cx| {
            theme::init(cx);
            crate::preferences::init(cx);
            init(cx);
        });
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes/voronoi.wgnodes");
        let (shell, cx) = cx.add_window_view(|window, cx| Shell::new(Some(path), None, window, cx));
        let cx = &mut cx.clone();
        cx.run_until_parked();
        let editor = shell.read_with(cx, |s, _| s.editor().clone());
        let group = editor.read_with(cx, |e, _| {
            e.model().root().nodes.iter().find(|n| n.definition_ref().is_some_and(|r| r.starts_with("std-fractal-voronoi"))).map(|n| n.id.clone())
        });
        let group = group.expect("a fractal voronoi group");
        timed("no-op edit (sync + regenerate)", cx, |cx| editor.update(cx, |e, cx| e.edit(cx, |_| {})));
        timed("frame without sheet", cx, |cx| editor.update(cx, |_, cx| cx.notify()));
        timed("clone", cx, |cx| editor.update(cx, |e, cx| e.clone_group(&group, cx)));
        timed("enter", cx, |cx| editor.update(cx, |e, cx| e.enter_group(&group, cx)));
        timed("exit", cx, |cx| editor.update(cx, |e, cx| e.jump_to(0, cx)));
        timed("settings fps", cx, |cx| {
            editor.update(cx, |e, cx| {
                let mut s = e.settings();
                s.timing.fps = 30.0;
                e.update_settings(s, cx);
            })
        });
        timed("settings canvas", cx, |cx| {
            editor.update(cx, |e, cx| {
                let mut s = e.settings();
                s.canvas.width += 1;
                e.update_settings(s, cx);
            })
        });
        timed("open sheet", cx, |cx| editor.update(cx, |e, cx| e.open_sheet(&group, cx)));
        for _ in 0..3 {
            timed("frame with sheet", cx, |cx| editor.update(cx, |_, cx| cx.notify()));
        }
        let playback = shell.read_with(cx, |s, _| s.playback().clone());
        cx.update(|window, cx| playback.update(cx, |p, cx| p.toggle(window, cx)));
        let before = crate::probe::report().unwrap_or_default();
        let t = Instant::now();
        for _ in 0..30 {
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
        }
        eprintln!("30 frames while playing with the sheet open: {:?}", t.elapsed());
        let _ = before;
        cx.update(|window, cx| playback.update(cx, |p, cx| p.toggle(window, cx)));
        timed("no-op edit with sheet", cx, |cx| editor.update(cx, |e, cx| e.edit(cx, |_| {})));
        timed("delete", cx, |cx| {
            editor.update(cx, |e, cx| {
                e.edit(cx, |m| {
                    m.delete_nodes(std::slice::from_ref(&group), &[]);
                })
            })
        });
        if let Some(report) = crate::probe::report() {
            eprintln!("{report}");
        }
    }
}
