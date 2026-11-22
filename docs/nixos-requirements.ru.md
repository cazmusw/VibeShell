# Что нужно для Mosaic на NixOS

Для панели, Dock и проводника достаточно рабочей Wayland-сессии Labwc,
пользовательской шины D-Bus и сборки Mosaic из Nix. GTK4,
gtk4-layer-shell, `grim`, `pactl` и тема значков входят в её зависимости.
При запуске через Home Manager используются пользовательские сервисы
systemd.

| Функция | Что подключить в NixOS |
| --- | --- |
| Панель, Dock и окна | `programs.labwc.enable = true` |
| Живые изображения окон | Labwc 0.20 или новее |
| Wi‑Fi и сетевые профили | `networking.networkmanager.enable = true` |
| Громкость, входы и маршрутизация приложений | `services.pipewire.enable = true`, `services.pipewire.pulse.enable = true`, WirePlumber |
| Bluetooth | `hardware.bluetooth.enable = true`, поддерживаемый адаптер |
| Батарея и состояние зарядки | `services.upower.enable = true`, батарея для показаний заряда |
| Переключение профилей питания | `services.power-profiles-daemon.enable = true` |
| Корзина, диски и `admin:///` | `services.gvfs.enable = true`, `services.udisks2.enable = true`, Polkit и графический агент для защищённых файлов |
| Порталы для приложений | `xdg.portal.enable = true`, GTK и wlr backends |
| Температуры и яркость | Датчики или подсветка, доступные через ядро и logind |

Чтобы включить Bluetooth и службы питания, добавьте
эти параметры в конфигурацию NixOS.

```nix
hardware.bluetooth.enable = true;
services.upower.enable = true;
services.power-profiles-daemon.enable = true;
```

Службами питания должен управлять один менеджер профилей. В NixOS модуль
`power-profiles-daemon` конфликтует с TLP и `auto-cpufreq`.

Для встроенного Bluetooth-сопряжения запустите `mosaic-session` с
`MOSAIC_BLUETOOTH_PAIRING=1`. В Home Manager это опция
`programs.mosaic-shell.bluetoothPairing.enable`. Используйте один
агент сопряжения для этой задачи.

`networkmanagerapplet` добавляет `nm-connection-editor` для VPN,
Enterprise Wi‑Fi и сложных профилей. Для обычного Wi‑Fi этот пакет
необязателен. `pavucontrol` и `brightnessctl` полезны как внешние
инструменты.

XWayland нужен приложениям X11. GTK-приложения,
браузеры и терминалы можно выбирать независимо от Mosaic.

До реализации соответствующих пунктов Mosaic за уведомления продолжает
отвечать `mako`, за фон — `swaybg`, за системную блокировку — `swaylock`.
Снимки экрана можно делать
через Flameshot или `grim` с `slurp`. Интеграция с MPRIS
потребует запущенного плеера с поддержкой MPRIS, когда соответствующий
шаг будет готов.

Источники. [Параметры NixOS](https://nixos.org/manual/nixos/stable/options) ·
[Модуль power-profiles-daemon](https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/services/hardware/power-profiles-daemon.nix) ·
[Интерфейс UPower](https://upower.freedesktop.org/docs/Device.html) ·
[Интерфейс профилей питания](https://upower.pages.freedesktop.org/power-profiles-daemon/gdbus-org.freedesktop.UPower.PowerProfiles.html)
