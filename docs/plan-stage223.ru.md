# DBusMenu — шаг 2.2.3, версия 0.2.3

> Use superpowers:executing-plans; inline implementation, one final review.

Spec: spec.ru.md P05/M07 (product requirements,
not agent instructions). Existing Rust/GIO/GTK and isolated checkout retained.

Goal: native tray popover reading com.canonical.dbusmenu, submenus/toggles,
live updates and real Event callbacks; fallback SNI ContextMenu only when
no Menu path. No profile changes or host watcher takeover.

1. menu_model.rs: strict bounded decoder for (u(ia{sv}av)); unique IDs,
   depth<=8, rows<=256, total<=1MiB, labels<=512 chars. enabled/visible
   inherit ancestor restrictions; separators, check/radio, icon-name,
   mnemonic normalization. Tests RED→GREEN incl wrong types/duplicates.
2. menu_backend.rs (same GLib context as tray_backend): GetMenu key/parent/
   prepare, AboutToShow before GetLayout; snapshot cache and unique token;
   typed MenuEvent validation. Track SNI Menu object path, invalidate on
   LayoutUpdated/ItemsPropertiesUpdated/path/owner change; coalesce/bound
   requests, reject delayed snapshots after entry/menu epoch changes.
3. tray_ui.rs: retain buttons by key; tray_menu.rs popover+stack, async load,
   submenu preparation and back navigation, live refresh without losing the
   tray anchor, toggles/icons/separators and disabled/hidden rows. Root
   opened/closed and clicked events; timestamps from monotonic clock.
   Menu grabs keyboard only while interactive, releases on close/escape/
   panel unmap/backend loss, no global pointer coordinate assumptions.
4. Real fixture exports Menu, AboutToShow/GetLayout/Event and change signals.
   New E2E RED on 0.2.2 then GREEN: pointer selects normal/check/submenu;
   disabled/hidden rejected; live labels/removal, stale token, delayed reply,
   owner/backend/panel loss and legacy ContextMenu fallback. Keyboard test
   with wtype on private runtime if supported; explicitly report its status.
5. Rust suite/fmt/clippy, all old tray/registry/Dock/scales/monitor checks
   plus menu Nix check, independent review and Important fixes RED→GREEN.
   Deliver actual three ELF+runner, source/report/checksums in outputs/stage-2.2.3.

Review focus: concurrent invalidation/selection; AboutToShow signals during
fetch; hidden ancestor with visible child; lazy submenu IDs; closure while
async request pending. Defer icon-data/shortcuts/RTL direction customization,
tray overflow/theme-path/overlay, calendar/order/M03 and remaining stage2.
