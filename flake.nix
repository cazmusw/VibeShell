{
  description = "Mosaic — Rust/GTK4 shell for Labwc";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      packages = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; };
        in { default = pkgs.callPackage ./nix/package.nix { }; });
      devShells = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; };
        in { default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.default ];
          packages = with pkgs; [ cargo rustc rustfmt clippy labwc wayland-utils dbus ];
        }; });
      checks = forAllSystems (system: { build = self.packages.${system}.default; });
      homeManagerModules.default = import ./nix/home-manager.nix self;
      nixosModules.default = import ./nix/nixos.nix;
    };
}
