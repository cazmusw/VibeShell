# Реализация по плану 2026-11-15-initial-shell

Ruling: Начинаем самостоятельно после прямого указания пользователя «переходи к написанию»; дополнительное утверждение плана не требуется.
Ruling: Новый проект в отдельном каталоге и feature-ветке; исходного репозитория нет, worktree неприменим.
Ruling: Это начальный срез, а не весь выпуск 1.0. GUI/DRM и NixOS-проверки требуют пользовательской системы. Невыполненные пункты будут явно перечислены.
Pre-flight: core Config/WindowStore/search потребляются UI и Wayland; интерфейсы разделены, GTK объекты не пересекают границу потоков.
Task 1: complete — RED missing public types; GREEN cargo test -p shell-core, 5/5. Config validation, strict/defaults and stale epochs covered.
Task 2: complete for initial backend — registry/seat/foreign-toplevel worker, bounded commands with wake FD; 2/2 tests pass. Connection recovery and output hotplug remain future acceptance work. AsFd import fixed from compiler evidence.
Task 4: domain notifications RED missing store -> GREEN 3/3 tests. Server compiles and declares only body/persistence; UI/persistence/actions incomplete and explicitly tracked.
Task 3: initial UI and CLI implemented; cargo build --workspace passed with GTK4 4.18.6 / gtk4-layer-shell 1.0.4. Native GUI unavailable because Unix socket creation is denied. Not a completed platform acceptance.
Task 4: service and center snapshot implemented; no banners/actions/disk history in this slice. 4 store tests pass; D-Bus integration test cannot create socket, not counted as passed.
Task 5: flake.lock generated; nix flake check --no-build and NixOS config evaluation passed. Full Nix build not performed.
Final review: fresh reviewer found startup exit code and activation environment cleanup issues. Fixed with RED→GREEN regression tests (Rust startup status + Python wrapper lifecycle).
Final: Ruling: notification eviction was regraded Important because a live notification must produce NotificationClosed. Added eviction IDs and service signal; test RED→GREEN.
Final: Ruling: CSS provider accumulation was fixed by keeping one provider for the Shell lifetime; repeated settings changes must not retain unbounded providers. Verified compilation and clippy; runtime memory acceptance remains pending.
Final: minor (deferred): dock.margin/panel.height reload currently needs UI restart; documented in STATUS.md.

2026-11-16: source restored from user attachment. Work branch feat/dock-behavior. User explicitly paused all test writing/running. Existing tests remain unchanged. CI now only builds; Nix doCheck=false.
GitHub creation remains blocked by pending explicit consent to Google sign-in; no new authorization was inferred from attachment-only message. No GitHub repository or push has occurred.
2026-11-16 Dock 0.2: implemented fixed-allocation DockIcon snapshot scaling, bounded viewport and scrolling, edge sensor and autohide delays, stable popup hold, ShowDock keyboard invocation and GUI settings. Restored r2 bundle launcher/runtime diagnostic source from attached binary archive.
Verification: cargo check --workspace PASS; cargo build --workspace PASS; cargo fmt --all -- --check PASS. No tests written or run. Independent static reviewer found no Important/Critical defects; compositor focus, geometry and visual behavior remain unverified.
Manual review: removed button↔GestureClick and Popover↔child-callback retention cycles; menu detaches before deferred row rebuild. Keyboard summon temporarily uses Overlay and returns to Top on completion. Existing unit/integration tests retained without running.
