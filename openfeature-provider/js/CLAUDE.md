# JavaScript OpenFeature Provider - Development Guide

This guide covers development of the JavaScript/TypeScript OpenFeature provider for Confidence.

## Overview

The JavaScript provider (`@spotify-confidence/openfeature-server-provider-local`) enables feature flag resolution in Node.js and browser environments using the Confidence resolver compiled to WebAssembly.

**Key features:**
- Dual builds: Node.js and browser
- WASM-powered local resolution
- Remote resolver fallback for sticky assignments
- TypeScript support with full type definitions
- OpenFeature SDK integration

## Project Structure

```
openfeature-provider/js/
├── src/
│   ├── index.node.ts          # Node.js entry point
│   ├── index.browser.ts       # Browser entry point
│   ├── proto/                 # Generated TypeScript from protos (not committed)
│   └── *.ts                   # Provider implementation
├── proto/
│   ├── api.proto              # API definitions
│   └── messages.proto         # Message definitions
├── dist/
│   ├── index.node.js          # Node.js build output
│   ├── index.browser.js       # Browser build output
│   └── *.d.ts                 # TypeScript definitions
├── package.json               # Dependencies and scripts
├── yarn.lock                  # Locked dependencies
├── tsconfig.json              # TypeScript configuration
├── tsdown.config.ts           # Build configuration
├── vitest.config.ts           # Test configuration
├── Makefile                   # Build automation
└── CLAUDE.md                  # This file
```

## Build Tools

- **Yarn 4.6.0**: Package manager (via Corepack)
- **tsdown**: TypeScript bundler for dual Node/browser builds
- **ts-proto**: Protocol Buffer compiler for TypeScript
- **Vitest**: Testing framework
- **protoc**: Protocol Buffer compiler

## Local Development

### Prerequisites

```bash
# Node.js 20+
node --version  # Should be 20.x or later

# Enable Corepack for Yarn
corepack enable

# Protocol Buffers compiler
brew install protobuf  # macOS
# or
apt-get install protobuf-compiler  # Ubuntu
```

### Setup

```bash
cd openfeature-provider/js

# Install dependencies
yarn install

# Generate TypeScript from protos
yarn proto:gen

# Build WASM (required dependency)
make -C ../.. wasm/confidence_resolver.wasm
```

### Development Workflow

```bash
# Watch mode for development
yarn dev

# Generate protos
yarn proto:gen

# Build
yarn build

# Test
yarn test

# Test with coverage
yarn test --coverage
```

### Build Outputs

The build produces three artifacts:

1. **`dist/index.node.js`** - Node.js CommonJS bundle
2. **`dist/index.browser.js`** - Browser ESM bundle
3. **`dist/index.node.d.ts`** - TypeScript definitions

### Package Exports

The package.json exports configuration:

```json
{
  "exports": {
    ".": {
      "node": {
        "types": "./dist/index.node.d.ts",
        "default": "./dist/index.node.js"
      },
      "browser": {
        "types": "./dist/index.browser.d.ts",
        "default": "./dist/index.browser.js"
      }
    }
  }
}
```

## Testing

### Running Tests

```bash
# Run all tests
yarn test

# Run tests in watch mode
yarn test --watch

# Run specific test file
yarn test provider.test.ts

# Run with coverage
yarn test --coverage

# Exclude E2E tests (default)
yarn test --exclude='**/*.e2e.test.ts'

# Run E2E tests (requires credentials)
yarn test e2e.test
```

### E2E Tests

End-to-end tests require Confidence API credentials:

```bash
# Create .env.test file
echo "CONFIDENCE_FLAG_CLIENT_SECRET=your-secret" > .env.test
echo "CONFIDENCE_API_CLIENT_ID=your-client-id" >> .env.test
echo "CONFIDENCE_API_CLIENT_SECRET=your-api-secret" >> .env.test

# Run E2E tests
make test-e2e
```

### Test Structure

Tests use Vitest:

```typescript
import { describe, it, expect } from 'vitest';

describe('Provider', () => {
  it('should resolve flags', () => {
    // Test implementation
    expect(result).toBe(expected);
  });
});
```

## Building

### Local Build

```bash
# Build using Make
make build

# Or using Yarn directly
yarn build

# Clean build artifacts
make clean
```

### Docker Build

```bash
# Build inside Docker
docker build --target openfeature-provider-js.build ../..

# Extract package tarball for publishing
docker build --target openfeature-provider-js.artifact -o ./artifacts ../..
```

The Docker build ensures reproducible builds across environments.

## Publishing

### Publishing Strategy

The package uses a two-phase publishing approach:

1. **Build phase (Docker)**: Creates reproducible package tarball
2. **Publish phase (GitHub Actions)**: Publishes using OIDC authentication

**No npm tokens needed** - publishing uses OpenID Connect for secure, token-less authentication.

### npm Trusted Publishers Setup

To enable OIDC publishing, configure npm Trusted Publishers:

1. Go to https://www.npmjs.com/package/@spotify-confidence/openfeature-server-provider-local/access
2. Click "Trusted Publishers"
3. Add GitHub Actions publisher:
   - **Repository**: `spotify/confidence-resolver-rust`
   - **Workflow**: `.github/workflows/release-please.yml`
   - **Environment**: (leave empty)

### Local Publishing (Manual)

For manual testing only (production uses GitHub Actions):

```bash
# Build package tarball
npm pack

# Dry run (doesn't publish)
npm publish --dry-run

# Actual publish (requires npm login and permissions)
npm publish --access public
```

### Automated Publishing

Publishing happens automatically via GitHub Actions when a new version is released:

1. Release Please creates version PR
2. Merge PR to trigger release
3. GitHub Actions builds package in Docker
4. Publishes to npm using OIDC (no token needed)
5. Includes provenance attestation

## Dependencies

### Production Dependencies

```json
{
  "@bufbuild/protobuf": "^2.9.0"
}
```

### Development Dependencies

- TypeScript tooling: `tsdown`, `@types/node`
- Testing: `vitest`, `@vitest/coverage-v8`
- OpenFeature SDK: `@openfeature/server-sdk`, `@openfeature/core`
- Protocol Buffers: `ts-proto`
- Utilities: `debug`, `dotenv`

### Peer Dependencies

```json
{
  "debug": "^4.4.3"  // Optional
}
```

## Configuration Files

### tsconfig.json

TypeScript compiler configuration:
- Target: ES2022
- Module: ESNext
- Strict mode enabled
- Paths mapped for proto imports

### tsdown.config.ts

Build configuration for dual Node/browser bundles:
- Entry points: `index.node.ts`, `index.browser.ts`
- Format: CommonJS for Node, ESM for browser
- External dependencies properly marked
- TypeScript declarations generated

### vitest.config.ts

Test configuration:
- Test environment: node
- Coverage provider: v8
- E2E tests excluded by default

## Common Tasks

### Update Protobuf Definitions

```bash
# Edit proto files in proto/ directory
vim proto/api.proto

# Regenerate TypeScript
yarn proto:gen

# Rebuild
yarn build
```

### Add New Dependency

```bash
# Add production dependency
yarn add package-name

# Add dev dependency
yarn add -D package-name

# Update lockfile
yarn install
```

### Debug Logging

Enable debug logging during development:

```bash
# Enable all Confidence debug logs
DEBUG=cnfd:* yarn test

# Enable specific module
DEBUG=cnfd:provider yarn test
```

### Update WASM Dependency

The provider depends on the WASM resolver artifact:

```bash
# Rebuild WASM from repository root
make -C ../.. wasm/confidence_resolver.wasm

# Or use Docker
docker build --target wasm-rust-guest.artifact -o ../../wasm ../..
```

## Troubleshooting

### "WASM not found" error

**Solution**: Build WASM artifact first:
```bash
make -C ../.. wasm/confidence_resolver.wasm
```

### Proto generation fails

**Solution**: Install protoc:
```bash
brew install protobuf  # macOS
apt-get install protobuf-compiler  # Ubuntu
```

### Wrong Yarn version

**Solution**: Enable Corepack:
```bash
corepack enable
```

### Type errors after proto regeneration

**Solution**: Clean and rebuild:
```bash
make clean
yarn proto:gen
yarn build
```

### Tests fail with module not found

**Solution**: Ensure dependencies are installed:
```bash
yarn install
```

## Best Practices

### Code Style

- Use TypeScript strict mode
- Follow ESM module conventions
- Use async/await for asynchronous operations
- Add JSDoc comments for public APIs

### Testing

- Write tests alongside implementation
- Use descriptive test names
- Test both Node and browser code paths
- Mock external dependencies

### Commits

- Follow conventional commits format
- Reference issue numbers when applicable
- Keep commits focused and atomic

### Pull Requests

- Update tests for new features
- Ensure CI passes before requesting review
- Update documentation if adding public APIs

## Integration with OpenFeature

Basic usage example:

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { ConfidenceProvider } from '@spotify-confidence/openfeature-server-provider-local';

// Configure provider
const provider = new ConfidenceProvider({
  clientSecret: 'your-client-secret',
  // ... other options
});

// Set provider
OpenFeature.setProvider(provider);

// Get client
const client = OpenFeature.getClient();

// Evaluate flag
const value = await client.getBooleanValue('my-flag', false, {
  targetingKey: 'user-123'
});
```

## Additional Resources

- **[Repository root CLAUDE.md](../../CLAUDE.md)** - Overall development guide and publishing
- **[README.md](README.md)** - User-facing documentation
- **[CHANGELOG.md](CHANGELOG.md)** - Version history
- **OpenFeature Docs**: https://openfeature.dev/
- **Confidence Docs**: https://confidence.spotify.com/
