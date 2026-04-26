{
  description = "Composite - A WebAssembly component runtime with extended WIT support for recursive types";

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

        # Build inputs needed for the project
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
        # Development shell
        devShells.default = pkgs.mkShell {
          inherit buildInputs nativeBuildInputs;

          packages = with pkgs; [
            # Rust development tools
            cargo-watch
            cargo-edit
            cargo-expand

            # WASM tools
            wasm-pack
            wasmtime

            # Debugging and profiling
            lldb
            valgrind

            # Build tools
            cmake
            gnumake

            # GitHub CLI
            gh
          ];

          # Environment variables
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          RUST_BACKTRACE = "1";

          shellHook = ''
            echo "🔧 Composite development environment"
            echo "Rust version: $(rustc --version)"
            echo "Cargo version: $(cargo --version)"
            echo ""
            echo "Available targets:"
            rustup target list --installed 2>/dev/null || rustc --print target-list | grep wasm32
            echo ""
            echo "💡 Quick commands:"
            echo "  cargo build                    - Build the runtime"
            echo "  cargo test                     - Run tests"
            echo "  cargo build --release          - Build optimized"
            echo "  cd components/sexpr && cargo build --target wasm32-unknown-unknown"
            echo "                                 - Build a WASM component"
          '';
        };

        # Package definition for the composite runtime
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "composite";
          version = "0.1.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          inherit nativeBuildInputs buildInputs;

          meta = with pkgs.lib; {
            description = "A WebAssembly component runtime with extended WIT support for recursive types";
            homepage = "https://github.com/your-repo/composite";
            license = licenses.mit;
            maintainers = [ ];
          };
        };

        # Alias for the package
        packages.composite = self.packages.${system}.default;

        # Create a PR from the current jj revision
        packages.pr = pkgs.writeShellScriptBin "pack-pr" ''
          set -e
          DESCRIPTION=$(jj log -r @ --no-graph -T 'description' 2>/dev/null)
          if [ -z "$DESCRIPTION" ] || [ "$DESCRIPTION" = "(no description set)" ]; then
            echo "Error: Current revision has no description. Run: jj describe -m 'your change'"
            exit 1
          fi

          TITLE=$(echo "$DESCRIPTION" | head -1)
          BRANCH=$(echo "$TITLE" | tr '[:upper:]' '[:lower:]' | tr ' ' '-' | tr -cd 'a-z0-9-' | head -c 50)

          echo "Creating PR: $TITLE"
          echo "Branch: $BRANCH"
          echo ""

          jj bookmark create "$BRANCH" -r @ 2>/dev/null || jj bookmark set "$BRANCH" -r @
          jj git push --bookmark "$BRANCH"

          ${pkgs.gh}/bin/gh pr create \
            --title "$TITLE" \
            --body "$DESCRIPTION" \
            --base main \
            --head "$BRANCH"
        '';
      }
    );
}
