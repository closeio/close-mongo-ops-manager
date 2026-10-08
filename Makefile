# Development shortcuts. `make dist` builds the release archives for every
# target into dist/; pass script options with ARGS, e.g.
#   make dist ARGS="--targets linux"
#   make dist ARGS="--native-only"
# See scripts/build-release.sh --help.

CARGO ?= cargo
DOCKER ?= docker
ARGS ?=

.PHONY: build release test lint dist clean clean-docker

build:
	$(CARGO) build

release:
	$(CARGO) build --release

test:
	$(CARGO) test

lint:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings

dist:
	scripts/build-release.sh $(ARGS)

clean:
	$(CARGO) clean
	rm -rf dist

# Drop the cargo registry and target volumes used by the Docker (Linux) builds.
clean-docker:
	$(DOCKER) volume ls --quiet --filter name=close-mongo-ops-manager- | xargs -r $(DOCKER) volume rm
