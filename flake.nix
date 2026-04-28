{
  description = "Pack - A WebAssembly package runtime with extended WIT support for recursive types";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Rust toolchain with WASM target
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
          targets = [ "wasm32-unknown-unknown" ];
        };

        buildInputs = with pkgs; [
          rustToolchain
          pkg-config
          openssl
        ] ++ lib.optionals stdenv.isDarwin [
          darwin.apple_sdk.frameworks.Security
          darwin.apple_sdk.frameworks.SystemConfiguration
        ];

        nativeBuildInputs = with pkgs; [
          rustToolchain
          pkg-config
        ];

      in
      {
        devShells.default = pkgs.mkShell {
          inherit buildInputs nativeBuildInputs;

          packages = with pkgs; [
            cargo-watch
            cargo-edit
            cargo-expand
            wasm-pack
            wasmtime
            gh
          ];

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          RUST_BACKTRACE = "1";

          shellHook = ''
            echo "Pack development environment"
            echo "Rust: $(rustc --version)"
            echo ""
            echo "Commands:"
            echo "  cargo build                        - Build packr runtime"
            echo "  cargo test --workspace             - Run all tests"
            echo "  cargo test -p packr-abi --features derive  - Run derive tests"
            echo "  cargo clippy --workspace           - Lint"
            echo "  nix run .#test                     - Run full test suite"
            echo "  nix run .#pr                       - Create PR from jj revision"
            echo "  nix run .#release -- patch           - Bump version, tag, publish"
          '';
        };

        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "packr";
          version = "0.3.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          inherit nativeBuildInputs buildInputs;

          meta = with pkgs.lib; {
            description = "A WebAssembly package runtime with extended WIT support for recursive types";
            homepage = "https://github.com/colinrozzi/pack";
            license = licenses.mit;
          };
        };

        packages.packr = self.packages.${system}.default;

        # Run full test suite (workspace + derive tests)
        packages.test = pkgs.writeShellScriptBin "pack-test" ''
          set -e
          echo "Running workspace tests..."
          cargo test --workspace
          echo ""
          echo "Running derive tests..."
          cargo test -p packr-abi --features derive
          echo ""
          echo "All tests passed!"
        '';

        # Release: bump version, commit, tag, push
        packages.release = pkgs.writeShellScriptBin "pack-release" ''
          set -e

          BUMP="''${1:-patch}"
          CURRENT=$(${pkgs.gnugrep}/bin/grep -m1 '^version = ' Cargo.toml | ${pkgs.gnused}/bin/sed 's/version = "\(.*\)"/\1/')

          IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

          case "$BUMP" in
            patch) PATCH=$((PATCH + 1)) ;;
            minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
            major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
            [0-9]*.[0-9]*.[0-9]*) IFS='.' read -r MAJOR MINOR PATCH <<< "$BUMP" ;;
            *) echo "Usage: nix run .#release -- [patch|minor|major|X.Y.Z]"; exit 1 ;;
          esac

          NEW="$MAJOR.$MINOR.$PATCH"
          echo "Bumping $CURRENT -> $NEW"

          # Update workspace version in root Cargo.toml
          ${pkgs.gnused}/bin/sed -i "0,/^version = \"$CURRENT\"/s//version = \"$NEW\"/" Cargo.toml

          # Update workspace dependency versions
          ${pkgs.gnused}/bin/sed -i "s/version = \"$CURRENT\", path/version = \"$NEW\", path/g" Cargo.toml

          # Update flake.nix version
          ${pkgs.gnused}/bin/sed -i "s/version = \"$CURRENT\"/version = \"$NEW\"/" flake.nix

          # Update Cargo.lock
          cargo update --workspace 2>/dev/null || true

          echo ""
          echo "Updated to v$NEW"
          echo ""

          # Commit and tag with jj
          if command -v jj &>/dev/null; then
            jj describe -m "release v$NEW"
            jj bookmark create "v$NEW" -r @ 2>/dev/null || jj bookmark set "v$NEW" -r @
            jj git push --bookmark "v$NEW" --allow-new

            # Also push main forward
            jj new
            jj bookmark set main -r @-
            jj git push --bookmark main
          else
            git add -A
            git commit -m "release v$NEW"
            git tag "v$NEW"
            git push origin main "v$NEW"
          fi

          echo ""
          echo "Pushed v$NEW — CI will publish to crates.io"
        '';

        # Create a PR from the current jj revision
        packages.pr = pkgs.writeShellScriptBin "pack-pr" ''
          set -e
          DESCRIPTION=$(jj log -r @ --no-graph -T 'description' 2>/dev/null)
          if [ -z "$DESCRIPTION" ] || [ "$DESCRIPTION" = "(no description set)" ]; then
            echo "Error: Current revision has no description. Run: jj describe -m 'your change'"
            exit 1
          fi

          TITLE=$(echo "$DESCRIPTION" | head -1)
          BRANCH=$(echo "$TITLE" | tr '[:upper:]' '[:lower:]' | tr ' ' '-' | tr -cd 'a-z0-9-' | ${pkgs.gnused}/bin/sed 's/--*/-/g; s/^-//; s/-$//' | head -c 50)

          echo "Creating PR: $TITLE"
          echo "Branch: $BRANCH"
          echo ""

          jj bookmark create "$BRANCH" -r @ 2>/dev/null || jj bookmark set "$BRANCH" -r @
          jj git push --bookmark "$BRANCH" --allow-new

          ${pkgs.gh}/bin/gh pr create \
            --title "$TITLE" \
            --body "$DESCRIPTION" \
            --base main \
            --head "$BRANCH"
        '';
      }
    );
}
