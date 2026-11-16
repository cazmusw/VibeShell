# Mosaic Shell: план начальной реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Создать проверяемую начальную версию оболочки и основу дальнейших этапов утверждённого ТЗ.

**Architecture:** Rust workspace; чистое доменное ядро, отдельный Wayland-адаптер, GTK4 UI и backend системных сервисов. Первый вертикальный срез связывает реальные desktop entries, окна Wayland, Dock, поиск, уведомления и session D-Bus; интеграции центра управления вводятся последовательно.

**Tech Stack:** Rust 1.90, GTK4, gtk4-layer-shell, gio/glib, wayland-client, zbus, serde/TOML.

**Spec:** ../../specs/shell-spec-ru.md

## Global Constraints

- Rust + GTK4; Labwc остаётся композитором; нет root-операций и shell interpolation.
- Начальные размеры: панель 32, Dock 48, отступ 12 логических px.
- Неизвестные параметры пользовательской системы проверяет doctor; местная среда не считается NixOS/Labwc.
- Нет обещания полной версии 1.0 по результатам первого среза; незавершённые требования отмечаются в STATUS.md.

## Review Focus

- Устаревшие Wayland handle после закрытия окна: действие отклоняется.
- Конфигурация из Nix store: strict запрещает запись, defaults не перезаписывает declarative файл.
- Desktop entries с пробелами и field codes: запуск через GIO, без sh -c.
- Исчезновение D-Bus/Wayland: понятная ошибка без воздействия на Labwc.
- Недоступная интеграция не показывается как работающая кнопка.

## Файлы и этапы

### Task 1: Доменное ядро и конфигурация

**Files:** crates/shell-core/src/{lib,config,model,search}.rs, tests/core.rs, config.example.toml.
**Interfaces:** Config::parse(&str) -> Result<Config>; Config::load() -> Result<Config>; WindowStore::apply(WindowEvent); WindowStore::snapshot(); rank(query, candidates).

- [ ] Написать тесты диапазонов Dock, неизвестных полей, strict/defaults и stale window epochs.
- [ ] Выполнить cargo test -p shell-core; подтвердить отсутствие реализации.
- [ ] Реализовать типы, валидацию, конфигурационные слои и поиск.
- [ ] Выполнить весь набор тестов core; commit.

### Task 2: Реальный Wayland backend и doctor

**Files:** crates/shell-wayland/src/{lib,worker}.rs, crates/shellctl/src/main.rs.
**Interfaces:** WaylandHandle::start() -> channels; Command::{Activate,Minimize,Close}; Event::{Snapshot,Disconnected}; probe() -> CapabilityReport.

- [ ] Добавить тесты отсутствующих globals, устаревших команд и сортировки snapshot.
- [ ] Запустить тесты; реализовать registry, seat, foreign-toplevel и event-driven worker с bounded queues.
- [ ] Добавить shellctl doctor с JSON-отчётом среды, globals и доступности D-Bus; ошибки не превращать в успех.
- [ ] Проверить headless failure path и тесты; commit.

### Task 3: GTK4 UI, каталог приложений и CLI

**Files:** crates/shell-ui/src/{main,app,catalog,dock,panel,launcher}.rs, assets/style.css.
**Interfaces:** Catalog::load() использует gio::AppInfo; launch по desktop ID; session bus org.mosaic.Shell1 принимает действия UI.

- [ ] Проверить код запуска через GIO и тесты ранжирования из Task 1.
- [ ] Реализовать настоящие layer-shell поверхности, список окон, запуск/активацию, закрепления, поиск, сетку, настройки темы, часы и меню окон.
- [ ] Скомпилировать GTK4 UI с реальными native dependencies; GUI smoke test только при доступном композиторе.
- [ ] Проверить CLI на isolated D-Bus; commit.

### Task 4: Backend уведомлений

**Files:** crates/shell-services/src/{main,notifications}.rs, tests/notifications.rs.
**Interfaces:** org.freedesktop.Notifications, org.mosaic.Services1; snapshot истории для UI.

- [ ] Тесты replacement, limits, transient и корректного удаления.
- [ ] Реализовать сервер, deadlines, сигналы и ограниченную историю; UI отображает баннеры/центр только при доступном backend.
- [ ] Проверить протокол через dbus-run-session; commit.

### Task 5: Nix, упаковка и отчёт

**Files:** flake.nix, nix/{package,home-manager,nixos}.nix, scripts/mosaic-session, README.md, STATUS.md.
**Interfaces:** packages.default, devShells.default, homeManagerModules.default, nixosModules.default.

- [ ] Добавить session wrapper с импортом среды и остановкой custom target, не перезаписывая Labwc пользователя.
- [ ] Создать модули strict/defaults и пример запуска. Зафиксировать lock-файлы, если доступен Nix.
- [ ] cargo test --workspace, fmt, clippy, build; D-Bus smoke; документировать недоступные проверки.
- [ ] Отдельное ревью исходников, исправление существенных проблем; подготовить архив исходников и сохранить.

## Следующие планы полного ТЗ

После начального среза: полноценные sound/network/Bluetooth/power agents и privacy при блокировке; tray/DBusMenu; autohide/drag-and-drop/multi-monitor и accessibility; wallpaper/root-menu; файловый индекс; транзакционные SQLite-миграции; visual/performance/DRM acceptance. Для них сохраняются требования утверждённого ТЗ. Здесь не создаются ложные работающие заглушки этих возможностей.
