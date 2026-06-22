{
  description = "dx development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { nixpkgs, ... }:
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
          function {
            pkgs = import nixpkgs { inherit system; };
          }
        );
    in
    {
      devShells = forAllSystems (
        { pkgs }:
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              coreutils
              curl
              git
              gnutar
              just
              nodejs_24
              pnpm_11
              rust-analyzer
              rustc
              rustfmt
              zsh
            ];
            shellHook = ''
              dx_dev_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
              dx_dev_bin="$dx_dev_root/target/dev-bin"
              dx_dev_zdotdir="$dx_dev_root/target/dev-zdotdir"

              export DX_DEV_ROOT="$dx_dev_root"
              export DX_DEV_BIN="$dx_dev_bin"
              export DX_DEV_ZDOTDIR="$dx_dev_zdotdir"

              mkdir -p "$DX_DEV_BIN" "$DX_DEV_ZDOTDIR"
              cat > "$DX_DEV_BIN/dx" <<'DX_DEV_SHIM'
#!/usr/bin/env sh
set -eu

repo="''${DX_DEV_ROOT:?DX_DEV_ROOT is not set}"
binary="$repo/target/debug/dx"

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
  echo "dx dev shim: building dx-cli..." >&2
  (cd "$repo" && cargo build -p dx-cli --locked >&2)
fi

exec "$binary" "$@"
DX_DEV_SHIM
              chmod +x "$DX_DEV_BIN/dx"
              export PATH="$DX_DEV_BIN:$PATH"

              cat > "$DX_DEV_ZDOTDIR/.zshrc" <<'DX_DEV_ZSHRC'
export PATH="$DX_DEV_BIN:$PATH"
DX_DEV_ZSHRC

              if [ -z "''${DX_DEV_INTERACTIVE_SHELL:-}" ] && [ -t 0 ] && [ -t 1 ] && command -v zsh >/dev/null 2>&1; then
                export DX_DEV_INTERACTIVE_SHELL=1
                exec env ZDOTDIR="$DX_DEV_ZDOTDIR" zsh -i
              fi
            '';
          };
        }
      );
    };
}
