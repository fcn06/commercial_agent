# ─────────────────────────────────────────────────────────────────────────────
# Commercial B2B Standalone Agent Makefile
# (Directly powered by ssi_identity_agent, ssi_mcp_runtime, swarm_commons)
# ─────────────────────────────────────────────────────────────────────────────

SHELL := /bin/bash
BIN_NAME := commercial_b2b_agent
CONFIG := configuration/agent_identity_config.toml
PORT := 4010
OAUTH_PORT := 4012

# Default to release mode (override with DEBUG=1)
DEBUG ?= 0
ifeq ($(DEBUG),1)
  CARGO_PROFILE :=
  BUILD_MODE := debug
else
  CARGO_PROFILE := --release
  BUILD_MODE := release
endif

.PHONY: all build run test fmt clean help release dev

all: build

help:
	@echo "================================================================================"
	@echo "Commercial Standalone B2B Agent (Powered by ssi_identity_agent & ssi_mcp_runtime)"
	@echo "================================================================================"
	@echo "  make build      - Build binary [$(BUILD_MODE)] (Release by default)"
	@echo "  make run        - Run B2B Agent on port $(PORT) & OAuth on port $(OAUTH_PORT) [$(BUILD_MODE)]"
	@echo "  make test       - Execute all unit tests"
	@echo "  make fmt        - Format all code with cargo fmt"
	@echo "  make clean      - Clean target build directory"
	@echo "================================================================================"

build:
	@echo "==> Building $(BIN_NAME) [$(BUILD_MODE)]..."
	cargo build $(CARGO_PROFILE)

run:
	@echo "==> Starting Commercial B2B Agent (Port $(PORT), Identity Server: http://127.0.0.1:3075) [$(BUILD_MODE)]..."
	B2B_AGENT_PORT=$${B2B_AGENT_PORT:-$(PORT)} \
	B2B_IDENTITY_SERVER_URL=$${B2B_IDENTITY_SERVER_URL:-http://127.0.0.1:3075} \
	B2B_JWKS_URL=$${B2B_JWKS_URL:-http://127.0.0.1:3075/.well-known/jwks.json} \
	cargo run $(CARGO_PROFILE) -- --config-file $(CONFIG)

test:
	@echo "==> Running B2B Agent unit tests..."
	B2B_JWT_SECRET="test-secret" cargo test -- --nocapture

fmt:
	@echo "==> Formatting code..."
	cargo fmt --all

clean:
	@echo "==> Cleaning build artifacts..."
	cargo clean

# Dummy targets so 'make build release' or 'make run release' works cleanly
release:
	@:

dev:
	@:
