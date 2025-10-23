# Java OpenFeature Provider - Development Guide

This guide covers development of the Java OpenFeature provider for Confidence.

## Overview

The Java provider enables feature flag resolution in Java applications using the Confidence resolver compiled to WebAssembly.

**Key features:**
- WASM-powered local resolution
- OpenFeature SDK integration
- Maven Central distribution
- Java 17+ compatibility

## Project Structure

```
openfeature-provider/java/
├── src/
│   ├── main/java/              # Provider implementation
│   └── test/java/              # Tests
├── target/                     # Build outputs (not committed)
│   └── *.jar                   # JAR artifacts
├── pom.xml                     # Maven configuration
├── Makefile                    # Build automation
└── CLAUDE.md                   # This file
```

## Build Tools

- **Maven**: Build system and dependency management
- **Protocol Buffers**: For proto compilation
- **JUnit**: Testing framework

## Local Development

### Prerequisites

```bash
# Java 17+
java -version  # Should be 17 or later

# Maven
mvn -version

# Protocol Buffers compiler
brew install protobuf  # macOS
# or
apt-get install protobuf-compiler  # Ubuntu
```

### Setup

```bash
cd openfeature-provider/java

# Install dependencies
mvn dependency:resolve

# Build WASM (required dependency)
make -C ../.. wasm/confidence_resolver.wasm
```

### Development Workflow

```bash
# Compile (includes proto generation)
mvn compile

# Run tests
mvn test

# Package JAR
mvn package

# Clean build artifacts
mvn clean
```

### Build Outputs

Maven produces:

1. **JAR artifact** in `target/` directory
2. **Compiled protos** in `target/generated-sources/`
3. **Compiled classes** in `target/classes/`

## Testing

### Running Tests

```bash
# Run all tests
mvn test

# Run specific test class
mvn test -Dtest=ProviderTest

# Run with debugging
mvn test -X

# Skip tests during build
mvn package -DskipTests
```

### Test Structure

Tests use JUnit:

```java
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class ProviderTest {
    @Test
    void shouldResolveFlags() {
        // Test implementation
        assertEquals(expected, actual);
    }
}
```

## Building

### Local Build

```bash
# Build using Make
make build

# Or using Maven directly
mvn clean package

# Clean build artifacts
make clean
# or
mvn clean
```

### Docker Build

```bash
# Build inside Docker (from repo root)
docker build --target openfeature-provider-java.build .

# Run tests in Docker
docker build --target openfeature-provider-java.test .
```

The Docker build ensures reproducible builds with consistent tooling.

## Publishing

### Publishing Strategy

The package is published to Maven Central using a secure Docker-based workflow:

1. **Build phase (Docker)**: Compiles and tests the package
2. **Sign phase (Docker)**: GPG signs the artifacts
3. **Deploy phase (Docker)**: Publishes to Maven Central staging

**Security**: All credentials are mounted as Docker secrets and never written to image layers.

### Required Secrets

Publishing requires three secrets configured in GitHub:

1. **`MAVEN_SETTINGS`** - Complete Maven settings.xml with credentials:
   ```xml
   <settings>
     <servers>
       <server>
         <id>central</id>
         <username>your-sonatype-username</username>
         <password>your-sonatype-password</password>
       </server>
     </servers>
   </settings>
   ```

2. **`GPG_PRIVATE_KEY`** - GPG private key for signing:
   ```bash
   # Export your GPG key
   gpg --export-secret-keys --armor YOUR_KEY_ID > gpg-private.key
   ```

3. **`SIGN_KEY_PASS`** - GPG key passphrase

### Local Publishing (Manual)

For testing the publish process locally:

```bash
# Generate test GPG key
gpg --gen-key

# Export private key
gpg --export-secret-keys --armor KEY_ID > /tmp/gpg-private.key

# Create settings.xml
cat > /tmp/settings.xml <<EOF
<settings>
  <servers>
    <server>
      <id>central</id>
      <username>YOUR_USERNAME</username>
      <password>YOUR_PASSWORD</password>
    </server>
  </servers>
</settings>
EOF

# Create passphrase file
echo "your-passphrase" > /tmp/gpg.pass

# Build and publish with Docker
docker build \
  --target openfeature-provider-java.publish \
  --secret id=maven_settings,src=/tmp/settings.xml \
  --secret id=gpg_private_key,src=/tmp/gpg-private.key \
  --secret id=gpg_pass,src=/tmp/gpg.pass \
  ../..
```

### Automated Publishing

Publishing happens automatically via GitHub Actions when a new version is released:

1. Release Please creates version PR
2. Merge PR to trigger release
3. GitHub Actions builds package in Docker
4. Signs artifacts with GPG
5. Publishes to Maven Central

## Maven Configuration

### pom.xml Structure

Key sections in pom.xml:

- **Project metadata**: Group ID, artifact ID, version
- **Dependencies**: OpenFeature SDK, WASM runtime, protobuf
- **Build plugins**:
  - `protobuf-maven-plugin`: Proto compilation
  - `maven-compiler-plugin`: Java compilation
  - `maven-surefire-plugin`: Test execution
  - `maven-gpg-plugin`: Artifact signing (for publishing)
  - `nexus-staging-maven-plugin`: Maven Central deployment

### Dependency Management

```xml
<dependencies>
  <!-- OpenFeature SDK -->
  <dependency>
    <groupId>dev.openfeature</groupId>
    <artifactId>sdk</artifactId>
    <version>...</version>
  </dependency>

  <!-- Protocol Buffers -->
  <dependency>
    <groupId>com.google.protobuf</groupId>
    <artifactId>protobuf-java</artifactId>
    <version>...</version>
  </dependency>

  <!-- WASM Runtime -->
  <!-- ... -->
</dependencies>
```

## Common Tasks

### Update Protobuf Definitions

```bash
# Proto files are in ../../confidence-resolver/protos/
# and ../../wasm/proto/

# Regenerate Java code
mvn clean compile

# Protos are generated to target/generated-sources/protobuf/
```

### Add New Dependency

Edit pom.xml:

```xml
<dependency>
  <groupId>com.example</groupId>
  <artifactId>package-name</artifactId>
  <version>1.0.0</version>
</dependency>
```

Then update dependencies:

```bash
mvn dependency:resolve
```

### Update WASM Dependency

The provider depends on the WASM resolver artifact:

```bash
# Rebuild WASM from repository root
make -C ../.. wasm/confidence_resolver.wasm

# Or use Docker
docker build --target wasm-rust-guest.artifact -o ../../wasm ../..
```

### Version Bump

Versions are managed by Release Please. To manually update:

```bash
mvn versions:set -DnewVersion=1.2.3
mvn versions:commit
```

## Troubleshooting

### "WASM not found" error

**Solution**: Build WASM artifact first:
```bash
make -C ../.. wasm/confidence_resolver.wasm
```

### Proto compilation fails

**Solution**: Install protoc:
```bash
brew install protobuf  # macOS
apt-get install protobuf-compiler  # Ubuntu
```

### Maven dependency resolution issues

**Solution**: Clear local cache:
```bash
mvn dependency:purge-local-repository
mvn clean install
```

### GPG signing fails

**Solution**: Verify GPG setup:
```bash
# List keys
gpg --list-secret-keys

# Test signing
gpg --armor --detach-sign pom.xml
rm pom.xml.asc
```

### Tests fail with ClassNotFoundException

**Solution**: Ensure WASM runtime is available:
```bash
mvn dependency:tree | grep wasm
```

## Best Practices

### Code Style

- Follow Java conventions (camelCase, etc.)
- Use meaningful variable names
- Add Javadoc for public APIs
- Keep classes focused and single-purpose

### Testing

- Write unit tests for all public methods
- Use descriptive test method names
- Mock external dependencies
- Aim for high code coverage

### Dependency Management

- Keep dependencies up to date
- Review security advisories
- Use dependabot for automated updates
- Minimize dependency count

### Version Management

- Follow semantic versioning
- Use Release Please for version bumps
- Update CHANGELOG.md
- Tag releases appropriately

## Integration with OpenFeature

Basic usage example:

```java
import dev.openfeature.sdk.OpenFeatureAPI;
import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.MutableContext;
import com.spotify.confidence.ConfidenceProvider;

// Configure provider
ConfidenceProvider provider = new ConfidenceProvider.Builder()
    .clientSecret("your-client-secret")
    .build();

// Set provider
OpenFeatureAPI api = OpenFeatureAPI.getInstance();
api.setProvider(provider);

// Get client
Client client = api.getClient();

// Evaluate flag
MutableContext context = new MutableContext("user-123");
Boolean value = client.getBooleanValue("my-flag", false, context);
```

## Maven Central Release Process

### Prerequisites

- Sonatype OSSRH account
- GPG key for signing
- Access to `com.spotify` group ID (or your own)

### Steps

1. Ensure pom.xml has all required metadata:
   - `name`, `description`, `url`
   - `licenses`
   - `developers`
   - `scm`

2. Configure GPG signing in pom.xml

3. Set up `~/.m2/settings.xml` with OSSRH credentials (local) or use GitHub secrets (CI)

4. Deploy:
   ```bash
   mvn clean deploy -P release
   ```

5. Release from staging (automatic with nexus-staging-maven-plugin)

### Verification

After publishing:

1. Check Maven Central: https://search.maven.org/
2. Verify artifact signature
3. Test installation in a sample project

## Additional Resources

- **[Repository root CLAUDE.md](../../CLAUDE.md)** - Overall development guide and publishing
- **Maven Central**: https://central.sonatype.org/
- **OpenFeature Java SDK**: https://github.com/open-feature/java-sdk
- **Protocol Buffers Java**: https://protobuf.dev/getting-started/javatutorial/
- **Confidence Docs**: https://confidence.spotify.com/
