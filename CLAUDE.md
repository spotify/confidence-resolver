# Confidence Rust Flags Resolver - Development Guide

This document provides a comprehensive guide for AI assistants and developers working on the Confidence Rust Flags Resolver project.

## Repository Overview

The Confidence Rust Flags Resolver is a multi-language workspace implementing feature flag resolution in Rust, with WebAssembly compilation and bindings for multiple host languages.

### Repository Structure

```
confidence-resolver-rust/
├── confidence-resolver/          # Core Rust resolver library
│   ├── src/                     # Rust source code
│   ├── protos/                  # Protobuf definitions
│   ├── Cargo.toml
│   └── CLAUDE.md                # Component-specific development guide
├── confidence-cloudflare-resolver/  # Cloudflare Worker WASM build
│   ├── src/                     # Cloudflare-specific code
│   └── Cargo.toml
├── wasm-msg/                    # WASM messaging layer
│   ├── src/                     # Message passing utilities
│   ├── proto/                   # Message proto definitions
│   └── Cargo.toml
├── wasm/
│   ├── rust-guest/              # WASM guest (Rust resolver compiled to WASM)
│   │   ├── Cargo.toml
│   │   └── CLAUDE.md            # WASM build guide
│   ├── node-host/               # Node.js/TypeScript host example
│   ├── java-host/               # Java host example
│   ├── go-host/                 # Go host example
│   ├── python-host/             # Python host example
│   └── proto/                   # Shared proto definitions for hosts
├── openfeature-provider/
│   ├── js/                      # TypeScript OpenFeature provider
│   │   ├── src/                 # Provider implementation
│   │   ├── proto/               # Protobuf definitions
│   │   ├── package.json
│   │   └── CLAUDE.md            # JS provider development guide
│   └── java/                    # Java OpenFeature provider
│       ├── src/                 # Provider implementation
│       ├── pom.xml
│       └── CLAUDE.md            # Java provider development guide
├── data/                        # Sample resolver state for local dev
├── Cargo.toml                   # Rust workspace configuration
├── Dockerfile                   # Multi-stage Docker build
├── Makefile                     # Root build orchestration
└── rust-toolchain.toml          # Rust toolchain specification
```

**Component-specific guides:**
- **[confidence-resolver/CLAUDE.md](confidence-resolver/CLAUDE.md)** - Core Rust library development
- **[openfeature-provider/js/CLAUDE.md](openfeature-provider/js/CLAUDE.md)** - JavaScript provider development
- **[openfeature-provider/java/CLAUDE.md](openfeature-provider/java/CLAUDE.md)** - Java provider development
- **[wasm/rust-guest/CLAUDE.md](wasm/rust-guest/CLAUDE.md)** - WASM build and optimization

## Technology Stack

### Core Technologies
- **Rust**: Core resolver implementation (edition 2021)
- **WebAssembly**: Cross-platform distribution via WASM
- **Protobuf**: API and message definitions

### Language Bindings
- **TypeScript/Node.js**: OpenFeature provider + host example
- **Java**: OpenFeature provider + host example
- **Go**: Host example
- **Python**: Host example

### Build Tools
- **Cargo**: Rust build system
- **Make**: Build orchestration across all components
- **Docker**: Reproducible multi-stage builds
- **Yarn**: JavaScript dependency management (v4.6.0)
- **Maven**: Java dependency management

## Build System Architecture

### Docker Build Strategy

The Dockerfile uses a sophisticated multi-stage build approach:

1. **rust-base**: Base Alpine image with Rust toolchain
2. **rust-deps**: Cached dependency builds (native + WASM)
3. **rust-test-base**: Source code + dependency cache for testing
4. **Component stages**: Separate stages for each component (build/test/lint)
5. **Host stages**: Language-specific stages for each host example
6. **all**: Final stage that validates all components

**Key optimization**: Dependencies are built in a separate layer that gets cached, dramatically speeding up incremental builds.

### Makefile Hierarchy

The build system uses a hierarchical Makefile structure:

```
Root Makefile
├── confidence-resolver/Makefile
├── confidence-cloudflare-resolver/Makefile
├── wasm-msg/Makefile
├── wasm/rust-guest/Makefile
├── openfeature-provider/js/Makefile
├── openfeature-provider/java/Makefile
├── wasm/node-host/Makefile
├── wasm/java-host/Makefile
├── wasm/go-host/Makefile
└── wasm/python-host/Makefile
```

**Root Makefile targets**:
- `make all` (default): lint + test + build everything
- `make test`: Run all tests
- `make lint`: Run all linters
- `make build`: Build WASM + providers
- `make integration-test`: Run host integration tests
- `make clean`: Clean all build artifacts

## Development Workflows

### Quick Start

**Prerequisites**:
- Rust toolchain (auto-installed from `rust-toolchain.toml`)
- Node.js 20+ with Yarn 4.6.0
- Java 17+ with Maven (for Java components)
- Go 1.23+ (for Go host)
- Python 3.11+ (for Python host)
- protoc (Protocol Buffers compiler)

**Build everything:**
```bash
# Using Make (local)
make

# Or using Docker (reproducible)
docker build .
```

**Extract WASM artifact:**
```bash
docker build --target wasm-rust-guest.artifact -o wasm .
```

### Local Development (Fast Iteration)

```bash
# Build WASM (required first)
make wasm/confidence_resolver.wasm

# Work on specific components
cd openfeature-provider/js    # See js/CLAUDE.md
cd openfeature-provider/java  # See java/CLAUDE.md
cd confidence-resolver        # See confidence-resolver/CLAUDE.md
```

### Docker Development (Reproducible)

**No local tools needed** - everything runs in containers:

```bash
# Build everything with validation
docker build .

# Build specific component
docker build --target openfeature-provider-js.build .
docker build --target openfeature-provider-java.build .

# Extract artifacts
docker build --target wasm-rust-guest.artifact -o wasm .
docker build --target openfeature-provider-js.artifact -o artifacts .
```

### Testing Strategy

**Unit tests**:
- Rust: `cargo test` in each Rust crate
- JavaScript: `yarn test` (Vitest)
- Java: `mvn test` (JUnit)

**Integration tests**:
- Host examples serve as integration tests
- Each host (Node/Java/Go/Python) resolves flags using WASM
- Validates end-to-end WASM communication

**Running all tests**:
```bash
make test                    # All unit tests
make integration-test        # All host integration tests
docker build .              # Everything (tests + lint + build)
```

## Key Concepts

### WASM Architecture

The resolver is compiled to WebAssembly and called from various host languages:

```
Host (JS/Java/Go/Python)
    ↓ (message passing via wasm-msg)
WASM Guest (Rust resolver)
    ↓ (returns result)
Host
```

**Message passing**:
- Hosts communicate with WASM via protobuf messages
- `wasm-msg` crate provides the messaging layer
- Each host implements its own message serialization

### Sticky Assignments

Sticky assignments ensure users get consistent variant assignments:

**Default behavior**: Remote resolver fallback
- Local WASM attempts resolve first
- Falls back to Confidence cloud resolvers if sticky data needed
- Materializations stored server-side (90-day TTL)

See `STICKY_ASSIGNMENTS.md` for detailed documentation.

### Protobuf Schema Management

**Schema locations**:
- `confidence-resolver/protos/`: Core resolver API
- `wasm/proto/`: WASM message definitions
- `wasm-msg/proto/`: Messaging layer
- `openfeature-provider/js/proto/`: JS provider API

**Generation**:
- Rust: Generated in `build.rs` via `prost-build`
- TypeScript: Generated via `ts-proto` (see `yarn proto:gen`)
- Java: Generated via `protobuf-maven-plugin`
- Go: Generated via `protoc-gen-go`
- Python: Generated via `protoc` with Python plugin

## Environment Variables

### Docker Build
- `IN_DOCKER_BUILD=1`: Set in Docker stages to skip external dependencies

### Development
- `DEBUG=cnfd:*`: Enable debug logging in JavaScript provider
- `CONFIDENCE_FLAG_CLIENT_SECRET`: Flag client secret for testing
- `CONFIDENCE_API_CLIENT_ID`: API client ID
- `CONFIDENCE_API_CLIENT_SECRET`: API client secret

## Continuous Integration

The project uses GitHub Actions with the following workflows:

- **`ci.yml`**: Main CI workflow - builds, tests, and lints all components
- **`ci-cloudflare-deployer.yml`**: Cloudflare Worker deployment validation
- **`release-please.yml`**: Automated releases and publishing
- **`lint-pr-name.yaml`**: Validates PR titles follow conventional commits

## Release Process

Releases are managed via Release Please (see `.release-please-manifest.json`).

**Version management**:
- `openfeature-provider/js`: Versioned independently
- `openfeature-provider/java`: Versioned independently
- Cargo crates: Versioned independently
- WASM artifact: Tagged with git releases

## Publishing & Deployment

The project uses secure, automated publishing workflows for both Java and JavaScript providers.

### 🚨 CRITICAL SECURITY PRINCIPLE 🚨

**SECRETS MUST NEVER BE WRITTEN TO DOCKER LAYERS**

When working with Docker builds and secrets:

- ❌ **NEVER** use `echo`, `cat`, or any command that writes secrets to files in RUN commands
- ❌ **NEVER** use `ENV` with secret values
- ❌ **NEVER** write credentials to configuration files (`.npmrc`, `settings.xml`, etc.) in the Dockerfile
- ✅ **ALWAYS** use `--mount=type=secret` to mount secrets temporarily during RUN execution
- ✅ **ALWAYS** ensure secrets are only available in memory, never persisted to layers

**Why this matters:**
- Docker layers are immutable and can be extracted from images
- Once a secret is written to a layer, it persists even if deleted in a later layer
- Anyone with access to the image can extract secrets from intermediate layers
- This applies to ALL stages, not just the final image

**Correct pattern:**
```dockerfile
# ✅ CORRECT - Secret mounted, never written to filesystem
RUN --mount=type=secret,id=my_secret,target=/path/to/secret \
    command-that-uses /path/to/secret
```

**Incorrect patterns:**
```dockerfile
# ❌ WRONG - Writes secret to layer
RUN --mount=type=secret,id=my_secret \
    echo "secret=$(cat /run/secrets/my_secret)" > config.file

# ❌ WRONG - Secret persists in layer
RUN --mount=type=secret,id=my_secret \
    cat /run/secrets/my_secret > ~/.npmrc && \
    npm publish

# ❌ WRONG - Environment variable in layer
ENV SECRET_VALUE=sensitive-data
```

If you're unsure whether your code writes secrets to layers, ask before proceeding.

### Java Provider (Maven Central)

**Publishing Strategy:**
- Build and test executed in Docker for reproducibility
- Maven credentials mounted as Docker secrets (never written to filesystem)
- GPG signing for artifact verification
- Automatic publishing on release via GitHub Actions

**Required GitHub Secrets:**
- `MAVEN_SETTINGS` - Complete Maven `settings.xml` file with credentials:
  ```xml
  <settings>
    <servers>
      <server>
        <id>central</id>
        <username>your-maven-username</username>
        <password>your-maven-password</password>
      </server>
    </servers>
  </settings>
  ```
- `GPG_PRIVATE_KEY` - GPG private key for signing (export with `gpg --export-secret-keys --armor KEY_ID`)
- `SIGN_KEY_PASS` - GPG key passphrase

**Security features:**
- 🔒 **Secrets mounted during build only, NEVER written to filesystem or Docker layers**
- GPG signing ensures artifact integrity
- Scoped credentials (Maven Central only)

**Implementation details:**
The `settings.xml` file is mounted directly to `/root/.m2/settings.xml` using Docker's secret mount mechanism. Maven reads it during the build, but it never becomes part of any image layer. Similarly, the GPG private key is piped directly to `gpg --import` without touching the filesystem.

**Local testing:**
```bash
docker build \
  --target openfeature-provider-java.publish \
  --secret id=maven_settings,src=/path/to/settings.xml \
  --secret id=gpg_private_key,src=/path/to/gpg.key \
  --secret id=gpg_pass,src=/path/to/gpg.pass \
  .
```

See **[openfeature-provider/java/CLAUDE.md](openfeature-provider/java/CLAUDE.md)** for Java-specific development details.

### JavaScript Provider (npm)

**Publishing Strategy:**
- Build phase (Docker): Reproducible builds create package tarball (`npm pack`)
- Publish phase (GitHub Actions): OIDC authentication publishes the tarball
- No long-lived tokens required
- Automatic provenance statements for supply chain security

**Required GitHub Secrets:**
- None! Uses OpenID Connect (OIDC) authentication

**Required npm Configuration:**
1. Go to your package on npmjs.com
2. Navigate to package settings
3. Enable "Trusted Publishers" (GitHub Actions)
4. Add repository: `spotify/confidence-resolver-rust`
5. Specify workflow: `.github/workflows/release-please.yml`

**GitHub Actions Permissions:**
The publish job requires `id-token: write` permission for OIDC authentication (already configured in workflow).

**Security features:**
- 🔒 **No secrets in Docker layers - package built in Docker, published outside with OIDC**
- No token management - OIDC tokens are short-lived and auto-rotated
- Cryptographic provenance statements prove package origin
- Compliant with npm's security requirements (granular token phase-out)

**Implementation details:**
Docker builds the package and creates a tarball (`npm pack`). The tarball is extracted from the Docker image and published by GitHub Actions using OIDC authentication. This means no npm credentials ever enter the Docker build environment.

**Local testing:**
```bash
# Build and extract package tarball
docker build \
  --target openfeature-provider-js.artifact \
  -o ./artifacts \
  .

# Inspect the tarball
tar -tzf ./artifacts/package.tgz

# Test publish (requires npm login)
npm publish ./artifacts/package.tgz --dry-run
```

See **[openfeature-provider/js/CLAUDE.md](openfeature-provider/js/CLAUDE.md)** for JavaScript-specific development details.

### WASM Artifact Publishing

WASM artifacts are automatically attached to GitHub Releases when new versions are tagged.

**Workflow:**
1. Release Please creates version tags
2. GitHub Actions builds WASM artifact in Docker
3. Artifact uploaded to GitHub Release

**Accessing published WASM:**
- Download from GitHub Releases: `https://github.com/spotify/confidence-resolver-rust/releases`
- Or reference in projects via release URLs

### Deployment Checklist for Maintainers

**Initial setup (one-time):**
- [ ] Configure npm Trusted Publishers for `@spotify-confidence/openfeature-server-provider-local`
- [ ] Add `MAVEN_SETTINGS` secret to GitHub repository
- [ ] Add `GPG_PRIVATE_KEY` secret to GitHub repository
- [ ] Add `SIGN_KEY_PASS` secret to GitHub repository

**For each release:**
- [ ] Merge Release Please PR
- [ ] Verify GitHub Actions workflows complete successfully
- [ ] Confirm packages appear on npm and Maven Central
- [ ] Verify WASM artifact attached to GitHub Release

**Security maintenance:**
- [ ] Rotate Maven credentials if compromised
- [ ] Rotate GPG key before expiration
- [ ] Review npm Trusted Publisher configuration annually

## Performance Considerations

### WASM Build Size

The WASM artifact is optimized for size using:
- `opt-level = "z"` (optimize for size)
- `lto = true` (link-time optimization)
- `codegen-units = 1` (better optimization)
- `strip = "symbols"` (remove debug symbols)

Current typical size: ~400-600 KB

See **[wasm/rust-guest/CLAUDE.md](wasm/rust-guest/CLAUDE.md)** for WASM optimization details.

### Dependency Caching

Docker builds cache dependencies separately from source:
- Rust: Dependencies built with dummy source files
- JavaScript: `yarn install` with just `package.json`/`yarn.lock`
- Java: `mvn dependency:go-offline` with just `pom.xml`

This dramatically speeds up incremental builds.

## Code Style & Conventions

### Rust
- Follow `rustfmt` defaults
- Use `clippy` with default lints
- Edition 2021 features encouraged

### TypeScript
- ESM modules only
- Strict TypeScript configuration
- Vitest for testing

### Commit Messages
- Use conventional commits format
- Auto-generated commits include Claude Code attribution

### Branch Naming
- Feature branches: `<username>/<feature-name>`
- Example: `nicklasl/secure-publishing`

## Documentation

- **README.md**: High-level project overview
- **STICKY_ASSIGNMENTS.md**: Detailed sticky assignment documentation
- **CLAUDE.md** (this file): Repository-level development guide
- **Component CLAUDE.md files**: Component-specific development guides
  - `confidence-resolver/CLAUDE.md`
  - `openfeature-provider/js/CLAUDE.md`
  - `openfeature-provider/java/CLAUDE.md`
  - `wasm/rust-guest/CLAUDE.md`

## Additional Resources

- **Component READMEs**: Each component directory contains specific documentation
- **Test files**: See `*_test.rs`, `*.test.ts`, or `*Test.java` for usage examples
- **Dockerfile**: Reference for build dependencies and configuration
- **GitHub Issues**: For bug reports and feature requests
- **STICKY_ASSIGNMENTS.md**: Deep dive into sticky assignment behavior
