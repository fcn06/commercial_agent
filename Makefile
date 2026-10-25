# ─────────────────────────────────────────────────────────────────────────────
# Commercial B2B Standalone Agent Makefile
# (Directly powered by ssi_identity_agent, ssi_mcp_runtime, swarm_commons)
# ─────────────────────────────────────────────────────────────────────────────

SHELL := /bin/bash
BIN_NAME := commercial_b2b_agent
CONFIG := configuration/agent_identity_config.toml
PORT := 4010
OAUTH_PORT := 4012

.PHONY: all build run test fmt clean help

all: build

help:
	@echo "================================================================================"
	@echo "Commercial Standalone B2B Agent (Powered by ssi_identity_agent & ssi_mcp_runtime)"
	@echo "================================================================================"
	@echo "  make build      - Build release binary"
	@echo "  make run        - Run B2B Agent on port $(PORT) & OAuth on port $(OAUTH_PORT)"
	@echo "  make test       - Execute all unit tests"
	@echo "  make fmt        - Format all code with cargo fmt"
	@echo "  make clean      - Clean target build directory"
	@echo "================================================================================"

build:
	@echo "==> Building $(BIN_NAME) [Release]..."
	cargo build --release

run:
	@echo "==> Starting Commercial B2B Agent (Port $(PORT), OAuth $(OAUTH_PORT))..."
	B2B_AGENT_PORT=$${B2B_AGENT_PORT:-$(PORT)} \
	B2B_OAUTH_PORT=$${B2B_OAUTH_PORT:-$(OAUTH_PORT)} \
	cargo run --release -- --config-file $(CONFIG)

test:
	@echo "==> Running B2B Agent unit tests..."
	B2B_JWT_SECRET="test-secret" cargo test -- --nocapture

fmt:
	@echo "==> Formatting code..."
	cargo fmt --all

clean:
	@echo "==> Cleaning build artifacts..."
	cargo clean
