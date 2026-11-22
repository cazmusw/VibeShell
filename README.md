# VibeShell

Оболочка Mosaic для Wayland-сессии Labwc. Включает верхнюю панель, Dock,
переключатель окон, проводник и настройки звука, сети, Bluetooth и питания.

## Сборка и запуск

В NixOS x86_64 с включёнными flakes выполните из корня репозитория:

```sh
nix build . --no-write-lock-file --max-jobs 1 --cores 2
./result/bin/mosaic-session
```

Запускайте оболочку обычным пользователем внутри сессии Labwc.

- [Руководство Mosaic](README.ru.md)
- [Настройка NixOS и дополнительные службы](docs/nixos-requirements.ru.md)
- [Техническое задание](docs/spec.ru.md)

## GitHub Actions

[Сборка](https://github.com/cazmusw/VibeShell/actions/workflows/build.yml)
запускается при push, в pull request и вручную. Успешный запуск сохраняет
архив с пакетом NixOS x86_64, всеми его runtime-зависимостями и SHA-256.
Скачайте и распакуйте артефакт, затем выполните `bash install.sh`.
Скрипт проверит контрольные суммы и напечатает точную команду запуска.

Тег `v*` запускает сборку и создаёт черновик GitHub Release с этими же
файлами. Черновик можно опубликовать после проверки.

[Лицензия MIT](LICENSE)
