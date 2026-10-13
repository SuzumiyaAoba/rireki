{
  description = "Claude Code and Codex session history viewer";

  # Nixpkgs 26.05 still supports Intel Macs as well as Apple Silicon and Linux.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          rireki = pkgs.callPackage ./nix/package.nix { };
          default = self.packages.${system}.rireki;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.rireki ];
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
            ];

            # Override the host compiler settings without changing .cargo/config.toml.
            env = pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
              CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER = "${pkgs.stdenv.cc}/bin/cc";
              CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER = "${pkgs.stdenv.cc}/bin/cc";
              CC_aarch64_apple_darwin = "${pkgs.stdenv.cc}/bin/cc";
              CXX_aarch64_apple_darwin = "${pkgs.stdenv.cc}/bin/c++";
              CC_x86_64_apple_darwin = "${pkgs.stdenv.cc}/bin/cc";
              CXX_x86_64_apple_darwin = "${pkgs.stdenv.cc}/bin/c++";
            };

            shellHook = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              export LD_LIBRARY_PATH="${
                pkgs.lib.makeLibraryPath self.packages.${system}.rireki.buildInputs
              }''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            '';
          };
        }
      );

      checks = forAllSystems (system: {
        inherit (self.packages.${system}) rireki;
      });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-tree);
    };
}
