.PHONY: build test install uninstall check
build:
	cargo build --release --locked
test:
	cargo test --all
check:
	cargo fmt --all --check
	cargo clippy --all-targets --all-features -- -D warnings
install: build
	ADAPTIVE_SOURCE_DIR="$(CURDIR)" ./install.sh
uninstall:
	./uninstall.sh

