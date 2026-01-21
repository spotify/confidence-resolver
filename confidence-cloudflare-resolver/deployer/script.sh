#!/bin/bash
set -euo pipefail
# set e,u,o pipefail so that the script will fail if any command fails
# -e: exit immediately if a command fails
# -u: treat unset variables as an error and exit immediately
# -o pipefail: the return value of a pipeline is the status of the last command to exit with a non-zero status, or zero if no command exited with a non-zero status

CLOUDFLARE_API_TOKEN=${CLOUDFLARE_API_TOKEN:=}
CLOUDFLARE_ACCOUNT_ID=${CLOUDFLARE_ACCOUNT_ID:=}
RESOLVE_TOKEN_ENCRYPTION_KEY=${RESOLVE_TOKEN_ENCRYPTION_KEY:=}
CONFIDENCE_RESOLVER_ALLOWED_ORIGIN=${CONFIDENCE_RESOLVER_ALLOWED_ORIGIN:=}
CONFIDENCE_RESOLVER_STATE_URL=${CONFIDENCE_RESOLVER_STATE_URL:=}
CONFIDENCE_RESOLVER_STATE_ETAG_URL=${CONFIDENCE_RESOLVER_STATE_ETAG_URL:=}
CONFIDENCE_CLIENT_SECRET=${CONFIDENCE_CLIENT_SECRET:=}
NO_DEPLOY=${NO_DEPLOY:=}
FORCE_DEPLOY=${FORCE_DEPLOY:=}

# CDN base URL for fetching resolver state
CDN_BASE_URL="https://confidence-resolver-state-cdn.spotifycdn.com"

if test -z "$CLOUDFLARE_API_TOKEN"; then
    echo "CLOUDFLARE_API_TOKEN must be set"
    exit 1
fi

if test -z "$RESOLVE_TOKEN_ENCRYPTION_KEY"; then
    echo "RESOLVE_TOKEN_ENCRYPTION_KEY must be set"
    exit 1
fi

if test -z "$CONFIDENCE_CLIENT_SECRET"; then
    echo "CONFIDENCE_CLIENT_SECRET must be set"
    exit 1
fi

# Build CDN URL from SHA256 hash of client secret (if not explicitly provided)
if test -z "$CONFIDENCE_RESOLVER_STATE_URL"; then
    # Compute SHA256 hash of client secret
    SECRET_HASH=$(printf '%s' "$CONFIDENCE_CLIENT_SECRET" | sha256sum | cut -d' ' -f1)
    CONFIDENCE_RESOLVER_STATE_URL="${CDN_BASE_URL}/${SECRET_HASH}"
    echo "📦 Using CDN URL for state: ${CDN_BASE_URL}/<sha256>"
fi

mkdir -p data
RESPONSE_FILE="data/resolver_state_current.pb"
ETAG_TOML=""
ALLOWED_ORIGIN_TOML=""
VERSION_TOML=""
CLIENT_SECRET_TOML=""

EXTRA_HEADER=()

# Try to fetch previous etag from deployed resolver endpoint if provided
PREV_ETAG=""
PREV_DEPLOYER_VERSION=""
if [ -n "$CONFIDENCE_RESOLVER_STATE_ETAG_URL" ]; then
    echo "🌐 Fetching etag and git version from $CONFIDENCE_RESOLVER_STATE_ETAG_URL"
    ETAG_BODY_TMP=$(mktemp)
    ETAG_STATUS=$(curl -sS -w "%{http_code}" -o "$ETAG_BODY_TMP" "$CONFIDENCE_RESOLVER_STATE_ETAG_URL") || ETAG_STATUS="000"
    if [ "$ETAG_STATUS" = "200" ]; then
        if command -v jq >/dev/null 2>&1 && grep -q '^[[:space:]]*{' "$ETAG_BODY_TMP"; then
            PREV_ETAG=$(jq -r '.etag // empty' "$ETAG_BODY_TMP") || PREV_ETAG=""
            PREV_DEPLOYER_VERSION=$(jq -r '.version // empty' "$ETAG_BODY_TMP") || PREV_DEPLOYER_VERSION=""
            if [ -n "$PREV_ETAG" ]; then
                echo "⤵️ Previous etag from resolver: $PREV_ETAG"
            else
                echo "⚠️ Resolver returned empty ETag"
            fi
            if [ -n "$PREV_DEPLOYER_VERSION" ]; then
                echo "⤵️ Previous Resolver Version from resolver: $PREV_DEPLOYER_VERSION"
            else
                echo "⚠️ Previous Resolver Version empty from resolver"
            fi
        else
            PREV_ETAG=$(tr -d '\r' < "$ETAG_BODY_TMP")
            PREV_ETAG=$(echo -n "$PREV_ETAG" | tr -d '\n')
            if [ -n "$PREV_ETAG" ]; then
                echo "⤵️ Previous etag from resolver: $PREV_ETAG"
            else
                echo "⚠️ Resolver returned empty ETag"
            fi
        fi
    else
        echo "❌ Could not fetch etag from resolver (HTTP $ETAG_STATUS)"
    fi
    rm -f "$ETAG_BODY_TMP"
fi


DEPLOYER_VERSION=""
if command -v git >/dev/null 2>&1 && [ -d .git ]; then
    # Prefer tags that match the deployer release format confidence-cloudflare-resolver: vX.Y.Z
    if DEPLOYER_VERSION=$(git describe --tags 2>/dev/null); then
        echo "🏷️ Deployer version (tag): ${DEPLOYER_VERSION}"
    else
        echo "ℹ️ Unable to resolve deployer tag"
    fi
else
    if [ -s "/workspace/.release_tag" ]; then
        if DEPLOYER_VERSION=$(cat /workspace/.release_tag | tr -d '\n'); then
            echo "🏷️ Deployer version (baked tag): ${DEPLOYER_VERSION}"
        fi
    else
        echo "ℹ️ Baked deployer tag not found"
    fi
fi


# If version changed, force download to bypass etag and ensure fresh deploy
if [ -n "$PREV_DEPLOYER_VERSION" ] && [ -n "$DEPLOYER_VERSION" ] && [ "$PREV_DEPLOYER_VERSION" != "$DEPLOYER_VERSION" ]; then
    echo "☑️ Deployer version changed ($PREV_DEPLOYER_VERSION -> $DEPLOYER_VERSION); forcing state download and redeploy"
    FORCE_DEPLOY=1
fi
 
if [ -n "$PREV_ETAG" ]; then
    if [ -z "$FORCE_DEPLOY" ]; then
        EXTRA_HEADER+=("-H" "If-None-Match: $PREV_ETAG")
        echo "Using If-None-Match: $PREV_ETAG"
    else
        echo "⚠️ FORCE_DEPLOY is set; ignoring existing ETag"
    fi
fi

TMP_HEADER=$(mktemp)
HTTP_STATUS=$(curl -sS -w "%{http_code}" -D "$TMP_HEADER" -o "$RESPONSE_FILE" ${EXTRA_HEADER[@]+"${EXTRA_HEADER[@]}"} "$CONFIDENCE_RESOLVER_STATE_URL")

if [ "$HTTP_STATUS" = "304" ]; then
    echo "✅ Resolver state not modified (HTTP 304). Skipping the deployment"
    # No changes; keep previous ETag
    rm -f "$TMP_HEADER"
    exit 0
elif [ "$HTTP_STATUS" = "200" ]; then
    echo "✅ Download of resolver state successful"
    # Extract etag and normalize
    ETAG_RAW=$(awk -F': ' 'tolower($1)=="etag"{print $2}' "$TMP_HEADER" | tr -d '\r')
    rm -f "$TMP_HEADER"
    # Normalize ETag: drop weak prefix and surrounding quotes, then escape for TOML
    if [ -n "$ETAG_RAW" ]; then
        ETAG_STRIPPED=$(printf '%s' "$ETAG_RAW" | sed -e 's/^W\///' -e 's/^"//' -e 's/"$//')
        ETAG_TOML=$(printf '%s' "$ETAG_STRIPPED" | sed 's/\\/\\\\/g; s/\"/\\\"/g')
    fi
else
    echo "❌ Error downloading resolver state: HTTP status code $HTTP_STATUS"
    # Print response body if the file is not empty
    if [ -s "$RESPONSE_FILE" ]; then
        echo "Server response:"
        cat "$RESPONSE_FILE"
    else
        echo "No response body received"
    fi
    rm -f "$TMP_HEADER"
    exit 1
fi

echo -n "$RESOLVE_TOKEN_ENCRYPTION_KEY" > data/encryption_key

# Function to check if a file exists and is not empty
check_file() {
    if [ ! -s "$1" ]; then
        echo "❌ Error: $1 was not created or is empty!" >&2
        exit 1
    else
        echo "✅ $1 exists and is not empty"
    fi
}

# Verify all required files
check_file "data/resolver_state_current.pb"
check_file "data/encryption_key"

echo "🚀 All files successfully created and verified"

cd confidence-cloudflare-resolver

echo "🏁 Starting CloudFlare deployment"
echo "☁️ CloudFlare API token: ${CLOUDFLARE_API_TOKEN:0:5}.."
echo "☁️ CloudFlare account ID: $CLOUDFLARE_ACCOUNT_ID"


if [ -n "$CLOUDFLARE_ACCOUNT_ID" ]; then
    # Remove existing account_id line if present
    sed -i.tmp '/^account_id *= *.*$/d' wrangler.toml
    tmpfile=$(mktemp)
    echo "account_id = \"$CLOUDFLARE_ACCOUNT_ID\"" > "$tmpfile"
    cat wrangler.toml >> "$tmpfile"
    mv "$tmpfile" wrangler.toml
else
    echo "⚠️ CLOUDFLARE_ACCOUNT_ID environment variable is not set. This is required if the CloudFlare API token is of type Account, while User tokens with the correct permissions don't need this env variable set"
fi

# Prepare ALLOWED_ORIGIN for TOML (escape quotes and backslashes)
if [ -n "$CONFIDENCE_RESOLVER_ALLOWED_ORIGIN" ]; then
    ALLOWED_ORIGIN_TOML=$(printf '%s' "$CONFIDENCE_RESOLVER_ALLOWED_ORIGIN" | sed 's/\\/\\\\/g; s/\"/\\\"/g')
fi

# Prepare RESOLVER_VERSION for TOML
if [ -n "$DEPLOYER_VERSION" ]; then
    VERSION_TOML=$(printf '%s' "$DEPLOYER_VERSION" | sed 's/\\/\\\\/g; s/\"/\\\"/g')
fi

# Prepare CONFIDENCE_CLIENT_SECRET for TOML (escape quotes and backslashes)
if [ -n "$CONFIDENCE_CLIENT_SECRET" ]; then
    CLIENT_SECRET_TOML=$(printf '%s' "$CONFIDENCE_CLIENT_SECRET" | sed 's/\\/\\\\/g; s/\"/\\\"/g')
fi

# Update [vars] table with ALLOWED_ORIGIN, RESOLVER_STATE_ETAG and RESOLVER_VERSION, without duplicating the table
if [ -n "$ALLOWED_ORIGIN_TOML" ] || [ -n "$ETAG_TOML" ] || [ -n "$DEPLOYER_VERSION" ] || [ -n "$CLIENT_SECRET_TOML" ]; then
    # Remove any existing definitions to avoid duplicates
    sed -i.tmp '/^ALLOWED_ORIGIN *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^RESOLVER_STATE_ETAG *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^RESOLVER_VERSION *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^DEPLOYER_VERSION *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^CONFIDENCE_CLIENT_SECRET *= *.*$/d' wrangler.toml || true
    awk -v allowed="${ALLOWED_ORIGIN_TOML}" -v etag="${ETAG_TOML}" -v version="${DEPLOYER_VERSION}" -v client_secret="${CLIENT_SECRET_TOML}" '
        BEGIN{inserted=0}
        {
            print $0
            if (!inserted && $0 ~ /^\[vars\]/) {
                if (allowed != "") print "ALLOWED_ORIGIN = \"" allowed "\""
                if (etag != "") print "RESOLVER_STATE_ETAG = \"" etag "\""
                if (version != "") print "DEPLOYER_VERSION = \"" version "\""
                if (client_secret != "") print "CONFIDENCE_CLIENT_SECRET = \"" client_secret "\""
                inserted=1
            }
        }
    ' wrangler.toml > wrangler.toml.new && mv wrangler.toml.new wrangler.toml
    if [ -n "$ALLOWED_ORIGIN_TOML" ]; then
        echo "✅ ALLOWED_ORIGIN set to \"$CONFIDENCE_RESOLVER_ALLOWED_ORIGIN\" in wrangler.toml"
    fi
    if [ -n "$ETAG_TOML" ]; then
        echo "✅ RESOLVER_STATE_ETAG set to \"$ETAG_TOML\" in wrangler.toml"
    fi
    if [ -n "$DEPLOYER_VERSION" ]; then
        echo "✅ DEPLOYER_VERSION set to \"$DEPLOYER_VERSION\" in wrangler.toml"
    fi
    if [ -n "$CLIENT_SECRET_TOML" ]; then
        echo "✅ CONFIDENCE_CLIENT_SECRET set in wrangler.toml"
    fi
fi

# Build the worker after state is downloaded
export CARGO_TARGET_DIR=/workspace/target
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' worker-build --release

# only deploy if NO_DEPLOY is not set
if test -z "$NO_DEPLOY"; then
     wrangler deploy
else
     echo "NO_DEPLOY is set, skipping deploy"
fi
