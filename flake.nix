{
  description = "mark development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];

      forAllSystems =
        function:
        nixpkgs.lib.genAttrs systems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ (import rust-overlay) ];
            };
          in
          function {
            inherit pkgs;
            rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          }
        );
    in
    {
      formatter = forAllSystems (
        { pkgs, ... }:
        pkgs.writeShellApplication {
          name = "format-flake";
          runtimeInputs = [ pkgs.nixfmt ];
          text = ''exec nixfmt "$PWD/flake.nix"'';
        }
      );

      devShells = forAllSystems (
        { pkgs, rustToolchain }:
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustToolchain
              coreutils
              curl
              git
              gnutar
              just
              mise
              nodejs_24
              ripgrep
              sccache
            ];
            RUSTC_WRAPPER = "${pkgs.sccache}/bin/sccache";
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            SCCACHE_CACHE_SIZE = "20G";
            shellHook = ''
                            mark_dev_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
                            mark_dev_bin="$mark_dev_root/target/dev-bin"

                            export MARK_DEV_ROOT="$mark_dev_root"
                            export MARK_DEV_BIN="$mark_dev_bin"

                            mkdir -p "$MARK_DEV_BIN"
                            cat > "$MARK_DEV_BIN/mark" <<'MARK_DEV_SHIM'
              #!/usr/bin/env sh
              set -eu

              repo="''${MARK_DEV_ROOT:?MARK_DEV_ROOT is not set}"
              binary="$repo/target/debug/mark"

              needs_build=0
              if [ ! -x "$binary" ]; then
                needs_build=1
              else
                newer_source="$(find "$repo" \( -path "$repo/target" -o -path "$repo/.git" \) -prune -o -type f \( -name '*.rs' -o -name Cargo.toml -o -name Cargo.lock \) -newer "$binary" -print | sed -n '1p')"
                if [ -n "$newer_source" ]; then
                  needs_build=1
                fi
              fi

              if [ "$needs_build" -eq 1 ]; then
                echo "mark dev shim: building mark-cli..." >&2
                (cd "$repo" && cargo build -p mark-cli --locked >&2)
              fi

              exec "$binary" "$@"
              MARK_DEV_SHIM
                            chmod +x "$MARK_DEV_BIN/mark"
                            export PATH="$MARK_DEV_BIN:$PATH"
            '';
          };
        }
      );
    };
}
