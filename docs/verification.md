# Проверка начального среза

Дата: 15 ноября 2026. Среда: Debian toolchain в изолированном workspace, не система пользователя.

| Проверка | Результат |
| --- | --- |
| cargo build --workspace | PASS |
| cargo test --workspace | PASS: 13 тестов, 0 failures |
| cargo fmt --all -- --check | PASS |
| cargo clippy --workspace --all-targets -- -D warnings | PASS |
| Python session regression | PASS: 1 тест |
| nix flake lock | PASS, lock сохранён |
| nix flake check --no-build | PASS для x86_64-linux; это evaluation, не сборка |
| NixOS module evaluation | PASS: Labwc, polkit и явно включённые NetworkManager/BlueZ вычислены как true |
| isolated D-Bus smoke | НЕ ВЫПОЛНЕН: создание Unix socket запрещено средой |
| Labwc GUI smoke / DRM | НЕ ВЫПОЛНЕН: headless запуск остановился на XWayland display |
| Home Manager end-to-end | НЕ ВЫПОЛНЕН |
| nix build | НЕ ВЫПОЛНЕН |
| aarch64 build | НЕ ВЫПОЛНЕН |

Собраны реальные binaries Rust/GTK4, а не mock интерфейс. Без проверки в пользовательской сессии совместимость конкретной установки Labwc не подтверждается. Внешняя публикация, установка на пользовательский компьютер и изменение его конфигурации не выполнялись.
