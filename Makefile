#!/usr/bin/make -f

# Commands
UV ?= uv

.PHONY: sync
sync:
	$(UV) sync

.PHONY: build
build:
	$(UV) build

.PHONY: publish
publish:
	$(UV) publish

