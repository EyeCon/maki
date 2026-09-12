.PHONY: default build check run install test lint bench bench-quick lint-fix fmt-check fmt pylint gen-docs gen-docs-check machete ci

default:
	@echo "Targets: build check run install test lint bench bench-quick lint-fix fmt-check fmt pylint gen-docs gen-docs-check machete ci"

build:
	cargo build $(ARGS)

# Types only, no codegen, no lints. Add `-p <crate>` to make it cheaper still.
check:
	cargo check --workspace --all-targets $(ARGS)

run:
	cargo run $(ARGS)

install:
	cargo install --locked --path . $(ARGS)

test:
	cargo nextest run --workspace $(ARGS)

lint:
	cargo clippy --all --all-targets -- -D warnings

# Narrow it down, e.g. `make bench ARGS='-p maki-ui --bench input'`.
bench:
	cargo bench $(ARGS)

# Dev profile: builds fast, to see a bench run. Its numbers mean little.
bench-quick:
	cargo bench --profile dev $(ARGS)

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
