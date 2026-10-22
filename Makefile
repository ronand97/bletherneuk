# bletherneuk build entry points. Run `make help` for a summary.
SHELL := /bin/bash
CARGO ?= cargo
PREFIX ?= $(HOME)/.local
VENV := bridge/.venv

.PHONY: help web build test test-rust test-web test-bridge lint install uninstall clean

help:
	@echo "make build     build the pages and the release binary"
	@echo "make test      run the Rust, page and bridge tests"
	@echo "make lint      rustfmt, clippy and svelte-check"
	@echo "make install   build, copy blether to $(PREFIX)/bin and run 'blether install'"

web/node_modules: web/package-lock.json
	cd web && npm ci --ignore-scripts
	@touch web/node_modules

web: web/node_modules
	cd web && npm run build

build: web
	$(CARGO) build --release --locked

$(VENV): bridge/requirements-dev.txt
	python3 -m venv $(VENV)
	$(VENV)/bin/pip install --quiet --require-hashes -r bridge/requirements-dev.txt
	@touch $(VENV)

test-rust: web
	$(CARGO) test --locked

test-web: web/node_modules
	cd web && npm test

test-bridge: $(VENV)
	$(VENV)/bin/python -m pytest bridge/tests

test: test-rust test-web test-bridge

lint: web/node_modules
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets --locked -- -D warnings
	cd web && npm run check

install: build
	install -d $(PREFIX)/bin
	install -S -m 0755 target/release/blether $(PREFIX)/bin/blether
	$(PREFIX)/bin/blether install

uninstall:
	$(PREFIX)/bin/blether uninstall

clean:
	$(CARGO) clean
	rm -rf web/dist web/node_modules $(VENV)
