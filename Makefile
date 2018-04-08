# Use this file purely for shortcuts only

all: test fmt lint

fmt:
	cargo +nightly fmt -- --write-mode=diff

lint:
	cargo +nightly clippy -- -D warnings

test: test_clean_database
	cargo test
	cargo test -- --ignored --test-threads=1

test_clean_database:
	cargo test test_clean_database -- --ignored
