# Продолжение разработки VibeShell

Основной репозиторий: https://github.com/cazmusw/VibeShell, ветка main. Публичность явно подтверждена владельцем 16 ноября 2026. Здесь находятся текущие исходники, CI/CD и документация; временный workspace больше не считается единственным источником проекта.

## Что сделано

- Импортирован Mosaic Shell 0.2.0 с Dock, панелью, поиском, foreign-toplevel adapter и экспериментальным сервером уведомлений.
- Сохранены fixes r2 для XKB/Compose/locale; FHS launcher умеет использовать приватный временный cache, если постоянный каталог не доступен.
- Нативная Nix-поставка собирается в Actions и экспортируется вместе со всей runtime closure; Rust на машине пользователя компилировать не требуется.
- В 0.2.1 добавлены output-change lifecycle и live panel height; поведение на настоящем compositor ещё не подтверждено.

## Проверка

Пользователь временно запретил писать и запускать тесты. Nix package doCheck=false; workflow дополнительно принудительно отключает check phase. Допускаются компиляция, форматирование и статическое ревью. Прошедшая CI-сборка не равна проверке hotplug, keyboard focus или GPU.

## Следующий блок по ТЗ

1. Центр управления: сначала default audio output volume/mute и корректная обработка недоступного backend, затем выбор output/input.
2. Разделить необязательную роль notification server и постоянно доступный system-services backend, чтобы отключение уведомлений не отключало прочие интеграции.
3. Tray: StatusNotifierWatcher/Host, затем DBusMenu.
4. NetworkManager / BlueZ / power adapters с асинхронными операциями и понятными ошибками.
5. Полноценные уведомления, persistence и lock privacy; до этого notification server остаётся экспериментальным.

Каждый блок следует границам утверждённого docs/specs/shell-spec-ru.md. Не выдавать отсутствующие функции за готовые элементы UI. Большие binary closures и Git bundles не коммитить в Git; они размещаются в Actions artifacts/Release.
