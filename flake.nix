{
  inputs = {
    nixpkgs = {
      url = "github:NixOs/nixpkgs/nixos-unstable";
    };
    fenix = {
      url = "github:nix-community/fenix/monthly";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { nixpkgs, fenix, ... }:
    let
      forAllSystems = nixpkgs.lib.genAttrs [
        "aarch64-linux"
        "x86_64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];

      mkToolchain = system: with fenix.packages.${system}; combine [
        latest.cargo
        latest.rust-std
        latest.rustc
        latest.clippy
        latest.miri
        latest.llvm-tools
        latest.rustfmt
        targets.thumbv6m-none-eabi.latest.rust-std
        targets.thumbv7em-none-eabihf.latest.rust-std
        targets.thumbv7em-none-eabi.latest.rust-std
      ];
    in
    {
      devShells = forAllSystems
        (system:
          let
            pkgs = nixpkgs.legacyPackages.${system};
            nativeBuildInputs = with pkgs; [
              (mkToolchain system)
              cargo-expand
              cargo-binutils
              cargo-udeps
              clang-tools
              probe-rs
            ];
          in
          {
            default = pkgs.mkShell {
              inherit nativeBuildInputs;
            };
          }
        );
    };
}
