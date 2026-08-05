#!/usr/bin/env bash
# Deployer script for the Confidence Lambda Resolver.
# Adapted from confidence-cloudflare-resolver/deployer/script.sh.
#
# This script:
# 1. Validates credentials (AWS + Confidence)
# 2. Fetches resolver state from CDN (conditional on ETag)
# 3. Creates SQS queue and DynamoDB tables as needed
# 4. Builds the resolver and consumer Lambda binaries
# 5. Packages and deploys to AWS Lambda
set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
: "${AWS_REGION:?AWS_REGION is required}"
: "${CONFIDENCE_CLIENT_SECRET:?CONFIDENCE_CLIENT_SECRET is required}"

FUNCTION_NAME_PREFIX="${FUNCTION_NAME_PREFIX:-}"
LAMBDA_FUNCTION_NAME="${LAMBDA_FUNCTION_NAME:-confidence-lambda-resolver}"
CONSUMER_FUNCTION_NAME="${CONSUMER_FUNCTION_NAME:-confidence-lambda-consumer}"
LAMBDA_MEMORY_MB="${LAMBDA_MEMORY_MB:-128}"
LAMBDA_TIMEOUT="${LAMBDA_TIMEOUT:-10}"
LAMBDA_ROLE_ARN="${LAMBDA_ROLE_ARN:-}"
CONFIDENCE_RESOLVER_STATE_URL="${CONFIDENCE_RESOLVER_STATE_URL:-}"
RESOLVE_TOKEN_ENCRYPTION_KEY="${RESOLVE_TOKEN_ENCRYPTION_KEY:-}"
ALLOWED_ORIGIN="${ALLOWED_ORIGIN:-*}"
ENABLE_METRICS="${ENABLE_METRICS:-}"
ENABLE_STICKY_ASSIGNMENTS_DYNAMODB="${ENABLE_STICKY_ASSIGNMENTS_DYNAMODB:-}"
FORCE_DEPLOY="${FORCE_DEPLOY:-}"
NO_DEPLOY="${NO_DEPLOY:-}"

if [ -n "$FUNCTION_NAME_PREFIX" ]; then
    LAMBDA_FUNCTION_NAME="${FUNCTION_NAME_PREFIX}-${LAMBDA_FUNCTION_NAME}"
    CONSUMER_FUNCTION_NAME="${FUNCTION_NAME_PREFIX}-${CONSUMER_FUNCTION_NAME}"
fi

SQS_QUEUE_NAME="${LAMBDA_FUNCTION_NAME}-flag-logs"
METRICS_TABLE_NAME="${LAMBDA_FUNCTION_NAME}-metrics"
MATERIALIZATIONS_TABLE_NAME="${LAMBDA_FUNCTION_NAME}-materializations"

# ---------------------------------------------------------------------------
# Helper functions
# ---------------------------------------------------------------------------
log() { echo "[deployer] $*"; }
die() { echo "[deployer] ERROR: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1. Validate AWS credentials
# ---------------------------------------------------------------------------
log "Validating AWS credentials..."
aws sts get-caller-identity --region "$AWS_REGION" > /dev/null || die "AWS credentials invalid"
log "AWS credentials OK (region: $AWS_REGION)"

# ---------------------------------------------------------------------------
# 2. Compute CDN URL from client secret hash
# ---------------------------------------------------------------------------
if [ -z "$CONFIDENCE_RESOLVER_STATE_URL" ]; then
    SECRET_HASH=$(echo -n "$CONFIDENCE_CLIENT_SECRET" | sha256sum | awk '{print $1}')
    CONFIDENCE_RESOLVER_STATE_URL="https://confidence-resolver-state-cdn.spotifycdn.com/${SECRET_HASH}"
fi
log "State URL: $CONFIDENCE_RESOLVER_STATE_URL"

# ---------------------------------------------------------------------------
# 3. Determine deployer version
# ---------------------------------------------------------------------------
if [ -n "${COMMIT_SHA:-}" ]; then
    DEPLOYER_VERSION="$COMMIT_SHA"
elif command -v git &>/dev/null && git describe --tags 2>/dev/null; then
    DEPLOYER_VERSION=$(git describe --tags 2>/dev/null || echo "unknown")
elif [ -f /workspace/.release_tag ]; then
    DEPLOYER_VERSION=$(cat /workspace/.release_tag)
else
    DEPLOYER_VERSION="unknown"
fi
log "Deployer version: $DEPLOYER_VERSION"

# ---------------------------------------------------------------------------
# 4. Check deployed ETag via Function URL
# ---------------------------------------------------------------------------
PREVIOUS_ETAG=""
PREVIOUS_VERSION=""

FUNCTION_URL=$(aws lambda get-function-url-config \
    --function-name "$LAMBDA_FUNCTION_NAME" \
    --region "$AWS_REGION" \
    --query 'FunctionUrl' --output text 2>/dev/null || echo "")

if [ -n "$FUNCTION_URL" ] && [ "$FUNCTION_URL" != "None" ]; then
    log "Checking deployed state at $FUNCTION_URL"
    STATE_RESPONSE=$(curl -sf "${FUNCTION_URL}v1/state:etag" 2>/dev/null || echo "{}")
    PREVIOUS_ETAG=$(echo "$STATE_RESPONSE" | jq -r '.etag // ""')
    PREVIOUS_VERSION=$(echo "$STATE_RESPONSE" | jq -r '.version // ""')
    log "Deployed ETag: ${PREVIOUS_ETAG:-none}, Version: ${PREVIOUS_VERSION:-none}"
fi

if [ -n "$PREVIOUS_VERSION" ] && [ "$PREVIOUS_VERSION" != "$DEPLOYER_VERSION" ]; then
    log "Version changed ($PREVIOUS_VERSION -> $DEPLOYER_VERSION), forcing deploy"
    FORCE_DEPLOY=1
fi

# ---------------------------------------------------------------------------
# 5. Fetch resolver state from CDN
# ---------------------------------------------------------------------------
ETAG_HEADER=""
if [ -n "$PREVIOUS_ETAG" ] && [ -z "$FORCE_DEPLOY" ]; then
    ETAG_HEADER="-H \"If-None-Match: $PREVIOUS_ETAG\""
fi

RESPONSE_FILE=$(mktemp)
HTTP_CODE=$(eval curl -sf -w '%{http_code}' -o "$RESPONSE_FILE" "$ETAG_HEADER" "$CONFIDENCE_RESOLVER_STATE_URL" || echo "000")

if [ "$HTTP_CODE" = "304" ]; then
    log "State unchanged (304 Not Modified). Nothing to deploy."
    rm -f "$RESPONSE_FILE"
    exit 0
elif [ "$HTTP_CODE" != "200" ]; then
    die "Failed to fetch state from CDN: HTTP $HTTP_CODE"
fi

# Extract ETag from response headers
NEW_ETAG=$(curl -sI "$CONFIDENCE_RESOLVER_STATE_URL" 2>/dev/null | grep -i 'etag:' | tr -d '\r' | sed 's/etag: //i' | sed 's/"//g' || echo "")
log "Downloaded state (ETag: ${NEW_ETAG:-unknown})"

# ---------------------------------------------------------------------------
# 6. Write data files
# ---------------------------------------------------------------------------
WORKSPACE_ROOT="${WORKSPACE_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
DATA_DIR="$WORKSPACE_ROOT/data"
mkdir -p "$DATA_DIR"

cp "$RESPONSE_FILE" "$DATA_DIR/resolver_state_current.pb"
rm -f "$RESPONSE_FILE"

echo -n "${RESOLVE_TOKEN_ENCRYPTION_KEY:-}" > "$DATA_DIR/encryption_key"
log "Data files written to $DATA_DIR"

# ---------------------------------------------------------------------------
# 7. Create SQS queue if not exists
# ---------------------------------------------------------------------------
log "Ensuring SQS queue: $SQS_QUEUE_NAME"
SQS_QUEUE_URL=$(aws sqs get-queue-url \
    --queue-name "$SQS_QUEUE_NAME" \
    --region "$AWS_REGION" \
    --query 'QueueUrl' --output text 2>/dev/null || echo "")

if [ -z "$SQS_QUEUE_URL" ] || [ "$SQS_QUEUE_URL" = "None" ]; then
    SQS_QUEUE_URL=$(aws sqs create-queue \
        --queue-name "$SQS_QUEUE_NAME" \
        --region "$AWS_REGION" \
        --query 'QueueUrl' --output text)
    log "Created SQS queue: $SQS_QUEUE_URL"
else
    log "SQS queue exists: $SQS_QUEUE_URL"
fi

SQS_QUEUE_ARN=$(aws sqs get-queue-attributes \
    --queue-url "$SQS_QUEUE_URL" \
    --attribute-names QueueArn \
    --region "$AWS_REGION" \
    --query 'Attributes.QueueArn' --output text)

# ---------------------------------------------------------------------------
# 8. Create DynamoDB tables if needed
# ---------------------------------------------------------------------------
METRICS_TABLE_ENV=""
MATERIALIZATIONS_TABLE_ENV=""

if [ -n "$ENABLE_METRICS" ]; then
    log "Ensuring DynamoDB metrics table: $METRICS_TABLE_NAME"
    if ! aws dynamodb describe-table --table-name "$METRICS_TABLE_NAME" --region "$AWS_REGION" &>/dev/null; then
        aws dynamodb create-table \
            --table-name "$METRICS_TABLE_NAME" \
            --attribute-definitions AttributeName=pk,AttributeType=S \
            --key-schema AttributeName=pk,KeyType=HASH \
            --billing-mode PAY_PER_REQUEST \
            --region "$AWS_REGION" > /dev/null
        aws dynamodb wait table-exists --table-name "$METRICS_TABLE_NAME" --region "$AWS_REGION"
        log "Created DynamoDB metrics table"
    fi
    METRICS_TABLE_ENV="$METRICS_TABLE_NAME"
fi

if [ -n "$ENABLE_STICKY_ASSIGNMENTS_DYNAMODB" ]; then
    log "Ensuring DynamoDB materializations table: $MATERIALIZATIONS_TABLE_NAME"
    if ! aws dynamodb describe-table --table-name "$MATERIALIZATIONS_TABLE_NAME" --region "$AWS_REGION" &>/dev/null; then
        aws dynamodb create-table \
            --table-name "$MATERIALIZATIONS_TABLE_NAME" \
            --attribute-definitions \
                AttributeName=pk,AttributeType=S \
                AttributeName=sk,AttributeType=S \
            --key-schema \
                AttributeName=pk,KeyType=HASH \
                AttributeName=sk,KeyType=RANGE \
            --billing-mode PAY_PER_REQUEST \
            --region "$AWS_REGION" > /dev/null
        aws dynamodb wait table-exists --table-name "$MATERIALIZATIONS_TABLE_NAME" --region "$AWS_REGION"
        log "Created DynamoDB materializations table"
    fi
    MATERIALIZATIONS_TABLE_ENV="$MATERIALIZATIONS_TABLE_NAME"
fi

# ---------------------------------------------------------------------------
# 9. Build
# ---------------------------------------------------------------------------
log "Building Lambda binaries..."
cd "$WORKSPACE_ROOT"
cargo build --release --target aarch64-unknown-linux-gnu -p confidence-lambda-resolver
log "Build complete"

# ---------------------------------------------------------------------------
# 10. Package
# ---------------------------------------------------------------------------
TARGET_DIR="$WORKSPACE_ROOT/target/aarch64-unknown-linux-gnu/release"
DEPLOY_DIR=$(mktemp -d)

# Package resolver
cp "$TARGET_DIR/resolver" "$DEPLOY_DIR/bootstrap"
(cd "$DEPLOY_DIR" && zip -j resolver.zip bootstrap)
rm "$DEPLOY_DIR/bootstrap"

# Package consumer
cp "$TARGET_DIR/consumer" "$DEPLOY_DIR/bootstrap"
(cd "$DEPLOY_DIR" && zip -j consumer.zip bootstrap)
rm "$DEPLOY_DIR/bootstrap"

log "Packaged: $DEPLOY_DIR/resolver.zip, $DEPLOY_DIR/consumer.zip"

# ---------------------------------------------------------------------------
# 11. Deploy (unless NO_DEPLOY is set)
# ---------------------------------------------------------------------------
if [ -n "$NO_DEPLOY" ]; then
    log "NO_DEPLOY is set. Skipping deployment."
    exit 0
fi

ENV_VARS="{\"Variables\":{\"CONFIDENCE_CLIENT_SECRET\":\"$CONFIDENCE_CLIENT_SECRET\",\"ALLOWED_ORIGIN\":\"$ALLOWED_ORIGIN\",\"RESOLVER_STATE_ETAG\":\"${NEW_ETAG:-}\",\"DEPLOYER_VERSION\":\"$DEPLOYER_VERSION\",\"SQS_QUEUE_URL\":\"$SQS_QUEUE_URL\""
[ -n "$METRICS_TABLE_ENV" ] && ENV_VARS="$ENV_VARS,\"DYNAMODB_METRICS_TABLE\":\"$METRICS_TABLE_ENV\""
[ -n "$MATERIALIZATIONS_TABLE_ENV" ] && ENV_VARS="$ENV_VARS,\"DYNAMODB_MATERIALIZATIONS_TABLE\":\"$MATERIALIZATIONS_TABLE_ENV\""
ENV_VARS="$ENV_VARS}}"

# Deploy resolver Lambda
log "Deploying resolver Lambda: $LAMBDA_FUNCTION_NAME"
if aws lambda get-function --function-name "$LAMBDA_FUNCTION_NAME" --region "$AWS_REGION" &>/dev/null; then
    aws lambda update-function-code \
        --function-name "$LAMBDA_FUNCTION_NAME" \
        --zip-file "fileb://$DEPLOY_DIR/resolver.zip" \
        --architectures arm64 \
        --region "$AWS_REGION" > /dev/null
    aws lambda wait function-updated --function-name "$LAMBDA_FUNCTION_NAME" --region "$AWS_REGION"
    aws lambda update-function-configuration \
        --function-name "$LAMBDA_FUNCTION_NAME" \
        --environment "$ENV_VARS" \
        --memory-size "$LAMBDA_MEMORY_MB" \
        --timeout "$LAMBDA_TIMEOUT" \
        --region "$AWS_REGION" > /dev/null
else
    [ -z "$LAMBDA_ROLE_ARN" ] && die "LAMBDA_ROLE_ARN required for first deploy"
    aws lambda create-function \
        --function-name "$LAMBDA_FUNCTION_NAME" \
        --runtime provided.al2023 \
        --handler bootstrap \
        --architectures arm64 \
        --role "$LAMBDA_ROLE_ARN" \
        --zip-file "fileb://$DEPLOY_DIR/resolver.zip" \
        --environment "$ENV_VARS" \
        --memory-size "$LAMBDA_MEMORY_MB" \
        --timeout "$LAMBDA_TIMEOUT" \
        --region "$AWS_REGION" > /dev/null
    aws lambda wait function-active --function-name "$LAMBDA_FUNCTION_NAME" --region "$AWS_REGION"
fi
log "Resolver Lambda deployed"

# Ensure Function URL
if ! aws lambda get-function-url-config --function-name "$LAMBDA_FUNCTION_NAME" --region "$AWS_REGION" &>/dev/null; then
    aws lambda create-function-url-config \
        --function-name "$LAMBDA_FUNCTION_NAME" \
        --auth-type NONE \
        --cors '{"AllowOrigins":["*"],"AllowMethods":["POST","GET","OPTIONS"],"AllowHeaders":["*"]}' \
        --region "$AWS_REGION" > /dev/null

    aws lambda add-permission \
        --function-name "$LAMBDA_FUNCTION_NAME" \
        --statement-id FunctionURLAllowPublicAccess \
        --action lambda:InvokeFunctionUrl \
        --principal "*" \
        --function-url-auth-type NONE \
        --region "$AWS_REGION" > /dev/null
    log "Function URL created"
fi

# Deploy consumer Lambda
CONSUMER_ENV="{\"Variables\":{\"CONFIDENCE_CLIENT_SECRET\":\"$CONFIDENCE_CLIENT_SECRET\""
[ -n "$METRICS_TABLE_ENV" ] && CONSUMER_ENV="$CONSUMER_ENV,\"DYNAMODB_METRICS_TABLE\":\"$METRICS_TABLE_ENV\""
CONSUMER_ENV="$CONSUMER_ENV}}"

log "Deploying consumer Lambda: $CONSUMER_FUNCTION_NAME"
if aws lambda get-function --function-name "$CONSUMER_FUNCTION_NAME" --region "$AWS_REGION" &>/dev/null; then
    aws lambda update-function-code \
        --function-name "$CONSUMER_FUNCTION_NAME" \
        --zip-file "fileb://$DEPLOY_DIR/consumer.zip" \
        --architectures arm64 \
        --region "$AWS_REGION" > /dev/null
    aws lambda wait function-updated --function-name "$CONSUMER_FUNCTION_NAME" --region "$AWS_REGION"
    aws lambda update-function-configuration \
        --function-name "$CONSUMER_FUNCTION_NAME" \
        --environment "$CONSUMER_ENV" \
        --memory-size "$LAMBDA_MEMORY_MB" \
        --timeout 30 \
        --region "$AWS_REGION" > /dev/null
else
    [ -z "$LAMBDA_ROLE_ARN" ] && die "LAMBDA_ROLE_ARN required for first deploy"
    aws lambda create-function \
        --function-name "$CONSUMER_FUNCTION_NAME" \
        --runtime provided.al2023 \
        --handler bootstrap \
        --architectures arm64 \
        --role "$LAMBDA_ROLE_ARN" \
        --zip-file "fileb://$DEPLOY_DIR/consumer.zip" \
        --environment "$CONSUMER_ENV" \
        --memory-size "$LAMBDA_MEMORY_MB" \
        --timeout 30 \
        --region "$AWS_REGION" > /dev/null
    aws lambda wait function-active --function-name "$CONSUMER_FUNCTION_NAME" --region "$AWS_REGION"
fi
log "Consumer Lambda deployed"

# Ensure SQS event source mapping
EXISTING_MAPPING=$(aws lambda list-event-source-mappings \
    --function-name "$CONSUMER_FUNCTION_NAME" \
    --event-source-arn "$SQS_QUEUE_ARN" \
    --region "$AWS_REGION" \
    --query 'EventSourceMappings[0].UUID' --output text 2>/dev/null || echo "None")

if [ "$EXISTING_MAPPING" = "None" ] || [ -z "$EXISTING_MAPPING" ]; then
    aws lambda create-event-source-mapping \
        --function-name "$CONSUMER_FUNCTION_NAME" \
        --event-source-arn "$SQS_QUEUE_ARN" \
        --batch-size 100 \
        --maximum-batching-window-in-seconds 10 \
        --region "$AWS_REGION" > /dev/null
    log "SQS event source mapping created"
fi

# Cleanup
rm -rf "$DEPLOY_DIR"

FINAL_URL=$(aws lambda get-function-url-config \
    --function-name "$LAMBDA_FUNCTION_NAME" \
    --region "$AWS_REGION" \
    --query 'FunctionUrl' --output text 2>/dev/null || echo "unknown")

log "Deployment complete!"
log "Function URL: $FINAL_URL"
log "ETag: ${NEW_ETAG:-unknown}"
log "Version: $DEPLOYER_VERSION"
