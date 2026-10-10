{
  description = "Native Rust overview shell for Niri";
  inputs.nixpkgs.url = "nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }: let
    systems = [ "x86_64-linux" "aarch64-linux" ];
    eachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    makePackage = pkgs: pkgs.rustPlatform.buildRustPackage {
      pname = "astal-shell";
      version = "0.2.0";
      src = pkgs.lib.fileset.toSource {
        root = ./.;
        fileset = pkgs.lib.fileset.unions [
          ./Cargo.toml ./Cargo.lock ./src ./LICENSE
        ];
      };
      cargoLock.lockFile = ./Cargo.lock;
      nativeBuildInputs = with pkgs; [ pkg-config wrapGAppsHook4 ];
      buildInputs = with pkgs; [ gtk4 gtk4-layer-shell ];
      # Preserve the session's XDG_DATA_DIRS (including Home Manager's icon
      # packages). Adwaita is only a fallback, never a forced theme.
      preFixup = ''
        gappsWrapperArgs+=(
          --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.bash pkgs.curl pkgs.wireplumber pkgs.brightnessctl pkgs.systemd pkgs.niri ]}
          --suffix XDG_DATA_DIRS : ${pkgs.adwaita-icon-theme}/share
        )
      '';
      meta = with pkgs.lib; {
        description = "Rust GTK4 overview shell for Niri";
        license = licenses.gpl3Only;
        platforms = platforms.linux;
        mainProgram = "astal-shell";
      };
    };
  in {
    packages = eachSystem (pkgs: {
      default = makePackage pkgs;
      astal-shell = makePackage pkgs;
    });
    apps = eachSystem (pkgs: {
      default = {
        type = "app";
        program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/astal-shell";
      };
    });
    devShells = eachSystem (pkgs: {
      default = pkgs.mkShell {
        packages = with pkgs; [
          rustc cargo rustfmt clippy pkg-config gtk4 gtk4-layer-shell
          dbus wayland niri curl brightnessctl wireplumber ripgrep
        ];
        RUST_BACKTRACE = "1";
        shellHook = ''
          export XDG_DATA_DIRS="''${XDG_DATA_DIRS:-/usr/local/share:/usr/share}:${pkgs.adwaita-icon-theme}/share"
        '';
      };
    });
    checks = eachSystem (pkgs: {
      build = makePackage pkgs;
    });
    overlays.default = final: prev: { astal-shell = makePackage final; };
    homeManagerModules.default = import ./home-manager.nix self;
  };
}
