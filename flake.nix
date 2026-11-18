{
  description = "Mouse-first terminal graph editor that exports Typst/Fletcher drawings";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "petersen";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./tests
              ./typst
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          # The tests read system fonts, create POSIX shared memory and compile
          # Typst with network access; run `cargo test` in the dev shell instead.
          doCheck = false;
          meta = {
            description = "Mouse-first terminal graph editor that exports Typst/Fletcher drawings";
            homepage = "https://github.com/sreyassabbani/petersen";
            mainProgram = "petersen";
          };
        };
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = pkgs.lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
