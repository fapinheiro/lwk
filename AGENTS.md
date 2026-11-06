# AGENTS.md - Guidelines for Coding Agents

This file contains guidelines for agentic coding agents working in the LWK (Liquid Wallet Kit) repository.

## Instruction Priority and Conflict Resolution

When multiple instructions apply, follow this priority order:
1. Safety, security, and data integrity
2. Correctness and testability
3. Repository conventions (style, structure, commit format)
4. Performance and speed of execution

If instructions conflict, follow the higher-priority rule and document the tradeoff briefly in your final message.

## Project Overview

LWK is a Rust workspace containing libraries for Liquid wallets. It consists of multiple crates:
- `lwk_wollet` - Watch-only wallets based on CT descriptors
- `lwk_signer` - Signing operations
- `lwk_jade` / `lwk_ledger` - Hardware wallet integrations
- `lwk_bindings` - UniFFI bindings for Python/Kotlin/Swift/C#
- `lwk_common` - Shared utilities
- `lwk_cli` - Command line interface
- `lwk_wasm` - WebAssembly bindings
- and others

## Ambiente de desenvolvimento

**Cargo NÃO está instalado localmente nesta máquina.** Todos os builds e testes devem ser executados via Docker usando os scripts `./build.sh` e `./test.sh`.

Nunca executar `cargo` diretamente no shell — o binário não existe no PATH.

## Build Commands

```bash
# Buildar a imagem Docker de desenvolvimento (necessário antes de rodar testes)
./build.sh --version <ver>           # ex: ./build.sh --version 1.0.0
./build.sh --version <ver> --override  # sobrescreve imagem existente
```

O Dockerfile usado é `context/Dockerfile`. A imagem gerada é `lwk-server:dev-builder-<ver>`.

## Test Commands

> **IMPORTANTE:** `cargo` não está no PATH local. SEMPRE usar `./test.sh` para rodar testes.

```bash
# Rodar testes unitários + gerar fixtures
./test.sh --build-version <ver>

# Rodar apenas geração de fixtures (pula unit tests)
./test.sh --build-version <ver> --fixtures-only
```

Para rodar um teste específico diretamente via Docker (ex: durante desenvolvimento):

```bash
# Run unit tests for a specific package
docker run --rm -v "$(pwd)":/workspace -w /workspace \
  lwk-server:dev-builder-<ver> /root/.cargo/bin/cargo test -p lwk_wollet --lib

# Run a single test by name
docker run --rm -v "$(pwd)":/workspace -w /workspace \
  lwk-server:dev-builder-<ver> /root/.cargo/bin/cargo test -p lwk_wollet --lib test_name_here

# Build tests without running
docker run --rm -v "$(pwd)":/workspace -w /workspace \
  lwk-server:dev-builder-<ver> /root/.cargo/bin/cargo test --no-run
```

## Lint Commands

```bash
# Format code
cargo -q fmt

# Check formatting
cargo -q fmt --check

# Run clippy
cargo -q clippy --all-features --all-targets -- -D warnings

# Security audit
cargo audit --deny yanked

# Generate documentation
cargo -q doc --no-deps -p lwk_wollet --all-features
RUSTDOCFLAGS="-D warnings --cfg docsrs" cargo +nightly -q doc --all-features --no-deps
```

### YAML Validation
Use `yq` to validate YAML files:
`yq '.' .gitlab-ci.yml > /dev/null && echo "ok"`
or python:
`python3 -c 'import yaml; yaml.safe_load(open(".gitlab-ci.yml")); print("ok")'`

## Just Commands

The project uses `just` for complex workflows, some examples include:

```bash
just --list                           # List all recipes
just build-bindings-lib              # Build liblwk.so
just python-test-bindings            # Test Python bindings
just swift                           # Build Swift framework (macOS only)
just android                         # Build Android libs
just kotlin                          # Generate Kotlin bindings
just mdbook                          # Build documentation
```

if you're adding a complex flow, please add a just command.

## Code Style Guidelines

### Rust Version
- Rust toolchain: **1.85.0** (specified in `rust-toolchain.toml`)
- Edition: **2021**

### Imports
- Group imports: std lib → external crates → workspace crates → local modules
- Use `use crate::` for local module imports
- Re-export commonly used types at crate root (see `lwk_wollet/src/lib.rs`)

### Error Handling
- Use `thiserror` for error enums with `#[derive(thiserror::Error, Debug)]`
- Use `#[error(transparent)]` for wrapping other errors
- Use `#[from]` for automatic conversion
- Return `Result<T, Error>` from public functions
- Deny `unwrap()` in non-test code: `#![cfg_attr(not(test), deny(clippy::unwrap_used))]`

### Naming Conventions
- Types: `PascalCase` (e.g., `WolletDescriptor`, `TxBuilder`)
- Functions/Methods: `snake_case` (e.g., `full_scan_with_electrum_client`)
- Constants: `SCREAMING_SNAKE_CASE` (e.g., `LIQUID_SOCKET`)
- Feature flags: `snake_case` (e.g., `foreign_bindings`, `test_emulator`)

### Documentation
- Require docs on public items: `#![warn(missing_docs)]`
- Use `//!` for module-level documentation
- Document all public functions with examples where appropriate
- Mark feature-gated items: `#[cfg_attr(docsrs, feature(doc_cfg))]`

### Type Patterns
- Use `pub type` aliases for common types (e.g., `pub type BlindingPublicKey = elements::secp256k1_zkp::PublicKey`)
- Prefer `&str` over `&String` in function parameters
- Use builder patterns for complex construction (e.g., `TxBuilder`, `WolletBuilder`) and for constructor with large/unknown number of parameters

### Commit Messages
Format: `context: <description>`
- Context: crate/directory name or `ci`/`fix`/`feat`/`docs`/`refactor`
- For crates, do not include `lwk_` for example `lwk_wollet:` use just `wollet:`
- Breaking changes: append `!` after context
- Title: max 50 chars, imperative mood, no period
- Body: blank line after title, explain "why", use bullet points

### Feature Flags
- Default features should be minimal but functional
- Group related features logically
- Use `#[cfg(feature = "...")]` for conditional compilation
- Document feature requirements in crate README

### Testing
- Unit tests in `#[cfg(test)]` modules
- Integration tests in `tests/` directory
- Use `lwk_test_util` for shared test utilities
- E2E tests require Docker environment (Jade emulator, etc.)
- Possible panics, unchecked index array access, unwraps, are allowed in testing code.
- Do not suggest safety improvements (e.g., bounds checks, error handling) for these in tests unless they cause compile/run failures.

## Additional Resources
- CI Configuration: `.gitlab-ci.yml` (GitLab) and `.github/workflows/` (GitHub)
- Nix development: `nix develop .` for reproducible environment
