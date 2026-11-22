# Этап 1: вертикальный срез

Поручение пользователя: продолжать разработку по ТЗ, выдавать бинарник
после каждого шага. Повторное согласование выбранного GTK/Rust не требуется.

1. Доменные правила в `src/desktop.rs`: matching desktop ID/app_id/WMClass,
   непрозрачные window IDs с epoch, решения launch/activate/list. Тесты
   закрытого окна, старой epoch, неоднозначности и стабильного порядка pins.
2. `src/settings.rs`: минимальная версия конфигурации, pins, атомарная запись
   в XDG state; отдельные тестовые каталоги. Не трогать Nix symlink.
3. `src/toplevel.rs`: wayland-client + wlr-protocol bindings, отдельное
   соединение с собственным wl_seat; события подтверждают состояния.
   Event loop ожидает fd и команды без непрерывного polling.
4. `src/ui.rs`, `src/bin/mosaic-shell.rs`: gtk4-rs + layer-shell, верхняя
   панель, плавающий Dock, каталог приложений GIO, запуск через AppInfo,
   выбор окна, pin/unpin, индикация running/active. Один монитор.
5. `src/ipc.rs`, `src/bin/mosaic-services.rs`: session D-Bus health/snapshot,
   команды существующему UI. CLI launcher/applications/dock/settings/quit.
   Одна копия UI/сервиса; GTK объекты только в main thread.
6. Nix package и Home Manager target/services для UWSM graphical-session;
   явные enable/disable, без изменения текущего пользовательского профиля.
7. fmt, clippy, domain/config tests, isolated D-Bus, headless end-to-end
   с реальным launch и window activation, restart; затем GUI smoke в
   текущей сессии. Выпустить `outputs/stage-1` с командами и ограничениями.

Полная оболочка 1.0, tray/уведомления/control-center/анимации и multi-monitor
не объявляются реализованными на этапе 1. Проверки этапа 0 сохранены.
