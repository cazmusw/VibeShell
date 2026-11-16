# Сборка и выпуск VibeShell

CI запускается на push, pull request и вручную. Сборочная архитектура — x86_64-linux; исходный проект Mosaic Shell должен находиться в корне репозитория. До импорта исходников application build явно пропускается, о чём пишется в summary. Это не считается успешной сборкой приложения.

Nix собирает закреплённые в flake.lock и Cargo.lock зависимости. Тесты временно отключены прямым override doCheck=false по просьбе пользователя. Никакие unit, integration или graphical tests workflow не выполняет.

После сборки Actions сохраняет 14 дней артефакт с полным Nix export: исполняемые файлы и runtime closure, install.sh, package-path.txt и SHA256SUMS. Это нативная Nix-поставка, а не ELF без библиотек. Чтобы её использовать, не нужно повторно компилировать Rust. Установка требует Nix и zstd, а для непроверенного импортера может понадобиться системный администратор. Приложение запускается обычным пользователем сессии Labwc.

Тег v* после успешной сборки создаёт draft GitHub Release с теми же файлами. Финальную публикацию draft выполняет владелец. Workflow использует contents:read; contents:write выдаётся только tag-release job. Pull request job не публикует release. Дополнительные секреты не требуются.

Сборка не доказывает работоспособность UI: масштабирование, фокус, edge sensor, GPU и уведомления проверяются на реальном Labwc отдельно.

## Импорт исходников

В корень должны попасть Cargo.toml, Cargo.lock, flake.nix, flake.lock, crates/, assets/, nix/, config.example.toml и остальные файлы версии 0.2.0. Сохраните этот workflow и scripts/export-nix-package.sh, чтобы старый workflow из архива не заменил актуальную настройку CI/CD.
