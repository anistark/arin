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
