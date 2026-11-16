# Проверка публикации и сборки 0.2.1

16 ноября 2026.

- Исходники опубликованы с явного разрешения владельца в публичный cazmusw/VibeShell.
- Импорт 0.2.0: commit ad5d658304daf89222712ab851ece5ac665f1762; Nix build и upload runtime closure успешно прошли: https://github.com/cazmusw/VibeShell/actions/runs/43821731814.
- Изменения 0.2.1: commit 81062d2070ecc48c90f3a18398137bfb48e8f1fa; Nix build и upload runtime closure успешно прошли: https://github.com/cazmusw/VibeShell/actions/runs/43822643542.
- Version tag не создавался; release job закономерно skipped. Готовые binaries доступны через Actions artifacts. CD на v-тег настроен, но фактический draft release пока не создавался.
- Новые тесты не писались, существующие не запускались. Nix doCheck=false и workflow override doCheck=false сохраняются по просьбе пользователя.
- Независимый статический просмотр output-lifecycle изменений не выявил конкретных блокирующих проблем. Это не runtime-проверка.
- Визуальное поведение, hotplug, фокус, GPU, звук и прочие системные интеграции на пользовательской машине пока не проверены.

Последующие коммиты только документации с [skip ci] не меняют проверенный Rust/Nix-код. Пользователь может скачать артефакт указанного запуска, распаковать ZIP и выполнить bash install.sh (Nix + zstd).
