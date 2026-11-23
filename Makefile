# Homebrew's rustup is keg-only, so cargo may not be on PATH. Appending rather
# than prepending leaves an existing toolchain in charge.
export PATH := $(PATH):/opt/homebrew/opt/rustup/bin

.PHONY: check build release kinds

## the commit gate: fmt, clippy, tests, docs
check:
	@script/check.sh

build:
	@cargo build

release:
	@cargo build --release

## regenerate the Ruby frontend's node table after bumping ruby-prism
kinds:
	@script/gen-ruby-kinds.sh
