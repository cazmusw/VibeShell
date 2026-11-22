#!/usr/bin/env bash
# Package an already built Nix derivation; does not compile or execute application tests.
set -euo pipefail
result=${1:-result}
destination=${2:-dist}
package=$(readlink -f -- "$result")
if [[ $package != /nix/store/* || ! -x $package/bin/mosaic-shell ]]; then
    echo "Not a built Mosaic/VibeShell package: $result" >&2
    exit 1
fi
mkdir -p -- "$destination"
mapfile -t closure < <(nix-store --query --requisites "$package")
if (( ${#closure[@]} == 0 )); then
    echo 'Nix returned an empty runtime closure' >&2
    exit 1
fi
nix-store --export "${closure[@]}" | zstd -T2 -10 -o "$destination/VibeShell-x86_64-linux.nix-export.zst"
printf '%s\n' "$package" > "$destination/package-path.txt"
cat > "$destination/install.sh" <<'INSTALL'
#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
sha256sum --check SHA256SUMS
zstd --decompress --stdout VibeShell-x86_64-linux.nix-export.zst | nix-store --import
IFS= read -r package < package-path.txt
if [[ $package != /nix/store/* || $package == *$'\n'* || ! -x $package/bin/mosaic-shell ]]; then
    echo 'Imported application executable was not found' >&2
    exit 1
fi
printf 'Installed Nix store package: %s\nRun inside your Labwc session as its normal user:\n  %s/bin/mosaic-shell\n' "$package" "$package"
INSTALL
chmod +x "$destination/install.sh"
cat > "$destination/README.txt" <<'README'
VibeShell / Mosaic Shell — NixOS x86_64 binary package

Download all files into one directory and run:
  bash install.sh

Requires an installed Nix package manager and zstd. This imports already built
binaries and their complete runtime dependencies; it does not compile Rust.
If Nix refuses import because your user is not a trusted store importer, ask
your system administrator to import the archive into the Nix store. Run the
GUI itself as the normal user of your Labwc session, without sudo/root.

The package-path.txt file records the executable's exact immutable store path.
Keep a Nix GC root or install that path into a profile if you want to retain it
across garbage collection. The script does not modify your session or config.

Test the graphical session on your own compositor and GPU.
README
(
    cd -- "$destination"
    sha256sum VibeShell-x86_64-linux.nix-export.zst package-path.txt install.sh README.txt > SHA256SUMS
)
