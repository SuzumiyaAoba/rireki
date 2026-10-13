{
  lib,
  stdenv,
  rustPlatform,
  cmake,
  pkg-config,
  makeWrapper,
  openssl,
  fontconfig,
  freetype,
  wayland,
  libxkbcommon,
  libxcb,
  vulkan-loader,
  libiconv,
}:

rustPlatform.buildRustPackage {
  pname = "rireki";
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../.cargo
      ../src
      ../tests
      ../assets
      ../licenses
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    cmake
    pkg-config
    rustPlatform.bindgenHook
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [ makeWrapper ];

  buildInputs =
    lib.optionals stdenv.hostPlatform.isLinux [
      openssl
      fontconfig
      freetype
      wayland
      libxkbcommon
      libxcb
      vulkan-loader
    ]
    ++ lib.optionals stdenv.hostPlatform.isDarwin [ libiconv ];

  # Compile Metal shaders at runtime so the build does not need Xcode's metal tool.
  buildFeatures = lib.optionals stdenv.hostPlatform.isDarwin [ "gpui/runtime_shaders" ];

  # Use Nix's compiler inside the build copy instead of the host paths.
  postPatch = lib.optionalString stdenv.hostPlatform.isDarwin ''
    rm .cargo/config.toml
  '';

  dontUseCmakeConfigure = true;

  # GPUI loads these libraries dynamically, so retain their paths at runtime.
  postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
    wrapProgram "$out/bin/rireki" \
      --prefix LD_LIBRARY_PATH : ${
        lib.makeLibraryPath [
          wayland
          libxkbcommon
          libxcb
          vulkan-loader
        ]
      }
  '';

  meta = {
    description = "Native viewer for Claude Code and Codex session history";
    homepage = "https://github.com/SuzumiyaAoba/rireki";
    mainProgram = "rireki";
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
}
