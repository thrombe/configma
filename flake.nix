{
  description = "yaaaaaaaaaaaaaaaaaaaaa";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-23.05";
    unstable-nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    unstable-nixpkgs,
    flake-utils,
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
      };
      unstable = import unstable-nixpkgs {
        inherit system;
      };

      manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
    in {
      packages.default = unstable.rustPlatform.buildRustPackage {
        pname = manifest.name;
        version = manifest.version;
        cargoLock.lockFile = ./Cargo.lock;
        src = pkgs.lib.cleanSource ./.;

        nativeBuildInputs = [];
      };

      devShells.default = pkgs.mkShell {
        nativeBuildInputs = with pkgs;
          [
            unstable.rust-analyzer
            unstable.rustfmt
            unstable.clippy
          ]
          ++ self.packages."${system}".default.nativeBuildInputs;
        shellHook = ''
          export RUST_BACKTRACE="1"
        '';
      };
    });
}
