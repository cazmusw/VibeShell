use mosaic_shell_tools::dock_model::*;

#[test]
fn visual_icon_size_fits_the_existing_slot() {
    assert_eq!(visual_icon_size(32, 100), 32);
    assert_eq!(visual_icon_size(48, 50), 24);
    assert_eq!(visual_icon_size(47, 75), 35);
}

#[test]
fn window_indicators_keep_small_groups_and_compact_large_ones() {
    use WindowIndicator::*;
    assert_eq!(WindowIndicator::new(0, false), Empty);
    assert_eq!(WindowIndicator::new(0, true), Pending);
    assert_eq!(
        WindowIndicator::new(1, true),
        Dots {
            count: 1,
            overflow: false
        }
    );
    assert_eq!(
        WindowIndicator::new(2, false),
        Dots {
            count: 2,
            overflow: false
        }
    );
    assert_eq!(
        WindowIndicator::new(3, false),
        Dots {
            count: 3,
            overflow: false
        }
    );
    assert_eq!(
        WindowIndicator::new(4, false),
        Dots {
            count: 3,
            overflow: true
        }
    );
    assert_eq!(
        WindowIndicator::new(usize::MAX, false),
        Dots {
            count: 3,
            overflow: true
        }
    );
}
#[test]
fn autohide_delays_and_cancelled_edge_reveal() {
    let mut d = AutoHide::new(true, 150, 700, 0);
    d.event(Event::Timer, 699);
    assert!(d.visible);
    d.event(Event::Timer, 700);
    assert!(!d.visible);
    d.event(Event::EdgeEnter, 800);
    assert_eq!(d.deadline, Some(950));
    d.event(Event::EdgeLeave, 900);
    d.event(Event::Timer, 1000);
    assert!(!d.visible);
    d.event(Event::EdgeEnter, 1100);
    d.event(Event::Timer, 1249);
    assert!(!d.visible);
    d.event(Event::Timer, 1250);
    assert!(d.visible);
}
#[test]
fn popup_drag_and_keyboard_hold_prevent_hiding() {
    for hold in [Event::Menu(true), Event::Drag(true), Event::Keyboard(true)] {
        let mut d = AutoHide::new(true, 150, 700, 0);
        d.event(Event::Enter, 0);
        d.event(hold, 10);
        d.event(Event::Leave, 20);
        d.event(Event::Timer, 5000);
        assert!(d.visible);
        assert_eq!(d.deadline, None);
        let release = match hold {
            Event::Menu(_) => Event::Menu(false),
            Event::Drag(_) => Event::Drag(false),
            _ => Event::Keyboard(false),
        };
        d.event(release, 5100);
        assert_eq!(d.deadline, Some(5800));
        d.event(Event::Timer, 5800);
        assert!(!d.visible);
    }
}
#[test]
fn fullscreen_hides_and_explicit_show_can_reveal() {
    let mut d = AutoHide::new(false, 150, 700, 0);
    d.event(Event::Fullscreen(true), 10);
    assert!(!d.visible);
    d.event(Event::EdgeEnter, 20);
    d.event(Event::Timer, 1000);
    assert!(!d.visible);
    d.event(Event::Show, 1100);
    assert!(d.visible);
    d.event(Event::Enter, 1150);
    d.event(Event::Leave, 1200);
    d.event(Event::Timer, 1900);
    assert!(!d.visible);
    d.event(Event::Fullscreen(false), 2000);
    assert!(d.visible);
}
#[test]
fn disable_autohide_cancels_old_timer() {
    let mut d = AutoHide::new(true, 150, 700, 0);
    d.configure(false, 150, 700, 100);
    d.event(Event::Timer, 1000);
    assert!(d.visible);
    assert_eq!(d.deadline, None);
}
#[test]
fn magnification_is_symmetric_bounded_and_reduced_motion_is_static() {
    assert_eq!(zoom(0., 48., true, false), 1.45);
    assert_eq!(zoom(-30., 48., true, false), zoom(30., 48., true, false));
    assert!(zoom(60., 48., true, false) < zoom(30., 48., true, false));
    for d in [-1000., 0., 1000.] {
        assert_eq!(zoom(d, 48., true, true), 1.);
        assert_eq!(zoom(d, 48., false, false), 1.);
        assert!((1.0..=1.45).contains(&zoom(d, 48., true, false)));
    }
}
#[test]
fn overflow_stops_shrinking_at_minimum_and_preserves_requested_size() {
    assert_eq!(fitting_icon_size(48, 4, 600), 48);
    assert_eq!(fitting_icon_size(80, 40, 500), 32);
    assert_eq!(fitting_icon_size(48, 0, 0), 48);
    assert_eq!(fitting_icon_size(48, 20, 0), 32);
}
#[test]
fn reorder_preserves_unique_desktop_ids_and_rejects_unknown_source() {
    let p = vec!["a.desktop".into(), "b.desktop".into(), "c.desktop".into()];
    assert_eq!(
        reorder(&p, "c.desktop", "a.desktop").unwrap(),
        vec!["c.desktop", "a.desktop", "b.desktop"]
    );
    assert_eq!(
        reorder(&p, "a.desktop", "c.desktop").unwrap(),
        vec!["b.desktop", "a.desktop", "c.desktop"]
    );
    assert_eq!(reorder(&p, "a.desktop", "a.desktop").unwrap(), p);
    assert!(reorder(&p, "unknown.desktop", "a.desktop").is_err());
}
#[test]
fn dragging_across_a_pins_midpoint_can_move_after_it() {
    let pins = vec!["a.desktop".into(), "b.desktop".into(), "c.desktop".into()];
    let slots = [("a.desktop", 35.), ("b.desktop", 105.), ("c.desktop", 175.)];
    let before = pin_before_at(slots, 115.);
    assert_eq!(
        reorder(&pins, "a.desktop", before).unwrap(),
        ["b.desktop", "a.desktop", "c.desktop"]
    );
    let before = pin_before_at(slots, 185.);
    assert_eq!(
        reorder(&pins, "a.desktop", before).unwrap(),
        ["b.desktop", "c.desktop", "a.desktop"]
    );
}
#[test]
fn old_launch_timeout_cannot_expire_a_new_attempt() {
    let mut launches = Launches::default();
    let old = launches.start("a");
    let new = launches.start("a");
    assert!(!launches.expire("a", old));
    assert!(launches.contains("a"));
    assert!(launches.expire("a", new));
    assert!(!launches.contains("a"));
    launches.start("a");
    launches.acknowledge(&std::collections::HashSet::from(["a".into()]));
    assert!(!launches.contains("a"));
}
#[test]
fn divider_is_inserted_only_between_two_application_groups() {
    assert_eq!(divider_index([true, true, false, false]), Some(2));
    assert_eq!(divider_index([false, false]), None);
    assert_eq!(divider_index([true, true]), None);
    assert_eq!(divider_index([]), None);
}

#[test]
fn hover_strength_radius_and_easing_change_the_animation() {
    use mosaic_shell_tools::settings::{DockHover, HoverEasing};
    let hover = DockHover {
        zoom_percent: 60,
        radius_percent: 100,
        ..DockHover::default()
    };
    assert_eq!(zoom_with(0., 48., true, false, &hover), 1.6);
    assert_eq!(zoom_with(0., 48., true, true, &hover), 1.);
    let wider = DockHover {
        radius_percent: 150,
        ..hover.clone()
    };
    assert!(zoom_with(50., 48., true, false, &wider) > zoom_with(50., 48., true, false, &hover));
    assert_eq!(ease(HoverEasing::Linear, 0.5), 0.5);
    assert!(ease(HoverEasing::EaseOut, 0.5) > 0.5);
    assert_eq!(ease(HoverEasing::EaseInOut, 0.5), 0.5);
}

#[test]
fn hover_follows_a_sweep_and_reverses_without_jumping() {
    use mosaic_shell_tools::settings::{DockHover, HoverEasing};
    let settings = DockHover {
        easing: HoverEasing::Linear,
        ..DockHover::default()
    };
    let mut motion = HoverMotion::default();
    let entering = motion.frame(0, Some(0.), &settings);
    assert!(entering.active);
    assert_eq!(entering.amount, 0.);
    let sweep = motion.frame(100_000, Some(120.), &settings);
    assert_eq!(sweep.amount, 0.5);
    assert!(sweep.axis.is_some_and(|axis| axis > 0. && axis < 120.));
    let leave = motion.frame(100_000, None, &settings);
    assert_eq!(leave.amount, sweep.amount);
    assert_eq!(leave.axis, sweep.axis);
    let shrinking = motion.frame(150_000, None, &settings);
    assert_eq!(shrinking.amount, 0.375);
    let reverse = motion.frame(150_000, Some(20.), &settings);
    assert_eq!(reverse.amount, shrinking.amount);
    assert_eq!(reverse.axis, shrinking.axis);

    let mut settled = reverse;
    for tick in 10..=60 {
        settled = motion.frame(tick * 16_000, Some(20.), &settings);
    }
    assert_eq!(settled.amount, 1.);
    assert_eq!(settled.axis, Some(20.));
    assert!(!settled.active);
    motion.reset();
    assert!(!motion.frame(1_000_000, None, &settings).active);
}

#[test]
fn zero_duration_applies_the_target_in_one_frame() {
    use mosaic_shell_tools::settings::DockHover;
    let settings = DockHover {
        grow_ms: 0,
        shrink_ms: 0,
        ..DockHover::default()
    };
    let mut motion = HoverMotion::default();
    for (time, cursor) in [(0, Some(20.)), (16_000, Some(60.)), (32_000, None)] {
        let frame = motion.frame(time, cursor, &settings);
        assert_eq!(frame.axis, cursor);
        assert_eq!(frame.amount, if cursor.is_some() { 1. } else { 0. });
        assert!(!frame.active);
    }
}
