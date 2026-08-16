{
  description = "Clipboard Transformer desktop application and CLI";

  nixConfig = {
    extra-substituters = [ "https://jag-k.cachix.org" ];
    extra-trusted-public-keys = [
      "jag-k.cachix.org-1:aXuIIuYcjTuE8thtWB1UjeKLdZreS9huN+eNiQ3NaeA="
    ];
  };

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  # Nixpkgs 26.05 is the final maintained branch for Intel macOS. Keep this
  # separate because the normal 26.05 branch no longer supports that target.
  inputs.nixpkgs-x86_64-darwin.url = "github:NixOS/nixpkgs/nixpkgs-26.05-darwin";

  outputs = { self, nixpkgs, nixpkgs-x86_64-darwin }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      nixpkgsFor = system:
        if system == "x86_64-darwin" then nixpkgs-x86_64-darwin else nixpkgs;
      pkgsFor = system: import (nixpkgsFor system) { inherit system; };
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = pkgsFor system;
          manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          releaseDir = "target/${pkgs.stdenv.hostPlatform.rust.cargoShortTarget}/release";
          package = pkgs.rustPlatform.buildRustPackage {
            pname = "clipboard-transformer";
            version = manifest.workspace.package.version;
            src = self;

            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [
              "--package"
              "ct-cli"
              "--package"
              "ct-desktop"
            ];
            doCheck = false;

            nativeBuildInputs = [ pkgs.pkg-config ]
              ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
                pkgs.makeWrapper
              ];

            installPhase = ''
              runHook preInstall
              install -Dm0755 ${releaseDir}/clipboard-transformer \
                $out/bin/clipboard-transformer
              install -Dm0755 ${releaseDir}/clipboard-transformer-app \
                $out/bin/clipboard-transformer-app
              install -Dm0644 LICENSE \
                $out/share/licenses/clipboard-transformer/LICENSE
            '' + pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              install -Dm0644 package/linux/dev.jag-k.clipboard-transformer.desktop \
                $out/share/applications/dev.jag-k.clipboard-transformer.desktop
              service="$out/share/dbus-1/services/dev.jag-k.clipboard-transformer.service"
              install -Dm0644 package/linux/dev.jag-k.clipboard-transformer.service \
                "$service"
              substituteInPlace "$service" \
                --replace-fail /usr/bin/clipboard-transformer-app \
                  $out/bin/clipboard-transformer-app
              install -Dm0644 assets/generated/linux/app-icon.png \
                $out/share/icons/hicolor/256x256/apps/clipboard-transformer-app.png
              install -Dm0644 assets/tray.svg \
                $out/share/icons/hicolor/scalable/status/clipboard-transformer-symbolic.svg

              wrapProgram $out/bin/clipboard-transformer \
                --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [ pkgs.wayland ]}
              wrapProgram $out/bin/clipboard-transformer-app \
                --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [ pkgs.wayland ]}
            '' + pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
              app="$out/Applications/Clipboard Transformer.app"
              mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
              install -m0644 package/macos/Info.plist "$app/Contents/Info.plist"
              install -m0755 ${releaseDir}/clipboard-transformer-app \
                "$app/Contents/MacOS/Clipboard Transformer"
              install -m0644 assets/generated/macos/Assets.car \
                "$app/Contents/Resources/Assets.car"
              install -m0644 assets/generated/macos/AppIcon.icns \
                "$app/Contents/Resources/AppIcon.icns"
            '' + ''
              runHook postInstall
            '';

            meta = {
              description = "Rule-based clipboard transformer";
              homepage = "https://github.com/jag-k/clipboard-transformer";
              license = pkgs.lib.licenses.mpl20;
              mainProgram = "clipboard-transformer-app";
              platforms = pkgs.lib.platforms.linux ++ pkgs.lib.platforms.darwin;
            };
          };
        in
        {
          default = package;
          clipboard-transformer = package;
        });

      apps = forAllSystems (system:
        let
          pkgs = pkgsFor system;
          package = self.packages.${system}.default;
          guiProgram = if pkgs.stdenv.hostPlatform.isDarwin then
            pkgs.writeShellScript "clipboard-transformer" ''
              exec /usr/bin/open "${package}/Applications/Clipboard Transformer.app"
            ''
          else
            "${package}/bin/clipboard-transformer-app";
        in
        {
          default = {
            type = "app";
            program = "${guiProgram}";
          };
          cli = {
            type = "app";
            program = "${package}/bin/clipboard-transformer";
          };
        });

      devShells = forAllSystems (system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.pkg-config
              pkgs.rustc
            ];
          };
        });
    };
}
