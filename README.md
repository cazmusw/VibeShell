# VibeShell

Rust/GTK4 desktop shell for Labwc and NixOS, continued from Mosaic Shell.

Current source version: **0.2.1**. Dock, panel, application launcher, window integration and experimental notifications are implemented. The full 1.0 specification is still in progress.

## Build and run

```sh
nix build
./result/bin/shellctl doctor
./result/bin/mosaic-shell
```

Run inside your Labwc session as its normal user, without sudo/root.

CI builds x86_64 NixOS binaries and exports the complete runtime closure. Download the artifact from GitHub Actions and run `bash install.sh`. Tags `v*` create a draft release. Tests are temporarily disabled at the owner's request.

- [CI/CD and artifacts](docs/CI-CD.md)
- [Dock 0.2 features and limits](docs/dock-0.2.md)
- [Project status](STATUS.md)
- [Approved specification (Russian)](docs/specs/shell-spec-ru.md)
- [Original application documentation](docs/Mosaic-0.2-README.md)

A successful build does not validate graphical behavior on your compositor/GPU.
