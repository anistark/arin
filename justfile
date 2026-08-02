# Arin task runner. `just` with no arguments lists everything.

# Show available recipes.
default:
    @just --list

# development
# Run the daemon with no renderer. The protocol works, but nothing is drawn.
dev:
    cargo run --bin arin -- daemon --headless

# Drive a running daemon. `just run point 412 88 --display 1 --label Save`
run *ARGS:
    @cargo run --quiet --bin arin -- {{ ARGS }}

# Fastest feedback loop: typecheck without codegen.
check:
    cargo check --workspace --all-targets

build:
    cargo build --workspace --all-targets

release:
    cargo build --workspace --release

# packaging
# Build Arin.app into target/bundle. Universal when both darwin targets are installed.
# Pass an identity to sign it: `just bundle --sign "Developer ID Application: ..."`.
bundle *ARGS:
    packaging/macos/bundle.sh {{ ARGS }}

# Start Arin at login, from the bundle so the Screen Recording grant sticks.
startup-enable app="/Applications/Arin.app":
    packaging/macos/launch-agent.sh enable {{ app }}

startup-disable:
    packaging/macos/launch-agent.sh disable

test:
    cargo test --workspace

# Open the API docs for every crate.
doc:
    cargo doc --workspace --no-deps --open

clean:
    cargo clean

# quality
alias format := fmt

fmt:
    cargo fmt --all

# Formatting and clippy, at the same strictness CI uses.
lint:
    cargo fmt --all --check
    RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets

# versioning
# Every crate already inherits `version.workspace`, so the only literals that can drift
# are the path dependencies in `[workspace.dependencies]`, which need a version alongside
# the path to be publishable. Bump `[workspace.package]` and run this.

# Point every local dependency at the workspace version.
sync-version:
    #!/usr/bin/env sh
    set -eu
    version=$(awk '/^\[workspace.package\]/{f=1} f&&/^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)
    if [ -z "$version" ]; then
        echo "no version under [workspace.package]" >&2
        exit 1
    fi

    # A crate that names its own version instead of inheriting is a drift this cannot fix.
    stray=$(grep -L '^version.workspace = true' crates/*/Cargo.toml || true)
    if [ -n "$stray" ]; then
        echo "these do not inherit the workspace version, so syncing would not reach them:" >&2
        echo "$stray" >&2
        exit 1
    fi

    before=$(grep -cE '^arin[a-z-]* = \{ path = "crates/[a-z-]+", version = "'"$version"'" \}' Cargo.toml || true)
    sed -E 's|^(arin[a-z-]* = \{ path = "crates/[a-z-]+", version = )"[^"]*"|\1"'"$version"'"|' \
        Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
    total=$(grep -cE '^arin[a-z-]* = \{ path = "crates/' Cargo.toml)

    # Proves the manifest still parses and that every crate now reports the same version.
    cargo metadata --no-deps --format-version 1 >/dev/null
    echo "workspace version $version, $total local dependencies in sync ($((total - before)) rewritten)"

# invariants
# CI runs this on Linux, where a platform crate in the tree would fail to build.
# Locally it still catches a platform dependency leaking into core.

# Check core and the protocol stand alone, with no platform crate.
core:
    cargo test -p arin-protocol -p arin-core --all-targets

# Comment lines are skipped, so the rule can be written down next to the code it
# governs without tripping the check on its own wording.

# Check the product boundary: Arin draws and never actuates.
draw-only:
    #!/usr/bin/env sh
    hits=$(grep -rnE 'CGEventPost|CGEventTap|CGEventCreateMouseEvent|CGEventCreateKeyboardEvent|SendInput|XTestFake|uinput' \
      --include='*.rs' crates/ | grep -vE ':[0-9]+:[[:space:]]*//' || true)
    if [ -n "$hits" ]; then
        echo "input synthesis API referenced. Arin draws and never actuates:" >&2
        echo "$hits" >&2
        exit 1
    fi
    echo "clean: no input synthesis APIs referenced"

# Everything CI runs, in the order it runs it. Green here means green there.
ci: lint core test draw-only
