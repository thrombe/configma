{
  pkgs ? import <nixpkgs> {},
  unstable ? import <nixos-unstable> {},
}:
pkgs.mkShell {
  packages = with pkgs; [
    unstable.cargo
    unstable.rustc
    unstable.clippy
    unstable.rust-analyzer
  ];
}
