use shell_services::{NewNotification, NotificationStore};
fn input(sender: &str) -> NewNotification {
    NewNotification {
        sender: sender.into(),
        app: "Test".into(),
        summary: "Hello".into(),
        body: "Text".into(),
        ..Default::default()
    }
}
#[test]
fn replacements_are_scoped_to_sender_and_closed_ids_not_reused() {
    let mut s = NotificationStore::default();
    let id = s.notify(input(":1.2"), 0);
    let mut next = input(":1.2");
    next.replaces_id = id;
    next.summary = "Changed".into();
    assert_eq!(s.notify(next, 1), id);
    let mut other = input(":1.3");
    other.replaces_id = id;
    assert_ne!(s.notify(other, 2), id);
    assert_eq!(s.items()[0].summary, "Changed");
    assert!(s.close(id, ":1.3").is_none());
    assert!(s.close(id, ":1.2").is_some());
    let mut again = input(":1.2");
    again.replaces_id = id;
    assert_ne!(s.notify(again, 3), id);
}
#[test]
fn transient_and_expired_notifications_do_not_grow_history() {
    let mut s = NotificationStore::default();
    let mut n = input(":1.2");
    n.transient = true;
    n.timeout_ms = 10;
    let id = s.notify(n, 100);
    assert_eq!(s.expire(109), vec![]);
    assert_eq!(s.expire(110), vec![id]);
    assert!(s.items().is_empty());
    for now in 0..600 {
        let id = s.notify(input(":1.2"), now);
        s.close(id, ":1.2");
    }
    assert_eq!(s.items().len(), 500);
    assert!(s.items().iter().all(|n| !n.live));
}
#[test]
fn critical_default_is_persistent_and_explicit_zero_is_persistent() {
    let mut s = NotificationStore::default();
    let mut n = input(":1.2");
    n.urgency = 2;
    s.notify(n, 0);
    let mut n = input(":1.2");
    n.timeout_ms = 0;
    s.notify(n, 0);
    assert!(s.expire(100_000).is_empty());
}

#[test]
fn live_eviction_is_observable_and_closed_history_is_evicted_first() {
    let mut s = NotificationStore::default();
    let first = s.notify(input(":1.2"), 0);
    for now in 1..501 {
        s.notify(input(":1.2"), now);
    }
    assert_eq!(s.take_evicted(), vec![first]);
    let last = s.items().last().unwrap().id;
    s.close(last, ":1.2");
    s.notify(input(":1.2"), 600);
    assert!(s.take_evicted().is_empty());
}
