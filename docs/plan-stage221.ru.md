# Мониторы и hotplug — шаг 2.2.1, версия 0.2.1

> Use superpowers:executing-plans; implement inline, one final fresh review.

Goal: один Dock на preferred connector со стабильным fallback, одна панель
на каждом подключённом выходе, перенос при hotplug без потери pins.
Spec: spec.ru.md, M01/M02/M04/M05 частично.
Стек и изолированный рабочий каталог сохраняются. Пользователь поручил
продолжать существующий план; новая активация профиля не выполняется.

Constraints: Rust safe wrappers, GDK monitor identity + connector, события
wl_output enter/leave на собственном соединении Tracker; не передавать
GTK/Wayland объекты между соединениями. Не использовать глобальные
координаты указателя для выбора выхода. Schema 2 сохраняется с default
для нового dock.preferred_output (пустая строка — auto). Только один Dock;
режим Dock на каждом экране и current-output filter остаются далее.

Review focus: исчезновение всех outputs; изменение геометрии/scale без
нового monitor object; быстрое reconnect; output с пустым connector;
закрытие popup перед уничтожением surface. Headless не заменяет DRM audit.

Task 1 — output policy + config.
- [x] tests/outputs.rs select_output preferred/fallback/reorder/empty,
  config preferred connector roundtrip + invalid control/length. Run RED.
- [x] src/outputs.rs select_output(&str, &[String])->Option<String>,
  validate new dock.preferred_output <=256, no control chars. Run GREEN.

Task 2 — real output topology + surfaces.
- [x] tests/platform/monitors.py starts two outputs at mixed scales with
  negative placement; assert two mapped panels/one Dock on preferred,
  off/on preferred transfers Dock + pins unchanged + no duplicate panels;
  invalid preferred uses stable fallback; sole output resizes; zero
  outputs hides surfaces then reconnect recovers. Run RED on old binary.
- [x] src/panel.rs owns per-monitor panel widgets/lifetime; src/ui.rs
  reconciles monitor ListModel, updates geometry/scale notifications,
  closes shell dialogs/popover on monitor removal, preserves Dock items.
  Dock::set_monitor handles hide/rebind/resize/reveal and zero monitors.
- [x] src/ipc.rs OutputInfo/GetOutputs + shellctl outputs diagnostics.

Task 3 — output-scoped window state.
- [x] Extend real test: probe window output membership from wl_output,
  fullscreen hides only its output's panel and Dock when on that output.
- [x] src/toplevel.rs bind own wl_output v4 names; track enter/leave and
  global removal. Window.outputs Vec<String> projected at snapshot time.
  Preserve existing GetWindows signature, add GetWindowOutputs diagnostic.

Task 4 — release.
- [x] GUI field preferred connector; README+ledger; no schema 1 overwrite
  changes. Version 0.2.1, updated Home Manager defaults remain compatible.
- [x] Rust suite/fmt/clippy; full old scale/Dock E2E + monitors Nix check;
  final review, fix important issues RED→GREEN, build/copy actual ELF and
  runner, rerun delivered test, source tar/checksums/report in outputs.

Deferred: tray/SNI/DBusMenu, calendar/panel order, left/right Dock,
desktop-file drop, per-output Dock/filter and M03 popup placement remain
next substeps of 2.2. Never claim entire 2.2 or M01–M07 complete here.
