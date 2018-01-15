# Use this file purely for shortcuts only

all: test

test:
	cargo test
	cargo test -- --ignored --test-threads=1
