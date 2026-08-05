# Confidence Lambda Resolver Deployer

Docker-based deployer for the Confidence Lambda Resolver. Mirrors the Cloudflare deployer pattern: fetches resolver state from CDN, cross-compiles the Rust binary, and deploys to AWS Lambda.

## Usage

```bash
docker run --rm \
  -e AWS_ACCESS_KEY_ID="..." \
  -e AWS_SECRET_ACCESS_KEY="..." \
  -e AWS_REGION="us-east-1" \
  -e CONFIDENCE_CLIENT_SECRET="..." \
  confidence-lambda-deployer:latest
```

## Required Environment Variables

| Variable | Description |
|----------|-------------|
| `AWS_REGION` | AWS region for Lambda deployment |
| `CONFIDENCE_CLIENT_SECRET` | Confidence backend client secret (type BACKEND). Also used to derive the CDN state URL via SHA256. |

## Optional Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `AWS_ACCESS_KEY_ID` | (from IAM role) | AWS credential |
| `AWS_SECRET_ACCESS_KEY` | (from IAM role) | AWS credential |
| `LAMBDA_FUNCTION_NAME` | `confidence-lambda-resolver` | Name for the resolver Lambda function |
| `CONSUMER_FUNCTION_NAME` | `confidence-lambda-consumer` | Name for the consumer Lambda function |
| `FUNCTION_NAME_PREFIX` | | Prefix for all resource names (e.g., `staging`) |
| `LAMBDA_MEMORY_MB` | `128` | Memory allocation for both Lambda functions |
| `LAMBDA_TIMEOUT` | `10` | Timeout in seconds for the resolver Lambda |
| `LAMBDA_ROLE_ARN` | | IAM role ARN for the Lambda functions (required for first deploy) |
| `CONFIDENCE_RESOLVER_STATE_URL` | (derived from client secret SHA256) | Override CDN URL for state download |
| `RESOLVE_TOKEN_ENCRYPTION_KEY` | | AES-128 base64-encoded key for resolve token encryption |
| `ALLOWED_ORIGIN` | `*` | CORS `Access-Control-Allow-Origin` value |
| `ENABLE_METRICS` | | Set to enable DynamoDB-backed Prometheus metrics and `/metrics` endpoint |
| `ENABLE_STICKY_ASSIGNMENTS_DYNAMODB` | | Set to create DynamoDB table for sticky assignments (materializations) |
| `FORCE_DEPLOY` | | Force re-deploy regardless of ETag/state changes |
| `NO_DEPLOY` | | Build only, skip deployment |

## IAM Permissions

The Lambda execution role (`LAMBDA_ROLE_ARN`) needs:
- `sqs:SendMessage` on the flag-logs queue (resolver Lambda)
- `sqs:ReceiveMessage`, `sqs:DeleteMessage`, `sqs:GetQueueAttributes` on the flag-logs queue (consumer Lambda)
- `dynamodb:GetItem`, `dynamodb:PutItem` on the metrics table (if `ENABLE_METRICS`)
- `dynamodb:GetItem`, `dynamodb:PutItem`, `dynamodb:BatchGetItem`, `dynamodb:BatchWriteItem` on the materializations table (if `ENABLE_STICKY_ASSIGNMENTS_DYNAMODB`)

The deployer credentials need:
- `lambda:*` on the function names
- `sqs:CreateQueue`, `sqs:GetQueueUrl`, `sqs:GetQueueAttributes`
- `dynamodb:CreateTable`, `dynamodb:DescribeTable` (if metrics or materializations enabled)
- `sts:GetCallerIdentity`

## Deployment Lifecycle

1. Validates AWS credentials via `sts:GetCallerIdentity`
2. Computes CDN URL from `SHA256(CONFIDENCE_CLIENT_SECRET)`
3. Checks currently deployed ETag via the Function URL `/v1/state:etag`
4. Downloads resolver state from CDN with `If-None-Match` (skips on 304)
5. Creates SQS queue if not exists
6. Creates DynamoDB tables if enabled and not exist
7. Cross-compiles Rust binaries for `aarch64-unknown-linux-gnu`
8. Packages as Lambda zip files (`bootstrap` binary)
9. Creates or updates both Lambda functions
10. Creates Function URL and SQS event source mapping if not exist

## ETag-Based Skip

Like the Cloudflare deployer, this script uses ETag-based conditional fetching. If the CDN returns 304 (state unchanged) and the deployer version hasn't changed, the script exits early without rebuilding or redeploying. This makes frequent cron invocations cheap.

## AWS Resources Created

- **Lambda function** (resolver): `{prefix}-confidence-lambda-resolver`
- **Lambda function** (consumer): `{prefix}-confidence-lambda-consumer`
- **Lambda Function URL**: Public HTTPS endpoint on the resolver Lambda
- **SQS queue**: `{prefix}-confidence-lambda-resolver-flag-logs`
- **SQS event source mapping**: Connects queue to consumer Lambda (batch 100, 10s window)
- **DynamoDB table** (metrics, optional): `{prefix}-confidence-lambda-resolver-metrics`
- **DynamoDB table** (materializations, optional): `{prefix}-confidence-lambda-resolver-materializations`
