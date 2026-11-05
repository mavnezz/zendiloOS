{
  # Development tooling only. Zendiloos itself is a Debian system and is
  # packaged and imaged inside a debian:trixie container, not with Nix.
  description = "Zendiloos development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          # Must match the sqlx version in backend/Cargo.toml; nixpkgs lags behind.
          sqlx-cli = pkgs.rustPlatform.buildRustPackage rec {
            pname = "sqlx-cli";
            version = "0.9.0";
            # The crates.io package ships the Cargo.lock the git repository lacks.
            src = pkgs.fetchCrate {
              inherit pname version;
              hash = "sha256-XariusjsCgn0Qai0XWtr7EzSzDDTp1cCzjff1kJNO9Y=";
            };
            cargoHash = "sha256-pHaMKuB9v3fjbgeVyLyRtfoQ9BkE6z+TjDfdBaVdbXM=";
            buildNoDefaultFeatures = true;
            buildFeatures = [ "sqlite" ];
            doCheck = false;
          };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              (rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
              nodejs_24
              just
              sqlx-cli
              sqlite
              cargo-deny
              # Test VM (scripts/vm.sh)
              qemu_kvm
              xorriso
            ];
          };
        }
      );
    };
}
