use super::*;

const TOLERANCE: MinSize = MinSize {
    width: 0.05,
    height: 0.05,
};
const THRESHOLD: MinSize = MinSize {
    width: 0.01,
    height: 0.01,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ed {
    Preview,
    Timeline,
}

const A: AreaId = AreaId(0);
const B: AreaId = AreaId(1);
const C: AreaId = AreaId(2);

fn screen(rects: &[(f64, f64, f64, f64)]) -> Screen<Ed> {
    Screen::new(
        rects
            .iter()
            .map(|&(l, t, r, b)| (Rect::new(l, t, r, b), Ed::Preview)),
    )
}

/// A tall area on the left, two stacked ones on the right.
fn t_layout() -> Screen<Ed> {
    screen(&[
        (0.0, 0.0, 0.5, 1.0),
        (0.5, 0.0, 1.0, 0.5),
        (0.5, 0.5, 1.0, 1.0),
    ])
}

fn rects(screen: &Screen<Ed>) -> Vec<(u64, [f64; 4])> {
    screen.validate().expect("the result tiles the window");
    let mut out: Vec<_> = screen
        .areas()
        .iter()
        .map(|a| {
            (
                a.id.0,
                [a.rect.left, a.rect.top, a.rect.right, a.rect.bottom],
            )
        })
        .collect();
    out.sort_by_key(|(id, _)| *id);
    out
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn split_takes_the_asked_side_and_copies_the_editor() {
    let one = Screen::new([(Rect::FULL, Ed::Timeline)]);
    let (split, id) = one
        .split(A, Axis::X, 0.25, Part::Before, MinSize::NONE)
        .unwrap();
    assert_eq!(
        rects(&split),
        vec![(0, [0.25, 0.0, 1.0, 1.0]), (id.0, [0.0, 0.0, 0.25, 1.0])]
    );
    assert_eq!(split.area(id).unwrap().editor, Ed::Timeline);
}

#[test]
fn split_refuses_parts_smaller_than_the_minimum() {
    assert!(
        t_layout()
            .split(A, Axis::Y, 0.02, Part::After, TOLERANCE)
            .is_none()
    );
    assert!(
        t_layout()
            .split(A, Axis::Y, 1.0, Part::After, MinSize::NONE)
            .is_none()
    );
}

#[test]
fn join_grows_over_a_neighbour_sharing_its_whole_edge() {
    let joined = t_layout().join(B, C, TOLERANCE).unwrap();
    assert_eq!(
        rects(&joined),
        vec![(0, [0.0, 0.0, 0.5, 1.0]), (1, [0.5, 0.0, 1.0, 1.0])]
    );
}

#[test]
fn join_lines_up_ends_closer_than_the_tolerance() {
    let s = screen(&[
        (0.0, 0.0, 0.5, 0.5),
        (0.5, 0.0, 1.0, 0.52),
        (0.0, 0.5, 0.5, 1.0),
        (0.5, 0.52, 1.0, 1.0),
    ]);
    let joined = s.join(A, B, TOLERANCE).unwrap();
    joined.validate().unwrap();
    let a = joined.area(A).unwrap().rect;
    assert!(approx(a.right, 1.0) && approx(a.bottom, 0.51));
    assert!(approx(joined.area(C).unwrap().rect.top, 0.51));
    assert!(approx(joined.area(AreaId(3)).unwrap().rect.top, 0.51));
}

#[test]
fn join_gives_the_kept_areas_overhang_to_the_neighbours() {
    let joined = t_layout().join(A, B, TOLERANCE).unwrap();
    assert_eq!(
        rects(&joined),
        vec![(0, [0.0, 0.0, 1.0, 0.5]), (2, [0.0, 0.5, 1.0, 1.0])]
    );
}

#[test]
fn join_keeps_the_removed_areas_overhang_with_its_editor() {
    let mut s = t_layout();
    s.set_editor(A, Ed::Timeline);
    let joined = s.join(B, A, TOLERANCE).unwrap();
    assert_eq!(
        rects(&joined),
        vec![
            (1, [0.0, 0.0, 1.0, 0.5]),
            (2, [0.5, 0.5, 1.0, 1.0]),
            (3, [0.0, 0.5, 0.5, 1.0]),
        ]
    );
    assert_eq!(joined.area(AreaId(3)).unwrap().editor, Ed::Timeline);
}

#[test]
fn join_refuses_areas_that_arent_neighbours() {
    let s = screen(&[
        (0.0, 0.0, 0.3, 1.0),
        (0.3, 0.0, 0.6, 1.0),
        (0.6, 0.0, 1.0, 1.0),
    ]);
    assert!(s.join(A, C, TOLERANCE).is_none());
}

#[test]
fn close_lets_the_best_aligned_neighbour_take_its_place() {
    let closed = t_layout().close(B, TOLERANCE).unwrap();
    assert_eq!(
        rects(&closed),
        vec![(0, [0.0, 0.0, 0.5, 1.0]), (2, [0.5, 0.0, 1.0, 1.0])]
    );
}

#[test]
fn close_never_closes_the_last_area() {
    let one = Screen::new([(Rect::FULL, Ed::Preview)]);
    assert!(one.close(A, TOLERANCE).is_none());
}

#[test]
fn dock_moves_an_editor_into_a_new_part_of_another_area() {
    let mut s = screen(&[(0.0, 0.0, 0.5, 1.0), (0.5, 0.0, 1.0, 1.0)]);
    s.set_editor(A, Ed::Timeline);
    let docked = s
        .dock(A, B, DockTarget::Side(Side::Bottom), 0.25, TOLERANCE)
        .unwrap();
    assert_eq!(
        rects(&docked),
        vec![(1, [0.0, 0.0, 1.0, 0.75]), (2, [0.0, 0.75, 1.0, 1.0])]
    );
    assert_eq!(docked.area(B).unwrap().editor, Ed::Preview);
    assert_eq!(docked.area(AreaId(2)).unwrap().editor, Ed::Timeline);
}

#[test]
fn dock_in_the_centre_replaces_the_targets_editor() {
    let mut s = screen(&[(0.0, 0.0, 0.5, 1.0), (0.5, 0.0, 1.0, 1.0)]);
    s.set_editor(A, Ed::Timeline);
    let docked = s.dock(A, B, DockTarget::Center, 0.5, TOLERANCE).unwrap();
    assert_eq!(rects(&docked), vec![(1, [0.0, 0.0, 1.0, 1.0])]);
    assert_eq!(docked.area(B).unwrap().editor, Ed::Timeline);
}

#[test]
fn edge_picks_the_sides_connected_to_the_one_under_the_pointer() {
    let edge = t_layout().edge_at(0.5, 0.2, THRESHOLD, false).unwrap();
    assert_eq!(edge.axis, Axis::X);
    assert_eq!(
        edge.sides,
        vec![
            EdgeSide {
                area: A,
                side: Side::Right
            },
            EdgeSide {
                area: B,
                side: Side::Left
            },
            EdgeSide {
                area: C,
                side: Side::Left
            },
        ]
    );
}

#[test]
fn edge_stops_at_an_area_spanning_across_unless_extended() {
    let grid = screen(&[
        (0.0, 0.0, 0.5, 0.5),
        (0.5, 0.0, 1.0, 0.5),
        (0.0, 0.5, 1.0, 0.7),
        (0.0, 0.7, 0.4, 1.0),
        (0.4, 0.7, 1.0, 1.0),
    ]);
    let edge = grid.edge_at(0.5, 0.2, THRESHOLD, false).unwrap();
    let ids: Vec<_> = edge.sides.iter().map(|s| s.area.0).collect();
    assert_eq!(ids, vec![0, 1]);
    let moved = grid.move_edge(&edge, 0.3, MinSize::NONE);
    moved.validate().unwrap();
    assert_eq!(moved.area(A).unwrap().rect.right, 0.3);
    assert_eq!(moved.area(AreaId(3)).unwrap().rect.right, 0.4);

    let lower = grid.edge_at(0.4, 0.8, THRESHOLD, false).unwrap();
    assert_eq!(lower.sides.len(), 2);
    let horizontal = grid.edge_at(0.2, 0.7, THRESHOLD, true).unwrap();
    assert_eq!(horizontal.axis, Axis::Y);
    assert_eq!(horizontal.sides.len(), 3);
}

#[test]
fn edge_moves_are_clamped_to_the_minimum_size() {
    let s = t_layout();
    let edge = s.edge_at(0.5, 0.2, THRESHOLD, false).unwrap();
    let moved = s.move_edge(&edge, 0.99, TOLERANCE);
    moved.validate().unwrap();
    assert!(approx(moved.area(B).unwrap().rect.left, 0.95));
    assert_eq!(s.edge_limits(&edge, TOLERANCE), (0.05, 0.95));
}

#[test]
fn an_area_shrinks_to_exactly_its_header_and_no_further() {
    const WINDOW_HEIGHT: f64 = 800.0;
    let s = screen(&[(0.0, 0.0, 1.0, 0.5), (0.0, 0.5, 1.0, 1.0)]);
    let min = MinSize {
        width: 96.0 / 1000.0,
        height: f64::from(HEADER_HEIGHT) / WINDOW_HEIGHT,
    };
    let header = f64::from(HEADER_HEIGHT) / WINDOW_HEIGHT;
    let edge = s.edge_at(0.5, 0.5, THRESHOLD, true).unwrap();
    assert_eq!(edge.axis, Axis::Y);
    assert!(approx(s.edge_limits(&edge, min).0, header));
    for target in [0.0, 0.02] {
        let moved = s.move_edge(&edge, target, min);
        moved.validate().unwrap();
        assert!(approx(moved.area(A).unwrap().rect.bottom, header));
        assert!(approx(moved.area(B).unwrap().rect.top, header));
    }
    let moved = s.move_edge(&edge, 0.1, min);
    assert!(approx(moved.area(A).unwrap().rect.bottom, 0.1));
}

#[test]
fn edge_ignores_the_windows_border() {
    assert!(t_layout().edge_at(0.0, 0.5, THRESHOLD, false).is_none());
}

#[test]
fn swap_exchanges_editors_keeping_the_rectangles() {
    let mut s = screen(&[(0.0, 0.0, 0.5, 1.0), (0.5, 0.0, 1.0, 1.0)]);
    s.set_editor(A, Ed::Timeline);
    let swapped = s.swap(A, B).unwrap();
    assert_eq!(swapped.area(A).unwrap().editor, Ed::Preview);
    assert_eq!(swapped.area(B).unwrap().editor, Ed::Timeline);
    assert_eq!(rects(&swapped), rects(&s));
}

#[test]
fn validate_rejects_broken_screens() {
    assert_eq!(Screen::<Ed>::new([]).validate(), Err(Invalid::Empty));
    let gap = screen(&[(0.0, 0.0, 0.5, 1.0)]);
    assert_eq!(gap.validate(), Err(Invalid::NotCovered));
    let overlap = screen(&[(0.0, 0.0, 0.6, 1.0), (0.4, 0.0, 1.0, 1.0)]);
    assert_eq!(overlap.validate(), Err(Invalid::Overlap(A, B)));
}

#[test]
fn ids_stay_fresh_across_operations() {
    let s = t_layout();
    let (s, first) = s
        .split(A, Axis::Y, 0.5, Part::After, MinSize::NONE)
        .unwrap();
    let s = s.close(first, TOLERANCE).unwrap();
    let (_, second) = s
        .split(A, Axis::Y, 0.5, Part::After, MinSize::NONE)
        .unwrap();
    assert_ne!(first, second);
}

/// Random sequences of operations keep the window tiled.
#[test]
fn operations_keep_the_window_tiled() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut s = t_layout();
    for _ in 0..2000 {
        let pick = |s: &Screen<Ed>, r: f64| s.areas()[(r * s.areas().len() as f64) as usize].id;
        let a = pick(&s, random());
        let b = pick(&s, random());
        let next = match (random() * 5.0) as u32 {
            0 => {
                let axis = if random() < 0.5 { Axis::X } else { Axis::Y };
                let r = s.area(a).unwrap().rect;
                let at = match axis {
                    Axis::X => r.left + r.width() * random(),
                    Axis::Y => r.top + r.height() * random(),
                };
                s.split(a, axis, at, Part::After, TOLERANCE).map(|(s, _)| s)
            }
            1 => s.join(a, b, TOLERANCE),
            2 => s.close(a, TOLERANCE),
            3 => s.dock(a, b, DockTarget::Side(Side::Left), 0.5, TOLERANCE),
            _ => s
                .edge_at(random(), random(), THRESHOLD, random() < 0.3)
                .map(|edge| s.move_edge(&edge, random(), TOLERANCE)),
        };
        if let Some(next) = next {
            next.validate().expect("still tiled");
            s = next;
        }
    }
}
