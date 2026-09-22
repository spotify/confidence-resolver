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
ENABLE_STICKY_ASSIGNMENTS=${ENABLE_STICKY_ASSIGNMENTS:=}
FORCE_APPLY=${FORCE_APPLY:=}
ENABLE_APPLY_DEDUP=${ENABLE_APPLY_DEDUP:=}
INITIAL_WORKDIR="$(pwd)"

# CDN base URL for fetching resolver state
CDN_BASE_URL="https://confidence-resolver-state-cdn.spotifycdn.com"

if test -z "$CLOUDFLARE_API_TOKEN"; then
    echo "CLOUDFLARE_API_TOKEN must be set"
    exit 1
fi

# RESOLVE_TOKEN_ENCRYPTION_KEY is resolved after the worker name and account ID
# are determined (see the auto-generation block below).

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

# --- Resolve Token Encryption Key ---
# Auto-generate if not provided and no existing secret on the worker.
# The key is stored as a Cloudflare Worker secret so it persists across deploys.
SET_SECRET_AFTER_DEPLOY=""
if test -n "$RESOLVE_TOKEN_ENCRYPTION_KEY"; then
    echo "🔐 RESOLVE_TOKEN_ENCRYPTION_KEY provided"
    SET_SECRET_AFTER_DEPLOY="true"
else
    echo "🔍 Checking for existing RESOLVE_TOKEN_ENCRYPTION_KEY secret on worker '$WORKER_NAME'..."
    SECRETS_RESP=$(curl -sS -w "%{http_code}" \
        -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
        "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/workers/scripts/${WORKER_NAME}/secrets" 2>/dev/null) || SECRETS_RESP="000"
    SECRETS_STATUS="${SECRETS_RESP: -3}"
    SECRETS_BODY="${SECRETS_RESP%???}"

    HAS_SECRET="false"
    if [ "$SECRETS_STATUS" = "200" ] && command -v jq >/dev/null 2>&1; then
        HAS_SECRET=$(printf "%s" "$SECRETS_BODY" | jq -r '[.result[] | select(.name == "RESOLVE_TOKEN_ENCRYPTION_KEY")] | length > 0' 2>/dev/null || echo "false")
    fi

    if [ "$HAS_SECRET" = "true" ]; then
        echo "✅ Existing RESOLVE_TOKEN_ENCRYPTION_KEY secret found on worker"
    else
        echo "🔑 Auto-generating RESOLVE_TOKEN_ENCRYPTION_KEY..."
        RESOLVE_TOKEN_ENCRYPTION_KEY=$(node -e "console.log(require('crypto').randomBytes(16).toString('base64'))")
        SET_SECRET_AFTER_DEPLOY="true"
        echo "✅ Generated encryption key for resolve tokens"
    fi
fi

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
echo "🚀 All files successfully created and verified"

cd confidence-cloudflare-resolver

echo "🏁 Starting CloudFlare deployment"
echo "☁️ CloudFlare API token: ${CLOUDFLARE_API_TOKEN:0:5}.."
echo "☁️ CloudFlare account ID: $CLOUDFLARE_ACCOUNT_ID"

# ---------------------------------------------------------------------------
# Flag-log sink: "queue" (default) or "logpush".
#
# queue   — flag logs publish to the flag-logs queue shards created above and
#           the worker's queue consumer batches them.
# logpush — flag logs are written to console.log, Cloudflare Logpush captures
#           them into R2, and the worker's cron trigger aggregates, delivers
#           and deletes. Nothing is held in isolate memory between requests,
#           so an isolate eviction cannot lose a log for a completed request.
#
# Queue bindings are created under either sink, so switching FLAG_LOG_SINK
# back to "queue" is an immediate rollback that also drains whatever is still
# queued.
# ---------------------------------------------------------------------------
FLAG_LOG_SINK=${FLAG_LOG_SINK:-queue}
FLAG_LOG_SINK=$(printf '%s' "$FLAG_LOG_SINK" | tr '[:upper:]' '[:lower:]')
if [ "$FLAG_LOG_SINK" != "queue" ] && [ "$FLAG_LOG_SINK" != "logpush" ]; then
    echo "❌ FLAG_LOG_SINK must be \"queue\" or \"logpush\", got: $FLAG_LOG_SINK" >&2
    exit 1
fi

# Logpush delivers batches roughly once a minute regardless of its upload
# settings, so polling faster than that only burns invocations.
FLAG_LOGS_AGGREGATOR_CRON=${FLAG_LOGS_AGGREGATOR_CRON:-* * * * *}

# Fails fast if CLOUDFLARE_API_TOKEN cannot do what this deploy needs.
#
# Every capability is probed with a cheap read against the same endpoint the
# deploy will later write to, and against the resolved account. A permission
# gap then surfaces here, naming the scope to add, instead of half-way through
# after some resources already exist.
#
# Probing the account explicitly matters: a token valid for one account
# returns an indistinguishable authentication error for another, so "R2 is
# enabled" and "this token can see R2" are different questions and only the
# second one is answered here.
preflight_api_permissions() {
    local missing=0
    local base="https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}"
    local label perm url code

    echo "🔐 Verifying API token permissions on account ${CLOUDFLARE_ACCOUNT_ID}..."

    check_perm() {
        label="$1"; perm="$2"; url="$3"
        code=$(curl -sS -o /dev/null -w "%{http_code}" \
            -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" "$url")
        if [ "$code" = "200" ]; then
            echo "   ✅ ${label}"
        else
            echo "   ❌ ${label} (HTTP ${code}) — token needs: ${perm}" >&2
            missing=1
        fi
    }

    check_perm "Workers Scripts" "Account > Workers Scripts > Edit" "${base}/workers/scripts"
    check_perm "Workers Queues"  "Account > Workers Queues > Edit"  "${base}/queues"

    if [ "$FLAG_LOG_SINK" = "logpush" ]; then
        check_perm "R2 Storage" "Account > Workers R2 Storage > Edit" "${base}/r2/buckets"
        check_perm "Logpush"    "Account > Logs > Edit"               "${base}/logpush/jobs"
    fi

    if [ -n "${ENABLE_METRICS:-}" ] || [ -n "${ENABLE_STICKY_ASSIGNMENTS:-}" ]; then
        check_perm "Workers KV" "Account > Workers KV Storage > Edit" \
            "${base}/storage/kv/namespaces"
    fi

    if [ "$missing" -ne 0 ]; then
        {
            echo ""
            echo "❌ CLOUDFLARE_API_TOKEN lacks permissions required for this deploy."
            echo "   Create or edit a token at https://dash.cloudflare.com/profile/api-tokens"
            echo "   and make sure it is scoped to account ${CLOUDFLARE_ACCOUNT_ID}."
            if [ "$FLAG_LOG_SINK" = "logpush" ]; then
                echo "   FLAG_LOG_SINK=logpush additionally requires R2 and Logpush access,"
                echo "   and Logpush requires the Workers Paid plan."
            fi
        } >&2
        exit 1
    fi
    echo "✅ API token has every permission this deploy needs"
}

preflight_api_permissions


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

# Number of flag-log queue shards (default 1 for backward compat).
# Each shard adds ~5K msg/sec write capacity.
FLAG_LOGS_QUEUE_COUNT=${FLAG_LOGS_QUEUE_COUNT:-1}
if [[ ! "$FLAG_LOGS_QUEUE_COUNT" =~ ^[1-9][0-9]*$ ]] || [ "$FLAG_LOGS_QUEUE_COUNT" -gt 9999 ]; then
    echo "FLAG_LOGS_QUEUE_COUNT must be a positive integer between 1 and 9999" >&2
    exit 1
fi

ensure_queue() {
    local Q_NAME="$1"
    echo "🔍 Checking if queue '$Q_NAME' exists..."
    local Q_CHECK
    Q_CHECK=$(curl -sS -w "%{http_code}" \
        -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
        "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/queues?name=${Q_NAME}")
    local Q_STATUS="${Q_CHECK: -3}"
    local Q_BODY="${Q_CHECK%???}"

    if [ "$Q_STATUS" = "200" ]; then
        local Q_COUNT
        Q_COUNT=$(printf "%s" "$Q_BODY" | jq -r '.result | length')
        if [ "$Q_COUNT" = "0" ]; then
            echo "📦 Queue '$Q_NAME' not found, creating..."
            local Q_CREATE
            Q_CREATE=$(curl -sS -w "%{http_code}" -X POST \
                -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
                -H "Content-Type: application/json" \
                -d "{\"queue_name\": \"${Q_NAME}\"}" \
                "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/queues")
            local Q_CREATE_STATUS="${Q_CREATE: -3}"
            if [ "$Q_CREATE_STATUS" = "200" ] || [ "$Q_CREATE_STATUS" = "201" ]; then
                echo "✅ Queue '$Q_NAME' created successfully"
            else
                echo "❌ Failed to create queue '$Q_NAME' (HTTP $Q_CREATE_STATUS)"
                echo "$Q_CREATE"
                return 1
            fi
        else
            echo "✅ Queue '$Q_NAME' already exists"
        fi
    else
        echo "⚠️ Could not check queue '$Q_NAME' status (HTTP $Q_STATUS)"
    fi
}

# Create flag-log queue shards.
# Shard 0: "flag-logs-queue" (backward compatible name)
# Shard 1+: "flag-logs-queue-1", "flag-logs-queue-2", ...
BASE_QUEUE_NAME="flag-logs-queue"
if [ -n "$WORKER_NAME_PREFIX" ]; then
    BASE_QUEUE_NAME="${WORKER_NAME_PREFIX}-flag-logs-queue"
fi

for ((queue_index = 1; queue_index <= FLAG_LOGS_QUEUE_COUNT; queue_index++)); do
    if [ "$queue_index" -eq 1 ]; then
        SHARD_NAME="$BASE_QUEUE_NAME"
    else
        SHARD_NAME="${BASE_QUEUE_NAME}-${queue_index}"
    fi
    ensure_queue "$SHARD_NAME" || exit 1
done

# Append extra shard bindings (shard 2+) to wrangler.toml.
# Shard 1 is already in the checked-in wrangler.toml as "flag-logs-queue".
for ((queue_index = 2; queue_index <= FLAG_LOGS_QUEUE_COUNT; queue_index++)); do
    if [ -n "$WORKER_NAME_PREFIX" ]; then
        SHARD_NAME="${WORKER_NAME_PREFIX}-flag-logs-queue-${queue_index}"
    else
        SHARD_NAME="flag-logs-queue-${queue_index}"
    fi
    cat >> wrangler.toml <<EOF

[[queues.consumers]]
queue = "${SHARD_NAME}"
max_batch_size = 100
max_batch_timeout = 10

[[queues.producers]]
queue = "${SHARD_NAME}"
binding = "flag_logs_queue_${queue_index}"
EOF
done

if [ "$FLAG_LOGS_QUEUE_COUNT" -gt 1 ]; then
    echo "✅ Created ${FLAG_LOGS_QUEUE_COUNT} flag-log queue shards"
fi

# Create events queue
if [ -n "$WORKER_NAME_PREFIX" ]; then
    EVENTS_QUEUE_NAME="${WORKER_NAME_PREFIX}-events-queue"
else
    EVENTS_QUEUE_NAME="events-queue"
fi
ensure_queue "$EVENTS_QUEUE_NAME" || exit 1


ensure_r2_bucket() {
    local B_NAME="$1"
    echo "🔍 Checking if R2 bucket '$B_NAME' exists..."
    local B_CHECK B_STATUS
    B_CHECK=$(curl -sS -w "%{http_code}" \
        -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
        "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/r2/buckets/${B_NAME}")
    B_STATUS="${B_CHECK: -3}"

    if [ "$B_STATUS" = "200" ]; then
        echo "✅ R2 bucket '$B_NAME' already exists"
        return 0
    fi

    echo "📦 R2 bucket '$B_NAME' not found, creating..."
    local B_CREATE B_CREATE_STATUS
    B_CREATE=$(curl -sS -w "%{http_code}" -X POST \
        -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "{\"name\": \"${B_NAME}\"}" \
        "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/r2/buckets")
    B_CREATE_STATUS="${B_CREATE: -3}"
    if [ "$B_CREATE_STATUS" = "200" ] || [ "$B_CREATE_STATUS" = "201" ]; then
        echo "✅ R2 bucket '$B_NAME' created successfully"
        return 0
    fi
    echo "❌ Failed to create R2 bucket '$B_NAME' (HTTP $B_CREATE_STATUS)"
    echo "${B_CREATE%???}"
    return 1
}

# Creates or updates the workers_trace_events job that feeds the bucket.
#
# The ScriptName filter is what keeps the job scoped to this worker — without
# it the job captures every Worker in the account. The EventType filter drops
# the aggregator's own cron invocations, so its console output can never feed
# back into the bucket it is draining.
#
# field_names is restricted to the two fields the aggregator reads. Event
# carries full request metadata and is most of the volume; Exceptions would
# add panic text the aggregator ignores anyway.
#
# max_upload_bytes pins the uncompressed object size at 20 MB, comfortably
# under the aggregator's 48 MB per-object ceiling. Without it the size is
# whatever Cloudflare defaults to for the destination, which would leave the
# aggregator's memory bound resting on an unknown.
ensure_logpush_job() {
    local JOB_NAME="$1" BUCKET="$2"

    local DEST_CONF JOB_BODY
    DEST_CONF=$(jq -rn \
        --arg bucket "$BUCKET" \
        --arg account "$CLOUDFLARE_ACCOUNT_ID" \
        --arg key "$R2_ACCESS_KEY_ID" \
        --arg secret "$R2_SECRET_ACCESS_KEY" \
        '"r2://\($bucket)/flag-logs/{DATE}?account-id=\($account|@uri)&access-key-id=\($key|@uri)&secret-access-key=\($secret|@uri)"')

    JOB_BODY=$(jq -n \
        --arg name "$JOB_NAME" \
        --arg dest "$DEST_CONF" \
        --arg script "$WORKER_NAME" \
        '{
            name: $name,
            dataset: "workers_trace_events",
            destination_conf: $dest,
            enabled: true,
            output_options: {
                field_names: ["EventTimestampMs", "Logs"],
                timestamp_format: "rfc3339"
            },
            max_upload_bytes: 20000000,
            max_upload_records: 50000,
            filter: ({where: {and: [
                {key: "ScriptName", operator: "eq", value: $script},
                {key: "EventType", operator: "eq", value: "fetch"}
            ]}} | tostring)
        }')

    echo "🔍 Checking for existing Logpush job '$JOB_NAME'..."
    local JOB_LIST JOB_LIST_STATUS JOB_ID=""
    JOB_LIST=$(curl -sS -w "%{http_code}" \
        -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
        "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/logpush/jobs")
    JOB_LIST_STATUS="${JOB_LIST: -3}"
    if [ "$JOB_LIST_STATUS" = "200" ]; then
        JOB_ID=$(printf "%s" "${JOB_LIST%???}" \
            | jq -r ".result[]? | select(.name == \"${JOB_NAME}\") | .id" 2>/dev/null || true)
    else
        echo "⚠️ Could not list Logpush jobs (HTTP $JOB_LIST_STATUS)"
    fi

    local RESP RESP_STATUS
    if [ -n "$JOB_ID" ]; then
        # Re-PUT so a rotated R2 credential or an edited filter takes effect.
        # name and dataset are immutable, so they are dropped from the body.
        echo "♻️ Updating Logpush job '$JOB_NAME' (id: $JOB_ID)..."
        RESP=$(printf "%s" "$JOB_BODY" | jq 'del(.name, .dataset)' | curl -sS -w "%{http_code}" -X PUT \
            -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
            -H "Content-Type: application/json" \
            --data-binary @- \
            "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/logpush/jobs/${JOB_ID}")
    else
        echo "📦 Creating Logpush job '$JOB_NAME'..."
        RESP=$(printf "%s" "$JOB_BODY" | curl -sS -w "%{http_code}" -X POST \
            -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
            -H "Content-Type: application/json" \
            --data-binary @- \
            "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/logpush/jobs")
    fi

    RESP_STATUS="${RESP: -3}"
    if [ "$RESP_STATUS" = "200" ] || [ "$RESP_STATUS" = "201" ]; then
        echo "✅ Logpush job '$JOB_NAME' is configured"
        return 0
    fi
    # The response body can echo the destination, which carries the R2
    # secret, so only the error messages are printed.
    echo "❌ Failed to configure Logpush job '$JOB_NAME' (HTTP $RESP_STATUS)"
    printf "%s" "${RESP%???}" | jq -r '.errors[]? | "   \(.code): \(.message)"' 2>/dev/null || true
    return 1
}

if [ "$FLAG_LOG_SINK" = "logpush" ]; then
    if [ -z "${R2_ACCESS_KEY_ID:-}" ] || [ -z "${R2_SECRET_ACCESS_KEY:-}" ]; then
        echo "❌ FLAG_LOG_SINK=logpush requires R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY" >&2
        echo "   Logpush authenticates to R2 with an S3-style key pair; create an R2" >&2
        echo "   API token with object read+write and pass it via these variables." >&2
        exit 1
    fi

    if [ -n "$WORKER_NAME_PREFIX" ]; then
        FLAG_LOGS_BUCKET="${WORKER_NAME_PREFIX}-flag-logs"
    else
        FLAG_LOGS_BUCKET="flag-logs"
    fi

    ensure_r2_bucket "$FLAG_LOGS_BUCKET" || exit 1
    ensure_logpush_job "${WORKER_NAME}-flag-logs" "$FLAG_LOGS_BUCKET" || exit 1

    # logpush = true is a top-level script setting, so it has to be prepended:
    # appending it would land inside whichever table comes last.
    sed -i.tmp '/^logpush *= *.*$/d' wrangler.toml || true
    LOGPUSH_TMPFILE=$(mktemp)
    printf 'logpush = true\n' > "$LOGPUSH_TMPFILE"
    cat wrangler.toml >> "$LOGPUSH_TMPFILE"
    mv "$LOGPUSH_TMPFILE" wrangler.toml

    cat >> wrangler.toml <<EOF

# Bucket Logpush writes trace events into, drained by the cron trigger below.
[[r2_buckets]]
binding = "FLAG_LOGS_R2"
bucket_name = "${FLAG_LOGS_BUCKET}"

[triggers]
crons = ["${FLAG_LOGS_AGGREGATOR_CRON}"]

# The aggregation pass decompresses and parses many R2 objects in one
# invocation, which needs more than the 30s default. This raises the ceiling;
# it does not reserve time, so resolve requests are unaffected.
[limits]
cpu_ms = 120000
EOF
    echo "✅ Added FLAG_LOGS_R2 binding and aggregator cron (${FLAG_LOGS_AGGREGATOR_CRON}) to wrangler.toml"
    echo "✅ Flag-log sink: logpush (R2 bucket '${FLAG_LOGS_BUCKET}')"
else
    echo "✅ Flag-log sink: queue"
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

# Create KV namespace for sticky assignments if enabled
if [ -n "$ENABLE_STICKY_ASSIGNMENTS" ]; then
    if [ -n "$WORKER_NAME_PREFIX" ]; then
        MAT_KV_TITLE="${WORKER_NAME_PREFIX}-resolver-materializations"
    else
        MAT_KV_TITLE="resolver-materializations"
    fi

    echo "🔍 Checking if KV namespace '$MAT_KV_TITLE' exists..."
    MAT_KV_NAMESPACE_ID=""
    MAT_KV_PAGE=1
    while true; do
        MAT_KV_LIST=$(curl -sS -w "%{http_code}" \
            -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
            "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/storage/kv/namespaces?per_page=100&page=${MAT_KV_PAGE}")
        MAT_KV_LIST_STATUS="${MAT_KV_LIST: -3}"
        MAT_KV_LIST_BODY="${MAT_KV_LIST%???}"

        if [ "$MAT_KV_LIST_STATUS" != "200" ]; then
            break
        fi

        MAT_KV_NAMESPACE_ID=$(printf "%s" "$MAT_KV_LIST_BODY" | jq -r ".result[] | select(.title == \"${MAT_KV_TITLE}\") | .id" 2>/dev/null || true)
        if [ -n "$MAT_KV_NAMESPACE_ID" ]; then
            break
        fi

        MAT_KV_COUNT=$(printf "%s" "$MAT_KV_LIST_BODY" | jq -r '.result | length' 2>/dev/null || echo "0")
        if [ "$MAT_KV_COUNT" -lt 100 ]; then
            break
        fi
        MAT_KV_PAGE=$((MAT_KV_PAGE + 1))
    done

    if [ -z "$MAT_KV_NAMESPACE_ID" ]; then
        echo "📦 KV namespace '$MAT_KV_TITLE' not found, creating..."
        MAT_KV_CREATE_RESP=$(curl -sS -w "%{http_code}" -X POST \
            -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
            -H "Content-Type: application/json" \
            -d "{\"title\": \"${MAT_KV_TITLE}\"}" \
            "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/storage/kv/namespaces")
        MAT_KV_CREATE_STATUS="${MAT_KV_CREATE_RESP: -3}"
        MAT_KV_CREATE_BODY="${MAT_KV_CREATE_RESP%???}"
        if [ "$MAT_KV_CREATE_STATUS" = "200" ] || [ "$MAT_KV_CREATE_STATUS" = "201" ]; then
            MAT_KV_NAMESPACE_ID=$(printf "%s" "$MAT_KV_CREATE_BODY" | jq -r '.result.id')
            echo "✅ KV namespace '$MAT_KV_TITLE' created (id: $MAT_KV_NAMESPACE_ID)"
        else
            echo "❌ Failed to create KV namespace for materializations (HTTP $MAT_KV_CREATE_STATUS)"
            exit 1
        fi
    else
        echo "✅ KV namespace '$MAT_KV_TITLE' already exists (id: $MAT_KV_NAMESPACE_ID)"
    fi

    cat >> wrangler.toml <<EOF

[[kv_namespaces]]
binding = "CONFIDENCE_MATERIALIZATIONS_KV"
id = "$MAT_KV_NAMESPACE_ID"
EOF
    echo "✅ Added CONFIDENCE_MATERIALIZATIONS_KV binding to wrangler.toml"
else
    echo "ℹ️ Sticky assignments not enabled (set ENABLE_STICKY_ASSIGNMENTS to enable)"
fi

# Update worker name and queue names in wrangler.toml if using prefix
if [ -n "$WORKER_NAME_PREFIX" ]; then
    sed -i.tmp "s/^name = .*/name = \"$WORKER_NAME\"/" wrangler.toml
    sed -i.tmp "s/queue = \"flag-logs-queue\"/queue = \"$BASE_QUEUE_NAME\"/g" wrangler.toml
    sed -i.tmp "s/queue = \"events-queue\"/queue = \"$EVENTS_QUEUE_NAME\"/g" wrangler.toml
    echo "✅ Updated worker name to \"$WORKER_NAME\" in wrangler.toml"
    echo "✅ Updated queue names in wrangler.toml"
fi

# Prepare ALLOWED_ORIGIN for TOML (escape quotes and backslashes)
if [ -n "$CONFIDENCE_RESOLVER_ALLOWED_ORIGIN" ]; then
    ALLOWED_ORIGIN_TOML=$(printf '%s' "$CONFIDENCE_RESOLVER_ALLOWED_ORIGIN" | sed 's/\\/\\\\/g; s/\"/\\\"/g')
fi

# Prepare CONFIDENCE_CLIENT_SECRET for TOML (escape quotes and backslashes)
if [ -n "$CONFIDENCE_CLIENT_SECRET" ]; then
    CLIENT_SECRET_TOML=$(printf '%s' "$CONFIDENCE_CLIENT_SECRET" | sed 's/\\/\\\\/g; s/\"/\\\"/g')
fi

# Validate FORCE_APPLY if provided (worker defaults to true when unset)
if [ -n "$FORCE_APPLY" ]; then
    FORCE_APPLY=$(printf '%s' "$FORCE_APPLY" | tr '[:upper:]' '[:lower:]')
    if [ "$FORCE_APPLY" != "true" ] && [ "$FORCE_APPLY" != "false" ]; then
        echo "❌ FORCE_APPLY must be \"true\" or \"false\", got: $FORCE_APPLY" >&2
        exit 1
    fi
fi

# Validate ENABLE_APPLY_DEDUP if provided (worker defaults to false when unset)
if [ -n "$ENABLE_APPLY_DEDUP" ]; then
    ENABLE_APPLY_DEDUP=$(printf '%s' "$ENABLE_APPLY_DEDUP" | tr '[:upper:]' '[:lower:]')
    if [ "$ENABLE_APPLY_DEDUP" != "true" ] && [ "$ENABLE_APPLY_DEDUP" != "false" ]; then
        echo "❌ ENABLE_APPLY_DEDUP must be \"true\" or \"false\", got: $ENABLE_APPLY_DEDUP" >&2
        exit 1
    fi
fi

# Update [vars] without duplicating the table.
if [ -n "$ALLOWED_ORIGIN_TOML" ] || [ -n "$ETAG_TOML" ] || [ -n "$DEPLOYER_VERSION" ] || [ -n "$CLIENT_SECRET_TOML" ] || [ -n "$FORCE_APPLY" ] || [ -n "$ENABLE_APPLY_DEDUP" ] || [ -n "$FLAG_LOG_SINK" ]; then
    # Remove any existing definitions to avoid duplicates
    sed -i.tmp '/^ALLOWED_ORIGIN *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^RESOLVER_STATE_ETAG *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^RESOLVER_VERSION *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^DEPLOYER_VERSION *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^CONFIDENCE_CLIENT_SECRET *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^FORCE_APPLY *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^ENABLE_APPLY_DEDUP *= *.*$/d' wrangler.toml || true
    sed -i.tmp '/^FLAG_LOG_SINK *= *.*$/d' wrangler.toml || true
    awk -v allowed="${ALLOWED_ORIGIN_TOML}" -v etag="${ETAG_TOML}" -v version="${DEPLOYER_VERSION}" -v client_secret="${CLIENT_SECRET_TOML}" -v force_apply="${FORCE_APPLY}" -v enable_apply_dedup="${ENABLE_APPLY_DEDUP}" -v flag_log_sink="${FLAG_LOG_SINK}" '
        BEGIN{inserted=0}
        {
            print $0
            if (!inserted && $0 ~ /^\[vars\]/) {
                if (allowed != "") print "ALLOWED_ORIGIN = \"" allowed "\""
                if (etag != "") print "RESOLVER_STATE_ETAG = \"" etag "\""
                if (version != "") print "DEPLOYER_VERSION = \"" version "\""
                if (client_secret != "") print "CONFIDENCE_CLIENT_SECRET = \"" client_secret "\""
                if (force_apply != "") print "FORCE_APPLY = \"" force_apply "\""
                if (enable_apply_dedup != "") print "ENABLE_APPLY_DEDUP = \"" enable_apply_dedup "\""
                if (flag_log_sink != "") print "FLAG_LOG_SINK = \"" flag_log_sink "\""
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
    if [ -n "$FORCE_APPLY" ]; then
        echo "✅ FORCE_APPLY set to \"$FORCE_APPLY\" in wrangler.toml"
    fi
    if [ -n "$FLAG_LOG_SINK" ]; then
        echo "✅ FLAG_LOG_SINK set to \"$FLAG_LOG_SINK\" in wrangler.toml"
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

     # Store encryption key as a Cloudflare Worker secret (persists across deploys)
     if [ -n "$SET_SECRET_AFTER_DEPLOY" ] && [ -n "$RESOLVE_TOKEN_ENCRYPTION_KEY" ]; then
         echo "🔐 Storing RESOLVE_TOKEN_ENCRYPTION_KEY as worker secret..."
         SECRET_BODY=$(jq -n --arg text "$RESOLVE_TOKEN_ENCRYPTION_KEY" \
             '{"name": "RESOLVE_TOKEN_ENCRYPTION_KEY", "text": $text, "type": "secret_text"}')
         SECRET_PUT_RESP=$(curl -sS -w "%{http_code}" -X PUT \
             -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
             -H "Content-Type: application/json" \
             -d "$SECRET_BODY" \
             "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/workers/scripts/${WORKER_NAME}/secrets")
         SECRET_PUT_STATUS="${SECRET_PUT_RESP: -3}"
         if [ "$SECRET_PUT_STATUS" = "200" ] || [ "$SECRET_PUT_STATUS" = "201" ]; then
             echo "✅ RESOLVE_TOKEN_ENCRYPTION_KEY stored as worker secret"
         else
             echo "⚠️ Could not store encryption key as worker secret (HTTP $SECRET_PUT_STATUS)"
             echo "   The key is embedded in the worker binary for this deployment."
             echo "   To persist across deploys, set RESOLVE_TOKEN_ENCRYPTION_KEY explicitly."
         fi
     fi
else
     echo "NO_DEPLOY is set, skipping deploy"
fi
