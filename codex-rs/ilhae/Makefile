# ilhae-proxy Test Targets
#
# Usage:
#   make test-mock     # Run only mock tests (CI-safe, no server needed)
#   make test-real     # Run only real server tests (#[ignore] tagged)
#   make test-all      # Run everything (mock + real)
#   make test-compile  # Compile-only check (fast feedback)
#
# Timeout:
#   TIMEOUT=120        # Per-test timeout in seconds (default: 120)
#   CMD_TIMEOUT=300    # Overall command timeout (default: 300)
#   make test-domain DOMAIN=team  # Run specific domain tests

.PHONY: test-mock test-real test-all test-compile test-domain

# Timeout settings
TIMEOUT ?= 120
CMD_TIMEOUT ?= 300

# Default: mock only (CI-safe)
test: test-mock

# Mock tests — fast, no external dependencies
test-mock:
	@echo "🧪 Running mock tests (no --ignored)..."
	cargo test --no-fail-fast -- --nocapture 2>&1 | tee /tmp/test-mock.log
	@echo "✅ Mock tests complete"

# Real server tests — requires live A2A servers (with timeout)
test-real:
	@echo "🔌 Running real server tests (--ignored, timeout=$(TIMEOUT)s per test)..."
	@echo "⚠️  Requires: pnpm tauri dev (ports 4321-4324)"
	timeout $(CMD_TIMEOUT) cargo test --no-fail-fast -- --ignored --nocapture 2>&1 | tee /tmp/test-real.log
	@echo "✅ Real tests complete"

# All tests — mock + real (with timeout)
test-all:
	@echo "🧪 Running ALL tests (timeout=$(CMD_TIMEOUT)s)..."
	timeout $(CMD_TIMEOUT) cargo test --no-fail-fast -- --include-ignored --nocapture 2>&1 | tee /tmp/test-all.log
	@echo "✅ All tests complete"

# Compile-only check (fastest feedback)
test-compile:
	@echo "🔨 Compiling test crates..."
	cargo test --no-run 2>&1
	@echo "✅ All test crates compile"

# Domain-specific tests
# Usage: make test-domain DOMAIN=team
#        make test-domain DOMAIN=agent_chat
#        make test-domain DOMAIN=artifact
#        make test-domain DOMAIN=proxy
#        make test-domain DOMAIN=workflow
DOMAIN ?= team
test-domain:
	@echo "🧪 Running $(DOMAIN) tests..."
	cargo test --test $(DOMAIN) -- --nocapture 2>&1 | tee /tmp/test-$(DOMAIN).log
	@echo "✅ $(DOMAIN) tests complete"

# Domain-specific real tests
test-domain-real:
	@echo "🔌 Running $(DOMAIN) real tests..."
	cargo test --test $(DOMAIN) -- --ignored --nocapture 2>&1 | tee /tmp/test-$(DOMAIN)-real.log
	@echo "✅ $(DOMAIN) real tests complete"
