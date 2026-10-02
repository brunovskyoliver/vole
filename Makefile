.DEFAULT_GOAL := help
SHELL := /bin/sh
PYTHON ?= $(shell if [ -x /opt/homebrew/bin/python3 ]; then printf /opt/homebrew/bin/python3; elif [ -x /usr/local/bin/python3 ]; then printf /usr/local/bin/python3; else printf python3; fi)
PROFILE ?= debug

.PHONY: help mac-setup mac-check mac-build mac-run mac-verify test-macos-script

help:
	@printf '%s\n' \
	  'mac-setup  Install dependencies, pinned Rust and missing Xcode Metal tools' \
	  'mac-check  Check Xcode, Metal, Rust and guest assembler tools' \
	  'mac-build  Build target/macos/$(PROFILE)/Vole.app' \
	  'mac-run    Build and launch a new Vole.app instance' \
	  'mac-verify Run Rust checks and all five guest round trips' \
	  'test-macos-script  Test build orchestration on any host' \
	  '' 'Use PROFILE=release for an optimized build.'

mac-setup:
	bash scripts/setup-macos.sh

mac-check:
	"$(PYTHON)" scripts/macos.py check --profile "$(PROFILE)"

mac-build:
	"$(PYTHON)" scripts/macos.py build --profile "$(PROFILE)"

mac-run:
	"$(PYTHON)" scripts/macos.py run --profile "$(PROFILE)"

mac-verify:
	"$(PYTHON)" scripts/macos.py verify --profile "$(PROFILE)"

test-macos-script:
	"$(PYTHON)" -m unittest discover -s scripts/tests -p 'test_macos*.py' -v
