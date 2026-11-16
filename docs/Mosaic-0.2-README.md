# Обновление 0.2.0

Реализован следующий блок Dock: autohide с edge sensor, плавное увеличение иконок с фиксированными hit targets, ограниченная ширина с прокруткой, `shellctl dock show` и настройки GUI. Подробности и ограничения: [docs/dock-0.2.md](docs/dock-0.2.md).

На этом этапе тесты по просьбе пользователя не запускаются. Ниже сохранено описание исходной версии 0.1; актуальный список изменений выше и в STATUS.md.

# Mosaic Shell

Начальная реализация настольной оболочки для **Labwc / NixOS** на **Rust + GTK4**. Самостоятельный дизайн в духе macOS. Это версия **0.1.0 для разработки**, не завершённый выпуск 1.0 из ТЗ.

## Что уже написано

- GTK4 layer-shell Dock и верхняя панель; панель создаётся на каждом мониторе при запуске, Dock — на первом.
- Каталог XDG-приложений через GIO, запуск без `sh -c`, поиск и список приложений.
- Реальный foreign-toplevel backend: список окон, активация, сворачивание и закрытие; проверка connection epoch перед командами.
- Закрепление через контекстное меню, индикаторы запущенных и активного приложения.
- Часы, календарь, переход в домашнюю папку; настройки светлой/тёмной темы, размера иконок и reduced motion.
- TOML strict/defaults, отдельные атомарные пользовательские overrides; GUI сохраняет только изменённые поля.
- Экспериментальный отдельный сервер уведомлений и центр просмотра: replacement, deadlines, transient, лимит истории и сигналы закрытия. История пока в памяти, без баннеров, actions и lock integration. По умолчанию этот сервер не включён в Home Manager.
- `shellctl`, диагностика окружения, Nix-пакет, devShell и модули.

Полный перечень границ и проверок: [STATUS.md](STATUS.md). Утверждённое ТЗ: [docs/specs/shell-spec-ru.md](docs/specs/shell-spec-ru.md).

## Сборка

На NixOS с включёнными flakes:

```sh
nix build
./result/bin/shellctl version
./result/bin/shellctl doctor
```

`flake.lock` и `Cargo.lock` включены. Пакет собирает `mosaic-shell`, `mosaic-services`, `shellctl`, устанавливает `mosaic-session`. Полная сборка Nix ещё требует проверки на NixOS; в среде разработки прошли Rust build и Nix evaluation.

Для разработки:

```sh
nix develop
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
python3 -m unittest discover -s tests -p 'test_*.py'
```

Без Nix нужны Rust 1.90+, pkg-config, GTK4 4.12+ и gtk4-layer-shell 1.x с development headers. Зафиксированный toolchain находится в `rust-toolchain.toml`.

## Пробный запуск внутри существующего Labwc

Сначала проверьте окружение:

```sh
./result/bin/shellctl doctor
```

Команда печатает JSON и возвращает ненулевой код при отсутствии обязательной конфигурации, session D-Bus либо Wayland-протоколов. В отчёте показаны владельцы ролей уведомлений и трея; тело уведомлений и заголовки окон не выводятся.

Не меняя автозапуск, запустите UI в терминале сессии:

```sh
./result/bin/mosaic-shell
```

Для сохранения overrides в пользовательском режиме:

```sh
mkdir -p ~/.config/mosaic-shell
cp config.example.toml ~/.config/mosaic-shell/config.toml
```

В примере закрепления нужно заменить на реально установленные desktop IDs. В strict-режиме управление соответствующими настройками через GUI запрещено. `MOSAIC_CONFIG=/absolute/path/config.toml` позволяет выбрать отдельный конфиг.

Команды работающему UI:

```sh
./result/bin/shellctl launcher
./result/bin/shellctl applications
./result/bin/shellctl settings
./result/bin/shellctl notifications
./result/bin/shellctl reload
```

Экспериментальный backend можно отдельно запустить `./result/bin/mosaic-services`, если вы намеренно остановили другой сервер уведомлений. Он не вытесняет существующего владельца `org.freedesktop.Notifications`, а завершает запуск с ошибкой. Сейчас он предназначен для разработки протокола и центра истории: всплывающих баннеров ещё нет. Для повседневных уведомлений пока оставьте текущий сервер, например mako.

## Home Manager

Добавьте локальный проект как flake input (позже его можно заменить URL репозитория):

```nix
inputs.mosaic.url = "path:/absolute/path/to/mosaic-shell";
inputs.mosaic.inputs.nixpkgs.follows = "nixpkgs";
```

В конфигурации Home Manager:

```nix
imports = [ inputs.mosaic.homeManagerModules.default ];
programs.mosaic-shell = {
  enable = true;
  configurationMode = "strict"; # defaults разрешает изменяемые GUI overrides
  enableLabwcBindings = true;  # создаёт пример, не меняет rc.xml
  notifications.enable = false;
  settings = {
    theme = "dark";
    dock = {
      icon_size = 48;
      margin = 12;
      pinned = [ "firefox.desktop" ];
    };
    panel.height = 32;
  };
};
```

Внутри **существующего** `~/.config/labwc/autostart` добавьте одну строку:

```sh
"${XDG_CONFIG_HOME:-$HOME/.config}/mosaic-shell/start-target" &
```

Она запускает custom user target после появления Wayland display и импортирует актуальную среду. Генерируемый файл `mosaic-shell/labwc-keybindings.xml` — пример: его `<keybind>` нужно перенести в `<keyboard>` существующего `rc.xml`. Автоматического перезаписывания Labwc-конфигурации нет.

Для управляемого завершения используйте `mosaic-session` **вместо команды запуска Labwc** в вашем session launcher. Не запускайте его внутри уже работающего Labwc: он запускает сам композитор. Wrapper ожидает дочерний процесс и при выходе останавливает target, очищает Wayland/X11 activation environment. Поддерживается одна графическая сессия пользователя. Если wrapper не используется, остановите `mosaic-session.target` в lifecycle hook вашего менеджера сессии.

Вручную внутри сессии:

```sh
systemctl --user start mosaic-session.target
systemctl --user status mosaic-shell.service
journalctl --user -u mosaic-shell.service
systemctl --user stop mosaic-session.target
```

Прямой `start` предполагает, что `start-target` уже импортировал среду. Для первого ручного запуска используйте именно `~/.config/mosaic-shell/start-target`.

NixOS-модуль `services.mosaic-shell.enable` включает Labwc и polkit. NetworkManager и Bluetooth включаются только отдельными флагами `enableNetworkManager` / `enableBluetooth`. Это подготовка системных служб; интерфейс управления ими пока не реализован. Существующие звук, сеть, wallpaper, panel и notification services автоматически не отключаются.

## Проверка интеграции

На машине, где разрешены локальные Unix-сокеты:

```sh
dbus-run-session -- tests/integration/notifications.sh
```

Тест запускает собственный сервер на отдельной session bus, отправляет реальное уведомление и проверяет, что второй сервер не вытесняет первый. Он не затрагивает D-Bus текущего рабочего стола.

После запуска в вашей Labwc-сессии следует проверить: активацию GTK/Qt/XWayland-окон, фокус после закрытия поиска, несколько мониторов, запуск и выход wrapper. Эти проверки не заменяются unit tests.

## Дальнейшая разработка

Следующий блок — проверка на фактической конфигурации пользователя: вывод doctor, версия Labwc/NixOS, GPU и схема мониторов. Далее по ТЗ: autohide и графика Dock; tray/DBusMenu; звук/NetworkManager/BlueZ/power; полноценные уведомления и безопасная интеграция блокировки; wallpaper и меню рабочего стола; файловый индекс; SQLite, доступность и performance acceptance.

Не реализованные действия не выводятся как якобы работающие элементы интерфейса.
