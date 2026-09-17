.PHONY: default build check run test lint lint-fix fmt-check fmt pylint gen-docs gen-docs-check machete ci

default:
	@echo "Targets: build check run test lint lint-fix fmt-check fmt pylint gen-docs gen-docs-check machete ci"

build:
	cargo build $(ARGS)

check:
	cargo check --workspace --tests $(ARGS)

run:
	cargo run $(ARGS)

test:
	cargo test --workspace $(ARGS)

lint:
	cargo clippy --all --tests -- -D warnings

lint-fix:
	cargo clippy --all --tests --fix

fmt-check:
	cargo fmt --all -- --check
	stylua --check plugins/

fmt:
	cargo fmt --all
	stylua plugins/

pylint:
	ruff check scripts/
	ty check scripts/

gen-docs:
	cargo run -p maki-docgen

gen-docs-check:
	cargo run -p maki-docgen -- --check

machete:
	cargo machete

# Full CI check
ci: fmt-check lint pylint test gen-docs-check machete
