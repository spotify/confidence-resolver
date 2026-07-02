#!/bin/bash
set -euo pipefail
# set e,u,o pipefail so that the script will fail if any command fails
# -e: exit immediately if a command fails
# -u: treat unset variables as an error and exit immediately
# -o pipefail: the return value of a pipeline is the status of the last command to exit with a non-zero status, or zero if no command exited with a non-zero status

CLOUDFLARE_API_TOKEN=${CLOUDFLARE_API_TOKEN:=}
CLOUDFLARE_ACCOUNT_ID=${CLOUDFLARE_ACCOUNT_ID:=}
RESOLVE_TOKEN_ENCRYPTION_KEY=${RESOLVE_TOKEN_ENCRYPTION_KEY:=}
STATE_ENCRYPTION_KEY=${STATE_ENCRYPTION_KEY:=}
CONFIDENCE_RESOLVER_ALLOWED_ORIGIN=${CONFIDENCE_RESOLVER_ALLOWED_ORIGIN:=}
CONFIDENCE_RESOLVER_STATE_URL=${CONFIDENCE_RESOLVER_STATE_URL:=}
CONFIDENCE_CLIENT_SECRET=${CONFIDENCE_CLIENT_SECRET:=}
NO_DEPLOY=${NO_DEPLOY:=}
FORCE_DEPLOY=${FORCE_DEPLOY:=}
WORKER_NAME_PREFIX=${WORKER_NAME_PREFIX:=}
WRANGLER_CONFIG_APPEND_FILE=${WRANGLER_CONFIG_APPEND_FILE:=}
WRANGLER_DEPLOY_ARGS=${WRANGLER_DEPLOY_ARGS:=}
WRANGLER_DEPLOY_ARGS_FILE=${WRANGLER_DEPLOY_ARGS_FILE:=}
WRANGLER_DEPLOY_TAG=${WRANGLER_DEPLOY_TAG:=}
WRANGLER_DEPLOY_MESSAGE=${WRANGLER_DEPLOY_MESSAGE:=}
INITIAL_WORKDIR="$(pwd)"

# CDN base URL for fetching resolver state
CDN_BASE_URL="https://confidence-resolver-state-cdn.spotifycdn.com"

if test -z "$CLOUDFLARE_API_TOKEN"; then
    echo "CLOUDFLARE_API_TOKEN must be set"
    exit 1
fi

# Default RESOLVE_TOKEN_ENCRYPTION_KEY to empty if not set
if test -z "$RESOLVE_TOKEN_ENCRYPTION_KEY"; then
    RESOLVE_TOKEN_ENCRYPTION_KEY=""
    echo "⚠️ RESOLVE_TOKEN_ENCRYPTION_KEY not set, using empty value"
fi

if test -z "$CONFIDENCE_CLIENT_SECRET"; then
    echo "CONFIDENCE_CLIENT_SECRET must be set"
    exit 1
fi

# Auto-detect Cloudflare account ID from token if not provided
if test -z "$CLOUDFLARE_ACCOUNT_ID"; then
    echo "🔍 Auto-detecting Cloudflare account ID from API token..."
    ACCOUNTS_RESP=$(curl -sS -w "%{http_code}" \
        -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
        "https://api.cloudflare.com/client/v4/accounts")
    ACCOUNTS_STATUS="${ACCOUNTS_RESP: -3}"
    ACCOUNTS_BODY="${ACCOUNTS_RESP%???}"

    if [ "$ACCOUNTS_STATUS" = "200" ] && command -v jq >/dev/null 2>&1; then
        ACCOUNT_COUNT=$(printf "%s" "$ACCOUNTS_BODY" | jq -r '.result | length')
        if [ "$ACCOUNT_COUNT" = "1" ]; then
            CLOUDFLARE_ACCOUNT_ID=$(printf "%s" "$ACCOUNTS_BODY" | jq -r '.result[0].id')
            ACCOUNT_NAME=$(printf "%s" "$ACCOUNTS_BODY" | jq -r '.result[0].name')
            echo "✅ Auto-detected account: $ACCOUNT_NAME ($CLOUDFLARE_ACCOUNT_ID)"
        elif [ "$ACCOUNT_COUNT" = "0" ]; then
            echo "❌ No Cloudflare accounts found for this API token"
            exit 1
        else
            echo "❌ Multiple Cloudflare accounts found. Please set CLOUDFLARE_ACCOUNT_ID explicitly:"
            printf "%s" "$ACCOUNTS_BODY" | jq -r '.result[] | "  - \(.name): \(.id)"'
            exit 1
        fi
    else
        echo "❌ Could not fetch Cloudflare accounts (HTTP $ACCOUNTS_STATUS)"
        exit 1
    fi
fi

# Build CDN URL from SHA256 hash of client secret (if not explicitly provided)
if test -z "$CONFIDENCE_RESOLVER_STATE_URL"; then
    SECRET_HASH=$(printf '%s' "$CONFIDENCE_CLIENT_SECRET" | sha256sum | cut -d' ' -f1)
    if [ -n "$STATE_ENCRYPTION_KEY" ]; then
        CONFIDENCE_RESOLVER_STATE_URL="${CDN_BASE_URL}/${SECRET_HASH}.enc"
        echo "🔐 Using encrypted CDN URL for state"
    else
        CONFIDENCE_RESOLVER_STATE_URL="${CDN_BASE_URL}/${SECRET_HASH}"
        echo "⚠️ No STATE_ENCRYPTION_KEY provided. Falling back to unencrypted state. An encryption key will be required in an upcoming version."
    fi
fi

# Worker name - prepend prefix if provided
if [ -n "$WORKER_NAME_PREFIX" ]; then
    WORKER_NAME="${WORKER_NAME_PREFIX}-confidence-cloudflare-resolver"
    echo "🏷️ Using prefixed worker name: $WORKER_NAME"
else
    WORKER_NAME="confidence-cloudflare-resolver"
fi

# Auto-detect Cloudflare resolver URL from workers subdomain
CLOUDFLARE_RESOLVER_URL=""
if test -n "$CLOUDFLARE_ACCOUNT_ID"; then
    echo "🔍 Auto-detecting Cloudflare Workers subdomain..."
    SUBDOMAIN_RESP=$(curl -sS -w "%{http_code}" \
        -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
        "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/workers/subdomain")
    SUBDOMAIN_STATUS="${SUBDOMAIN_RESP: -3}"
    SUBDOMAIN_BODY="${SUBDOMAIN_RESP%???}"

    if [ "$SUBDOMAIN_STATUS" = "200" ] && command -v jq >/dev/null 2>&1; then
        WORKERS_SUBDOMAIN=$(printf "%s" "$SUBDOMAIN_BODY" | jq -r '.result.subdomain // empty')
        if [ -n "$WORKERS_SUBDOMAIN" ]; then
            CLOUDFLARE_RESOLVER_URL="https://${WORKER_NAME}.${WORKERS_SUBDOMAIN}.workers.dev"
            echo "✅ Auto-detected resolver URL: $CLOUDFLARE_RESOLVER_URL"
        else
            echo "⚠️ Could not extract subdomain from API response"
        fi
    else
        echo "⚠️ Could not fetch workers subdomain (HTTP $SUBDOMAIN_STATUS). Skipping etag check."
    fi
fi

mkdir -p data
RESPONSE_FILE="data/resolver_state_current.pb"
ETAG_TOML=""
ALLOWED_ORIGIN_TOML=""
CLIENT_SECRET_TOML=""

EXTRA_HEADER=()

# Try to fetch previous etag from deployed resolver endpoint if provided
PREV_ETAG=""
PREV_DEPLOYER_VERSION=""
if [ -n "$CLOUDFLARE_RESOLVER_URL" ]; then
    RESOLVER_STATE_ETAG_URL="${CLOUDFLARE_RESOLVER_URL}/v1/state:etag"
    echo "🌐 Fetching etag and git version from $RESOLVER_STATE_ETAG_URL"
    ETAG_BODY_TMP=$(mktemp)
    ETAG_STATUS=$(curl -sS -w "%{http_code}" -o "$ETAG_BODY_TMP" "$RESOLVER_STATE_ETAG_URL") || ETAG_STATUS="000"
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
    if [ -n "$ETAG_RAW" ]; then
        ETAG_STRIPPED=$(printf '%s' "$ETAG_RAW" | sed -e 's/^W\///' -e 's/^"//' -e 's/"$//')
        ETAG_TOML=$(printf '%s' "$ETAG_STRIPPED" | sed 's/\\/\\\\/g; s/\"/\\\"/g')
    fi

    # Decrypt if using encrypted state
    if [ -n "$STATE_ENCRYPTION_KEY" ]; then
        echo "🔐 Decrypting resolver state..."
        SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
        node "$SCRIPT_DIR/decrypt_state.js" "$RESPONSE_FILE" "$STATE_ENCRYPTION_KEY"
        echo "✅ Resolver state decrypted"
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

resolve_input_path() {
    local input_path="$1"

    case "$input_path" in
        /*) printf '%s\n' "$input_path" ;;
        *) printf '%s/%s\n' "$INITIAL_WORKDIR" "$input_path" ;;
    esac
}

validate_wrangler_config_append() {
    local source_name="$1"
    local content="$2"

    while IFS= read -r line; do
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        if [ -z "$line" ] || [[ "$line" == \#* ]]; then
            continue
        fi
        if [[ "$line" == \[* ]]; then
            return
        fi
        echo "❌ ${source_name} must start with a TOML table header such as [[tail_consumers]] or [observability.logs]" >&2
        echo "   Top-level keys cannot be appended safely after existing tables in wrangler.toml." >&2
        exit 1
    done <<< "$content"
}

append_wrangler_config() {
    local source_name="$1"
    local content="$2"

    if [ -z "$content" ]; then
        return
    fi

    validate_wrangler_config_append "$source_name" "$content"

    {
        printf '\n'
        printf '# Appended by confidence-cloudflare-deployer from %s\n' "$source_name"
        printf '%s\n' "$content"
    } >> wrangler.toml

    echo "✅ Appended Wrangler config from ${source_name}"
}

add_wrangler_deploy_args_from_lines() {
    local source_name="$1"
    local content="$2"

    if [ -z "$content" ]; then
        return
    fi

    local count=0
    while IFS= read -r arg; do
        if [ -n "$arg" ]; then
            WRANGLER_DEPLOY_ARGS_ARRAY+=("$arg")
            count=$((count + 1))
        fi
    done <<< "$content"

    if [ "$count" -gt 0 ]; then
        echo "✅ Added ${count} Wrangler deploy arg(s) from ${source_name}"
    fi
}

# Verify all required files
check_file "data/resolver_state_current.pb"
# Note: encryption_key may be empty, so we just check it exists
if [ ! -f "data/encryption_key" ]; then
    echo "❌ Error: data/encryption_key was not created!" >&2
    exit 1
fi

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

# Determine queue name based on prefix
if [ -n "$WORKER_NAME_PREFIX" ]; then
    QUEUE_NAME="${WORKER_NAME_PREFIX}-flag-logs-queue"
else
    QUEUE_NAME="flag-logs-queue"
fi

# Create queue if it doesn't exist
echo "🔍 Checking if queue '$QUEUE_NAME' exists..."
QUEUE_CHECK=$(curl -sS -w "%{http_code}" \
    -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
    "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/queues?name=${QUEUE_NAME}")
QUEUE_STATUS="${QUEUE_CHECK: -3}"
QUEUE_BODY="${QUEUE_CHECK%???}"

if [ "$QUEUE_STATUS" = "200" ]; then
    QUEUE_COUNT=$(printf "%s" "$QUEUE_BODY" | jq -r '.result | length')
    if [ "$QUEUE_COUNT" = "0" ]; then
        echo "📦 Queue '$QUEUE_NAME' not found, creating..."
        CREATE_RESP=$(curl -sS -w "%{http_code}" -X POST \
            -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
            -H "Content-Type: application/json" \
            -d "{\"queue_name\": \"${QUEUE_NAME}\"}" \
            "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/queues")
        CREATE_STATUS="${CREATE_RESP: -3}"
        if [ "$CREATE_STATUS" = "200" ] || [ "$CREATE_STATUS" = "201" ]; then
            echo "✅ Queue '$QUEUE_NAME' created successfully"
        else
            echo "❌ Failed to create queue (HTTP $CREATE_STATUS)"
            echo "$CREATE_RESP"
            exit 1
        fi
    else
        echo "✅ Queue '$QUEUE_NAME' already exists"
    fi
else
    echo "⚠️ Could not check queue status (HTTP $QUEUE_STATUS)"
fi

# Create KV namespace for /metrics endpoint if it doesn't exist
if [ -n "$WORKER_NAME_PREFIX" ]; then
    KV_NAMESPACE_TITLE="${WORKER_NAME_PREFIX}-resolver-metrics"
else
    KV_NAMESPACE_TITLE="resolver-metrics"
fi

ENABLE_METRICS=${ENABLE_METRICS:=}
if [ -z "$ENABLE_METRICS" ]; then
    echo "ℹ️ ENABLE_METRICS not set; skipping KV namespace creation (/metrics endpoint disabled)"
    KV_NAMESPACE_ID=""
else

echo "🔍 Checking if KV namespace '$KV_NAMESPACE_TITLE' exists..."
KV_LIST=$(curl -sS -w "%{http_code}" \
    -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
    "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/storage/kv/namespaces?per_page=100")
KV_LIST_STATUS="${KV_LIST: -3}"
KV_LIST_BODY="${KV_LIST%???}"

KV_NAMESPACE_ID=""
if [ "$KV_LIST_STATUS" = "200" ]; then
    KV_NAMESPACE_ID=$(printf "%s" "$KV_LIST_BODY" | jq -r ".result[] | select(.title == \"${KV_NAMESPACE_TITLE}\") | .id" 2>/dev/null || true)
fi

if [ -z "$KV_NAMESPACE_ID" ]; then
    echo "📦 KV namespace '$KV_NAMESPACE_TITLE' not found, creating..."
    KV_CREATE_RESP=$(curl -sS -w "%{http_code}" -X POST \
        -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"title\": \"${KV_NAMESPACE_TITLE}\"}" \
        "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/storage/kv/namespaces")
    KV_CREATE_STATUS="${KV_CREATE_RESP: -3}"
    KV_CREATE_BODY="${KV_CREATE_RESP%???}"
    if [ "$KV_CREATE_STATUS" = "200" ] || [ "$KV_CREATE_STATUS" = "201" ]; then
        KV_NAMESPACE_ID=$(printf "%s" "$KV_CREATE_BODY" | jq -r '.result.id')
        echo "✅ KV namespace '$KV_NAMESPACE_TITLE' created (id: $KV_NAMESPACE_ID)"
    else
        echo "⚠️ Failed to create KV namespace (HTTP $KV_CREATE_STATUS), /metrics will be unavailable"
    fi
else
    echo "✅ KV namespace '$KV_NAMESPACE_TITLE' already exists (id: $KV_NAMESPACE_ID)"
fi

# Append KV binding to wrangler.toml if namespace was created
if [ -n "$KV_NAMESPACE_ID" ]; then
    cat >> wrangler.toml <<EOF

[[kv_namespaces]]
binding = "CONFIDENCE_METRICS_KV"
id = "$KV_NAMESPACE_ID"
EOF
    echo "✅ Added CONFIDENCE_METRICS_KV binding to wrangler.toml"
fi

fi  # end ENABLE_METRICS check

# Update worker name and queue name in wrangler.toml if using prefix
if [ -n "$WORKER_NAME_PREFIX" ]; then
    sed -i.tmp "s/^name = .*/name = \"$WORKER_NAME\"/" wrangler.toml
    # Update queue name in both producer and consumer sections
    sed -i.tmp "s/queue = \"flag-logs-queue\"/queue = \"$QUEUE_NAME\"/g" wrangler.toml
    echo "✅ Updated worker name to \"$WORKER_NAME\" in wrangler.toml"
    echo "✅ Updated queue name to \"$QUEUE_NAME\" in wrangler.toml"
fi

# Prepare ALLOWED_ORIGIN for TOML (escape quotes and backslashes)
if [ -n "$CONFIDENCE_RESOLVER_ALLOWED_ORIGIN" ]; then
    ALLOWED_ORIGIN_TOML=$(printf '%s' "$CONFIDENCE_RESOLVER_ALLOWED_ORIGIN" | sed 's/\\/\\\\/g; s/\"/\\\"/g')
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

if [ -n "$WRANGLER_CONFIG_APPEND_FILE" ]; then
    WRANGLER_CONFIG_APPEND_FILE_PATH=$(resolve_input_path "$WRANGLER_CONFIG_APPEND_FILE")
    if [ ! -f "$WRANGLER_CONFIG_APPEND_FILE_PATH" ]; then
        echo "❌ WRANGLER_CONFIG_APPEND_FILE does not exist: $WRANGLER_CONFIG_APPEND_FILE_PATH" >&2
        exit 1
    fi
    append_wrangler_config "WRANGLER_CONFIG_APPEND_FILE" "$(cat "$WRANGLER_CONFIG_APPEND_FILE_PATH")"
fi

# Build the worker after state is downloaded
export CARGO_TARGET_DIR=/workspace/target
export PATH="/usr/local/cargo/bin:$PATH"

# Debug: verify data files exist before build
echo "📁 Verifying data files before build..."
ls -la ../data/
echo "📁 resolver_state_current.pb size: $(wc -c < ../data/resolver_state_current.pb) bytes"
echo "📁 encryption_key size: $(wc -c < ../data/encryption_key) bytes"

# Debug: check wasm-bindgen
echo "🔧 Checking wasm-bindgen..."
which wasm-bindgen || echo "wasm-bindgen not in PATH"
wasm-bindgen --version || echo "wasm-bindgen version check failed"
echo "🔧 PATH: $PATH"
echo "🔧 CARGO_HOME: ${CARGO_HOME:-}"
wasm_tools=(/usr/local/cargo/bin/*wasm*)
if [ -e "${wasm_tools[0]}" ]; then
    printf '%s\n' "${wasm_tools[@]}"
else
    echo "no wasm tools in cargo bin"
fi

RUSTFLAGS='--cfg getrandom_backend="wasm_js"' worker-build --release

WRANGLER_DEPLOY_ARGS_ARRAY=()
if [ -n "$WRANGLER_DEPLOY_TAG" ]; then
    WRANGLER_DEPLOY_ARGS_ARRAY+=(--tag "$WRANGLER_DEPLOY_TAG")
    echo "✅ Using Wrangler deploy tag"
fi

if [ -n "$WRANGLER_DEPLOY_MESSAGE" ]; then
    WRANGLER_DEPLOY_ARGS_ARRAY+=(--message "$WRANGLER_DEPLOY_MESSAGE")
    echo "✅ Using Wrangler deploy message"
fi

if [ -n "$WRANGLER_DEPLOY_ARGS_FILE" ]; then
    WRANGLER_DEPLOY_ARGS_FILE_PATH=$(resolve_input_path "$WRANGLER_DEPLOY_ARGS_FILE")
    if [ ! -f "$WRANGLER_DEPLOY_ARGS_FILE_PATH" ]; then
        echo "❌ WRANGLER_DEPLOY_ARGS_FILE does not exist: $WRANGLER_DEPLOY_ARGS_FILE_PATH" >&2
        exit 1
    fi
    add_wrangler_deploy_args_from_lines "WRANGLER_DEPLOY_ARGS_FILE" "$(cat "$WRANGLER_DEPLOY_ARGS_FILE_PATH")"
fi

add_wrangler_deploy_args_from_lines "WRANGLER_DEPLOY_ARGS" "$WRANGLER_DEPLOY_ARGS"

# only deploy if NO_DEPLOY is not set
if test -z "$NO_DEPLOY"; then
     wrangler deploy "${WRANGLER_DEPLOY_ARGS_ARRAY[@]}"

else
     echo "NO_DEPLOY is set, skipping deploy"
fi
