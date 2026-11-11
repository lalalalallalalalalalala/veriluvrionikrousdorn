.PHONY: run test build

# One-command wrappers around the xtask runner.
run:
	cargo run -- run

test:
	cargo run -- test

build:
	cargo run -- build
