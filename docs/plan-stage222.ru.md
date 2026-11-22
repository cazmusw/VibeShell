# Системный трей SNI — шаг 2.2.2, версия 0.2.2

> Use superpowers:executing-plans; implement inline and one fresh final review.

Spec: spec.ru.md, P05, backend ownership.
Goal: реальные SNI items на панелях, watcher/host, события, tooltip,
Activate/SecondaryActivate/Scroll и app-side ContextMenu. DBusMenu host
rendering — отдельный следующий подшаг, P05 полностью не заявляется.
Existing Rust GTK/GIO+zbus versions retained; no profile/host changes.

Task 1: bounded safe model, tests/tray.rs RED→GREEN.
Registration target service/path forms, duplicate canonical owner/path key,
ARGB big-endian pixmap→RGBA, invalid dimensions/size rejected. Up to 128
items, icons<=256x256, strings<=512 chars, no remote markup in GTK tooltip.

Task 2: src/tray_backend.rs on its own GLib context thread in mosaic-services.
- Export both KDE and freedesktop watcher interfaces at /StatusNotifierWatcher.
- Own watcher names only DoNotQueue, never replace existing watcher.
- When existing watcher owns name, register own host and import its items;
  consume registration/unregistration/NameOwnerChanged signals, retry when
  watcher changes, no periodic polling.
- Resolve registrations to unique owners before calls; remove on owner loss;
  subscribe changes before first GetAll. Coalesce GetAll per item; timeout
  each call, no synchronous IPC on GTK thread. Cap lists/images/strings.
- Export org.mosaic.Tray1 snapshot + Changed and Action methods, backend
  owns adapter state. All actions target resolved unique owner/object path.

Task 3: src/tray_ui.rs asynchronous GIO UI client and widgets per panel.
- Event-driven snapshot, backend restart recovery, no redraw in idle.
- Prefer known theme icon then bounded RGBA texture then placeholder;
  attention appearance, passive hidden, plain tooltip + accessible label.
- Actual primary/secondary/context click and scroll methods. Coordinates
  0,0 mean unspecified for remote SNI methods; no global pointer guesses.
- ItemIsMenu routes primary to ContextMenu. XEmbed not implemented.
- CLI tray snapshot and UI state diagnostics for real tests.

Task 4: real private-bus fixture + GTK pointer E2E, RED on old binary.
Test path/name registration, duplicate suppression, icon byte order,
properties/attention/passive, actual pointer actions and scroll, cleanup,
existing watcher coexistence, watcher/backend restart. No host bus watcher
is claimed by tests. Preserve old monitor/scales/Dock tests in flake check.

Task 5: 0.2.2 release, README+ledger+independent review, full tests/fmt/clippy
and Nix checks, actual ELF + runner/source/checksums in outputs/stage-2.2.2.

Review focus: owner turnover while refresh/action in flight; hostile pixmap;
registration storm; absent/restarting watcher/backend; panel hotplug while
callbacks pending. Deferred DBusMenu local menu, custom icon theme paths,
tray overflow, panel order/calendar/M03; no whole stage2 completion claim.
